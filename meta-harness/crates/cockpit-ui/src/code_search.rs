//! A search of the code index as it's typed, for every picker that asks
//! it (`@` where text is written, the graph's find). One round per query,
//! its kinds asked together, at most one round out at a time: what's
//! typed meanwhile is asked once it's back. A round is answered once every
//! kind has answered, or once its time is up. What's shown is the last
//! round answered, whole and in the kinds' order, so a list never empties
//! and fills again at each key. Its rows read the same in every picker,
//! each carrying what it stands for. Driven one way (`drive`), and cleared
//! when its picker closes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use arch_search::{Found, Hit, SearchKind};

use crate::actions::Choice;
use crate::net::NetResult;

/// A kind's answer to a round, as it comes back (taken on the next drive).
pub type Reply = Box<dyn FnOnce(NetResult<Found>) + Send>;

/// Answers come back here: the round, the kind, what it said.
type Inbox = Arc<Mutex<Vec<(u64, SearchKind, NetResult<Found>)>>>;

/// How long a round may take before it counts as answered with what came.
pub const ROUND_LIMIT_S: f64 = 8.0;

/// A round asked: its number, the query as typed, since when, what came.
struct Round {
    number: u64,
    typed: String,
    since: f64,
    came: BTreeMap<SearchKind, Found>,
}

/// The last round answered: the query as typed (none: none yet), what
/// each kind found, in the kinds' order.
#[derive(Default)]
struct Answered {
    typed: Option<String>,
    found: Vec<(SearchKind, Found)>,
}

/// What a frame's tending gives: the round to ask now (its number and
/// the query to send), and what to tell the user.
#[derive(Debug, Default, PartialEq)]
pub struct Tended {
    pub ask: Option<(u64, String)>,
    pub note: Option<String>,
}

pub struct CodeSearch {
    /// What's asked, in the order shown.
    kinds: &'static [SearchKind],
    typed: String,
    out: Option<Round>,
    rounds: u64,
    shown: Answered,
    inbox: Inbox,
}

impl CodeSearch {
    pub fn new(kinds: &'static [SearchKind]) -> Self {
        Self { kinds, typed: String::new(), out: None, rounds: 0, shown: Answered::default(), inbox: Inbox::default() }
    }

    /// Once a frame while its picker is open, with what's typed (`query`):
    /// the answers come in, and a round due is sent, a kind at a time
    /// (`send`: the kind, the query, where its answer goes). What's to be
    /// said (a failure, a round out of time) is returned.
    pub fn drive(&mut self, query: &str, ctx: &egui::Context, mut send: impl FnMut(SearchKind, &str, Reply)) -> Option<String> {
        let answers = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let mut note = None;
        for (round, kind, answer) in answers {
            note = self.answer(round, kind, answer).or(note);
        }
        self.typed(query);
        let tended = self.tend(ctx.input(|i| i.time));
        if let Some((round, query)) = tended.ask {
            for &kind in self.kinds {
                let inbox = self.inbox.clone();
                send(kind, &query, Box::new(move |answer| inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((round, kind, answer))));
            }
        }
        if self.is_out() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(ROUND_LIMIT_S));
        }
        tended.note.or(note)
    }

    /// What's typed now.
    fn typed(&mut self, query: &str) {
        if self.typed != query {
            query.clone_into(&mut self.typed);
        }
    }

    /// Once a frame: a round answered (every kind, or its time up) becomes
    /// what's shown; then, when none is out and the rows aren't of what's
    /// typed, the round to ask. Nothing typed (or only spaces) is answered
    /// at once, with nothing, and the query the rows are of again (with
    /// other spaces at its ends) keeps them: either drops a round out.
    fn tend(&mut self, now: f64) -> Tended {
        let mut tended = Tended::default();
        let query = self.typed.trim();
        if query.is_empty() || self.shown.typed.as_deref().map(str::trim) == Some(query) {
            self.out = None;
            if self.shown.typed.as_deref() != Some(self.typed.as_str()) {
                if query.is_empty() {
                    self.shown = Answered { typed: Some(self.typed.clone()), found: Vec::new() };
                } else {
                    self.shown.typed = Some(self.typed.clone());
                }
            }
            return tended;
        }
        if let Some(out) = &self.out {
            let whole = self.kinds.iter().all(|k| out.came.contains_key(k));
            if whole || now - out.since >= ROUND_LIMIT_S {
                if !whole {
                    tended.note = Some("The code index didn't answer in time: what came is shown.".into());
                }
                let Round { typed, mut came, .. } = self.out.take().expect("a round is out");
                let found = self.kinds.iter().filter_map(|k| came.remove(k).map(|f| (*k, f))).collect();
                self.shown = Answered { typed: Some(typed), found };
            }
        }
        if self.out.is_some() || self.shown.typed.as_deref().map(str::trim) == Some(self.typed.trim()) {
            return tended;
        }
        self.rounds += 1;
        tended.ask = Some((self.rounds, self.typed.trim().to_owned()));
        self.out = Some(Round { number: self.rounds, typed: self.typed.clone(), since: now, came: BTreeMap::new() });
        tended
    }

    /// A kind's answer for round `number`: kept when that round is out (a
    /// failure as nothing found, and what went wrong returned to be said).
    fn answer(&mut self, number: u64, kind: SearchKind, answer: NetResult<Found>) -> Option<String> {
        let out = self.out.as_mut().filter(|o| o.number == number)?;
        match answer {
            Ok(found) => {
                out.came.insert(kind, found);
                None
            }
            Err(e) => {
                out.came.insert(kind, Found::default());
                Some(format!("Couldn't search the code: {}", String::from(e)))
            }
        }
    }

    /// The query the rows shown are of, as typed (none: nothing answered).
    pub fn rows_of(&self) -> Option<&str> {
        self.shown.typed.as_deref()
    }

    /// A round is out (to see its time run out, draw again).
    fn is_out(&self) -> bool {
        self.out.is_some()
    }

    /// The rows shown, named the same in every picker: what each is (a
    /// symbol, a file, a line) and where, the last of a kind saying what
    /// wasn't listed. `value` is what each stands for, picked (none: left
    /// out).
    pub fn rows(&self, value: impl Fn(SearchKind, &Hit) -> Option<String>) -> Vec<Choice> {
        let mut out = Vec::new();
        for (kind, found) in &self.shown.found {
            let (what, icon) = match kind {
                SearchKind::Symbols => ("symbol", egui_phosphor::regular::CODE),
                SearchKind::Files => ("file", egui_phosphor::regular::FILE),
                SearchKind::Text => ("line", egui_phosphor::regular::TEXT_ALIGN_LEFT),
            };
            let first = out.len();
            for hit in &found.hits {
                let Some(value) = value(*kind, hit) else { continue };
                // (The server's word for a file nothing stands for is "file".)
                let detail = if hit.detail.is_empty() || hit.detail == what { what.to_owned() } else { format!("{what} · {}", hit.detail) };
                out.push(Choice::new(value, hit.label.clone()).detail(Some(detail)).glyph(icon, crate::theme::colors().accent));
            }
            // The last listed of a kind says what wasn't: more matches than
            // shown, or a search cut at its time limit.
            if let Some(last) = out[first..].last_mut() {
                let detail = last.detail.get_or_insert_with(String::new);
                if found.total > found.hits.len() {
                    detail.push_str(&format!(" · {} of {} {what}s", found.hits.len(), found.total));
                }
                if found.partial {
                    detail.push_str(" · stopped at its time limit: there may be more");
                }
            }
        }
        out
    }

    /// Nothing asked or shown (its picker closed: what comes for the round
    /// out is dropped).
    pub fn clear(&mut self) {
        (self.typed, self.out, self.shown) = (String::new(), None, Answered::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(labels: &[&str]) -> NetResult<Found> {
        let hit = |label: &str| Hit { kind: SearchKind::Files, key: None, label: label.into(), detail: String::new(), file: None, line: None, callable: false, data: None };
        let hits = labels.iter().map(|l| hit(l)).collect::<Vec<_>>();
        Ok(Found { total: hits.len(), hits, ..Found::default() })
    }

    fn labels(search: &CodeSearch) -> Vec<String> {
        search.rows(|_, hit| Some(hit.label.clone())).into_iter().map(|c| c.label).collect()
    }

    const KINDS: &[SearchKind] = &[SearchKind::Symbols, SearchKind::Files];

    /// One round out at a time, its rows shown whole in the kinds' order
    /// whatever order they came in; what's typed meanwhile is asked once
    /// it's back, the rows of the last round shown until then.
    #[test]
    fn a_round_is_shown_whole_and_what_was_typed_meanwhile_is_asked_next() {
        let mut search = CodeSearch::new(KINDS);
        search.typed("");
        assert_eq!(search.tend(0.0), Tended::default(), "nothing typed: nothing asked");
        assert_eq!((search.rows_of(), labels(&search).len()), (Some(""), 0), "and answered at once");

        search.typed("a");
        let first = search.tend(0.0).ask.unwrap();
        assert_eq!(first.1, "a");
        search.typed("ab");
        assert_eq!(search.tend(0.1).ask, None, "one round out at a time");
        search.answer(first.0, SearchKind::Files, found(&["a.rs"]));
        assert_eq!(search.tend(0.2).ask, None, "half a round: still out");
        assert_eq!(search.rows_of(), Some(""), "the rows shown are still the last round's");
        search.answer(first.0, SearchKind::Symbols, found(&["alpha"]));
        let next = search.tend(0.3).ask.unwrap();
        assert_eq!((search.rows_of(), labels(&search)), (Some("a"), vec!["alpha".to_owned(), "a.rs".to_owned()]), "whole, symbols then files");
        assert_eq!(next.1, "ab", "what was typed meanwhile, asked once it's back");

        // An answer for a round gone is dropped (never shown); a failure is
        // said, as nothing found.
        search.answer(first.0, SearchKind::Files, found(&["late.rs"]));
        search.answer(next.0, SearchKind::Files, found(&["ab.rs"]));
        assert!(search.answer(next.0, SearchKind::Symbols, Err(crate::net::NetError::other("down"))).unwrap().contains("down"));
        search.tend(0.4);
        assert_eq!((search.rows_of(), labels(&search)), (Some("ab"), vec!["ab.rs".to_owned()]));
        // The same query with a space after it keeps its rows.
        search.typed("ab ");
        assert_eq!((search.tend(0.5).ask, search.rows_of()), (None, Some("ab ")));
        // A round out, then the field cleared, or back to what's shown:
        // answered at once, the round dropped.
        search.typed("abc");
        let (abc, _) = search.tend(0.6).ask.unwrap();
        search.typed("ab");
        assert_eq!((search.tend(0.7), search.rows_of(), search.is_out()), (Tended::default(), Some("ab"), false));
        search.answer(abc, SearchKind::Files, found(&["abc.rs"]));
        search.tend(0.75);
        assert_eq!(labels(&search), vec!["ab.rs".to_owned()], "its answers dropped");
        search.typed("abc");
        search.tend(0.8);
        search.typed("");
        assert_eq!((search.tend(0.9).ask, search.rows_of(), labels(&search).len()), (None, Some(""), 0));
    }

    #[test]
    fn a_round_out_of_time_shows_what_came_and_says_so() {
        let mut search = CodeSearch::new(KINDS);
        search.typed("a");
        let (round, _) = search.tend(0.0).ask.unwrap();
        search.answer(round, SearchKind::Files, found(&["a.rs"]));
        assert_eq!(search.tend(ROUND_LIMIT_S - 0.1).note, None);
        let tended = search.tend(ROUND_LIMIT_S);
        assert!(tended.note.unwrap().contains("in time"));
        assert_eq!((search.rows_of(), labels(&search)), (Some("a"), vec!["a.rs".to_owned()]));
        assert!(!search.is_out());
        let value = search.rows(|k, hit| Some(format!("{}:{}", k.name(), hit.label)))[0].value.clone();
        assert_eq!(value, "files:a.rs", "a row carries what it stands for");
    }

    /// Driven once a frame: a round due is sent a kind at a time, and what
    /// comes back where it was sent is shown on a later drive; a failure
    /// is said. Cleared, what comes for the round out is dropped.
    #[test]
    fn driven_it_sends_each_kind_and_takes_what_comes_back() {
        let ctx = egui::Context::default();
        let mut search = CodeSearch::new(KINDS);
        let mut sent: Vec<(SearchKind, String, Reply)> = Vec::new();
        assert_eq!(search.drive("a", &ctx, |k, q, reply| sent.push((k, q.to_owned(), reply))), None);
        assert_eq!(sent.iter().map(|(k, q, _)| (*k, q.as_str())).collect::<Vec<_>>(), vec![(SearchKind::Symbols, "a"), (SearchKind::Files, "a")]);
        for (kind, _, reply) in sent.drain(..) {
            reply(if kind == SearchKind::Files { found(&["a.rs"]) } else { Err(crate::net::NetError::other("down")) });
        }
        let note = search.drive("a", &ctx, |k, q, reply| sent.push((k, q.to_owned(), reply)));
        assert!(note.is_some_and(|n| n.contains("down")) && sent.is_empty(), "said; nothing more asked");
        assert_eq!((search.rows_of(), labels(&search)), (Some("a"), vec!["a.rs".to_owned()]));
        search.drive("ab", &ctx, |k, q, reply| sent.push((k, q.to_owned(), reply)));
        search.clear();
        sent.drain(..).for_each(|(_, _, reply)| reply(found(&["late.rs"])));
        search.drive("", &ctx, |_, _, _| panic!("nothing typed: nothing asked"));
        assert_eq!((search.rows_of(), labels(&search).len()), (Some(""), 0), "what came for the round cleared is dropped");
    }

    /// The last row listed of a kind says what wasn't listed, even when a
    /// row was left out; a file's detail is the word "file" once.
    #[test]
    fn rows_say_what_wasnt_listed_on_the_last_one_shown() {
        let mut search = CodeSearch::new(&[SearchKind::Files]);
        search.typed("a");
        let (round, _) = search.tend(0.0).ask.unwrap();
        let hit = |label: &str| Hit { kind: SearchKind::Files, key: None, label: label.into(), detail: "file".into(), file: None, line: None, callable: false, data: None };
        search.answer(round, SearchKind::Files, Ok(Found { hits: vec![hit("a.rs"), hit("b.rs"), hit("left out")], total: 9, ..Found::default() }));
        search.tend(0.1);
        let rows = search.rows(|_, hit| (hit.label != "left out").then(|| hit.label.clone()));
        let details: Vec<&str> = rows.iter().map(|c| c.detail.as_deref().unwrap_or_default()).collect();
        assert_eq!(details, ["file", "file · 3 of 9 files"], "{details:?}");
    }
}
