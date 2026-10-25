//! What views ask the server to work out from text they hold (a code
//! block's colours, a change's hunks), kept for every view that draws it:
//! each asked once, many in one call (of a bounded size: one bigger than
//! a call may be isn't asked, said too big), kept (past a bound, the
//! longest unused go first), a call that failed asked again after a
//! while. Views find what's known and ask through `Ask` for what isn't;
//! the shell sends what's asked once a frame.

use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// A call that wasn't answered (the server out of reach): asked again
/// after this long (seconds).
pub const RETRY_S: f64 = 30.0;

/// What's asked, as kept: by its key (`key`).
pub type Key = u64;

/// `question`'s key.
pub fn key(question: &impl Hash) -> Key {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    question.hash(&mut hasher);
    hasher.finish()
}

/// What's known of one question.
enum Known<A> {
    /// Asked for, not come yet.
    Asked,
    Got(Rc<A>),
    /// Its call failed, at this time: asked again after a while.
    Failed(f64),
    /// Bigger than a call may be: never asked.
    TooBig,
}

/// An answer, as a view finds it.
pub enum Found<A> {
    Known(Rc<A>),
    /// Not known yet; `asked` when it's on its way (or failed a moment
    /// ago).
    Unknown { asked: bool },
    /// Bigger than the server takes: not asked.
    TooBig,
}

/// What came back, by question: its answer, or that the call failed.
type Mailbox<A> = Arc<Mutex<Vec<(Key, Result<A, ()>)>>>;

/// How big a call may be: its questions, their text (bytes).
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub kept: usize,
    pub per_call: usize,
    pub call_bytes: usize,
}

/// Questions `Q` asked of the server, answers `A` kept.
pub struct Asked<Q, A> {
    /// Each question known, and when it was last used (by `used`).
    known: HashMap<Key, (Known<A>, Cell<u64>)>,
    used: Cell<u64>,
    /// Asked for and not sent yet: each with its key and size.
    wanted: Vec<(Key, Q, usize)>,
    mailbox: Mailbox<A>,
    bounds: Bounds,
    /// The clock (seconds) as of this frame.
    now: f64,
}

impl<Q, A> Asked<Q, A> {
    pub fn new(bounds: Bounds) -> Self {
        Self { known: HashMap::new(), used: Cell::new(0), wanted: Vec::new(), mailbox: Arc::default(), bounds, now: 0.0 }
    }

    /// What's known for `key` (used now: kept the longer).
    pub fn get(&self, key: Key) -> Found<A> {
        let Some((known, used)) = self.known.get(&key) else { return Found::Unknown { asked: false } };
        self.used.set(self.used.get() + 1);
        used.set(self.used.get());
        match known {
            Known::Got(answer) => Found::Known(answer.clone()),
            Known::Asked => Found::Unknown { asked: true },
            Known::Failed(at) => Found::Unknown { asked: self.now - at < RETRY_S },
            Known::TooBig => Found::TooBig,
        }
    }

    /// `question` (its key, its size), sent with the next call unless it's
    /// known, on its way or wanted already; one bigger than a call may be,
    /// known as too big.
    pub fn want(&mut self, key: Key, question: Q, size: usize) {
        if size > self.bounds.call_bytes {
            self.keep(key, Known::TooBig);
            return;
        }
        if let Found::Unknown { asked: false } = self.get(key)
            && !self.wanted.iter().any(|(k, ..)| *k == key)
        {
            self.wanted.push((key, question, size));
        }
    }

    /// Once a frame (the shell, at `now`): what came back kept, what's
    /// asked sent in calls of a bounded size (`send` makes each call; its
    /// answers, in the order asked, go to the reply it's handed).
    pub fn tick(&mut self, now: f64, mut send: impl FnMut(Vec<Q>, Reply<A>)) {
        self.now = now;
        let came = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (key, answer) in came {
            let known = match answer {
                Ok(answer) => Known::Got(Rc::new(answer)),
                Err(()) => Known::Failed(now),
            };
            self.keep(key, known);
        }
        let mut wanted = std::mem::take(&mut self.wanted).into_iter().peekable();
        while wanted.peek().is_some() {
            let (mut keys, mut call, mut bytes) = (Vec::new(), Vec::new(), 0);
            while let Some((key, question, size)) = wanted.next_if(|(.., size)| call.is_empty() || (call.len() < self.bounds.per_call && bytes + size <= self.bounds.call_bytes)) {
                bytes += size;
                keys.push(key);
                call.push(question);
            }
            for key in &keys {
                self.keep(*key, Known::Asked);
            }
            send(call, Reply { keys, mailbox: self.mailbox.clone() });
        }
    }

    /// `known` kept for `key`, used now; past what's kept, the longest
    /// unused go (a tenth at a time).
    fn keep(&mut self, key: Key, known: Known<A>) {
        self.used.set(self.used.get() + 1);
        self.known.insert(key, (known, Cell::new(self.used.get())));
        if self.known.len() > self.bounds.kept {
            let mut by_use: Vec<(u64, Key)> = self.known.iter().map(|(k, (_, used))| (used.get(), *k)).collect();
            by_use.sort_unstable();
            for (_, key) in by_use.into_iter().take(self.known.len() - self.bounds.kept + self.bounds.kept / 10) {
                self.known.remove(&key);
            }
        }
    }

    /// As if the server had answered `key` so.
    #[cfg(test)]
    pub(crate) fn knows(&mut self, key: Key, answer: A) {
        self.keep(key, Known::Got(Rc::new(answer)));
    }
}

/// Where a call's answers go.
pub struct Reply<A> {
    keys: Vec<Key>,
    mailbox: Mailbox<A>,
}

impl<A> Reply<A> {
    /// The call's answers, one for each question in the order asked (any
    /// other count, or none: the call failed).
    pub fn answered(self, answers: Option<Vec<A>>) {
        let mut came = self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match answers {
            Some(each) if each.len() == self.keys.len() => came.extend(self.keys.into_iter().zip(each).map(|(key, answer)| (key, Ok(answer)))),
            _ => came.extend(self.keys.into_iter().map(|key| (key, Err(())))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Bounds = Bounds { kept: 4, per_call: 2, call_bytes: 10 };

    /// Asked once, in calls of a bounded size; what came back kept, a
    /// failed call asked again only after a while; past what's kept, the
    /// longest unused go first; one too big for a call, never asked.
    #[test]
    fn what_is_asked_goes_in_bounded_calls_is_kept_and_a_failure_retried_later() {
        let mut asked: Asked<&str, usize> = Asked::new(SMALL);
        for q in ["a", "b", "c", "a"] {
            asked.want(key(&q), q, 4);
        }
        let mut calls = Vec::new();
        asked.tick(100.0, |questions, reply| calls.push((questions, reply)));
        assert_eq!(calls.iter().map(|(q, _)| q.clone()).collect::<Vec<_>>(), [vec!["a", "b"], vec!["c"]], "each once; two to a call, ten bytes at most");
        asked.want(key(&"a"), "a", 4);
        asked.tick(100.0, |_, _| panic!("on its way: not asked again"));
        let mut calls = calls.into_iter();
        let (_, first) = calls.next().unwrap();
        first.answered(Some(vec![1, 2]));
        let (_, second) = calls.next().unwrap();
        second.answered(None);
        asked.tick(100.0, |_, _| {});
        assert!(matches!(asked.get(key(&"a")), Found::Known(n) if *n == 1));
        assert!(matches!(asked.get(key(&"c")), Found::Unknown { asked: true }), "failed just now: not asked again yet");
        asked.tick(100.0 + RETRY_S + 1.0, |_, _| {});
        asked.want(key(&"c"), "c", 4);
        let mut again = Vec::new();
        asked.tick(100.0 + RETRY_S + 1.0, |questions, _| again.push(questions));
        assert_eq!(again, [vec!["c"]], "asked again after a while");
        // ("c" used just now; the longest unused, "a" and "b", go.)
        let _ = asked.get(key(&"c"));
        for q in ["d", "e", "f"] {
            asked.knows(key(&q), 0);
        }
        assert!(matches!(asked.get(key(&"a")), Found::Unknown { asked: false }), "the longest unused gone");
        assert!(!matches!(asked.get(key(&"c")), Found::Unknown { asked: false }), "one used lately kept");
        // Bigger than a call may be: never asked.
        asked.want(key(&"huge"), "huge", 11);
        asked.tick(200.0, |_, _| panic!("not asked"));
        assert!(matches!(asked.get(key(&"huge")), Found::TooBig));
    }
}
