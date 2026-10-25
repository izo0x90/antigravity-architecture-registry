//! Code coloured as the code view colours a file. A code block in a
//! message is read by the server with the language pack its fence names
//! (`/api/code/highlight`), once per block; what comes back is kept for
//! every view that draws one. Views read it (`Reader<Highlighter>`) and
//! ask for a block not known yet (`Ask::Colour`); the shell sends what's
//! asked each frame, many blocks in one call. One way of turning colours
//! into text (`coloured`), for the code view's lines and for code blocks.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use arch_graph::code::Highlights;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

use crate::config::CodeUi;

/// Blocks kept at the most; past that, the longest unused go first.
const KEPT: usize = 1024;

/// Blocks in one call at the most, and their text (bytes): the server's
/// bound.
const CALL_BLOCKS: usize = 64;
const CALL_BYTES: usize = 256 * 1024;

/// A call that wasn't answered (the server out of reach): asked again
/// after this long (seconds).
const RETRY_S: f64 = 30.0;

/// A block's colours as known here.
#[derive(Clone)]
enum Known {
    /// Asked for, not come yet.
    Asked,
    /// Come: none when no pack knows its fence.
    Got(Option<Rc<Highlights>>),
    /// Its call failed, at this time: asked again after a while.
    Failed(f64),
}

/// A block's colours, as a view finds them.
pub enum Colours {
    /// Known: none when no pack knows its fence.
    Known(Option<Rc<Highlights>>),
    /// Not known yet: drawn plain; `asked` when it's on its way.
    Unknown { asked: bool },
}

/// What came back, by block: its colours, or that the call failed.
type Mailbox = Arc<Mutex<Vec<(u64, Result<Option<Highlights>, ()>)>>>;

#[derive(Default)]
pub struct Highlighter {
    known: HashMap<u64, Known>,
    /// The blocks known, the least recently asked first.
    order: VecDeque<u64>,
    /// Blocks asked for and not sent yet: their key, fence and text.
    wanted: Vec<(u64, String, String)>,
    mailbox: Mailbox,
    /// Each capture's theme token (the code view's settings), and which
    /// change of the settings it took.
    tokens: CodeUi,
    seen_settings: u64,
    /// The clock (seconds) as of this frame.
    now: f64,
}

impl Highlighter {
    /// `text`'s colours as the language `fence` names. A fence that names
    /// none (an empty one) is known: plain.
    pub fn colours(&self, fence: &str, text: &str) -> Colours {
        let fence = fence.trim();
        if fence.is_empty() {
            return Colours::Known(None);
        }
        match self.known.get(&key(fence, text)) {
            Some(Known::Got(colours)) => Colours::Known(colours.clone()),
            Some(Known::Asked) => Colours::Unknown { asked: true },
            Some(Known::Failed(at)) => Colours::Unknown { asked: self.now - at < RETRY_S },
            None => Colours::Unknown { asked: false },
        }
    }

    /// A capture's colour (`keyword`, `function.method`), by the code
    /// view's settings and the theme.
    pub fn colour(&self, capture: &str) -> Option<Color32> {
        self.tokens.token(capture).and_then(|t| crate::theme::colors().token(t))
    }

    /// Blocks a view asked for (`Ask::Colour`: each its fence and text):
    /// sent with the next frame's call.
    pub fn want(&mut self, blocks: Vec<(String, String)>) {
        for (fence, text) in blocks {
            let key = key(fence.trim(), &text);
            if let Colours::Unknown { asked: false } = self.colours(&fence, &text)
                && !self.wanted.iter().any(|(k, _, _)| *k == key)
            {
                self.wanted.push((key, fence.trim().to_owned(), text));
            }
        }
    }

    /// Once a frame (the shell): the code view's settings taken when they
    /// changed (each capture's token), what came back kept, what's asked
    /// sent, in calls of a bounded size.
    pub fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context, settings: &crate::shared::Kept<crate::shared::Settings>) {
        self.now = ctx.input(|i| i.time);
        if let Some(settings) = settings.since(&mut self.seen_settings) {
            self.tokens = settings.code.clone();
        }
        self.take_came();
        let mut wanted = std::mem::take(&mut self.wanted).into_iter().peekable();
        while wanted.peek().is_some() {
            let mut call: Vec<(u64, String, String)> = Vec::new();
            let mut bytes = 0;
            while let Some(next) = wanted.next_if(|(_, _, text)| call.is_empty() || (call.len() < CALL_BLOCKS && bytes + text.len() <= CALL_BYTES)) {
                bytes += next.2.len();
                call.push(next);
            }
            for (key, _, _) in &call {
                self.keep(*key, Known::Asked);
            }
            let keys: Vec<u64> = call.iter().map(|(key, _, _)| *key).collect();
            let blocks: Vec<(String, String)> = call.into_iter().map(|(_, fence, text)| (fence, text)).collect();
            let mailbox = self.mailbox.clone();
            net.code_highlight(&blocks, ctx.clone(), move |result| {
                let mut came = mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                match result {
                    Ok(each) if each.len() == keys.len() => came.extend(keys.into_iter().zip(each).map(|(key, colours)| (key, Ok(colours)))),
                    _ => came.extend(keys.into_iter().map(|key| (key, Err(())))),
                }
            });
        }
    }

    /// What came back, kept.
    fn take_came(&mut self) {
        let came = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (key, colours) in came {
            let known = match colours {
                Ok(colours) => Known::Got(colours.map(Rc::new)),
                Err(()) => Known::Failed(self.now),
            };
            self.keep(key, known);
        }
    }

    /// `known` kept for `key`, as the most recently asked; past `KEPT`,
    /// the least recently asked go.
    fn keep(&mut self, key: u64, known: Known) {
        if self.known.insert(key, known).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > KEPT {
            if let Some(oldest) = self.order.pop_front() {
                self.known.remove(&oldest);
            }
        }
    }

    /// As if the server had said `fence`'s `text` is coloured so.
    #[cfg(test)]
    pub(crate) fn knows(&mut self, fence: &str, text: &str, colours: Highlights) {
        self.keep(key(fence, text), Known::Got(Some(Rc::new(colours))));
    }
}

fn key(fence: &str, text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (fence, text).hash(&mut hasher);
    hasher.finish()
}

/// `text[span]` as a layout, coloured by `highlights` (byte runs over the
/// whole of `text`, each with a capture's name), `colour` giving each
/// capture's colour and `base` the rest; `format` makes each part's look.
pub fn coloured(text: &str, span: std::ops::Range<usize>, highlights: Option<&Highlights>, colour: impl Fn(&str) -> Option<Color32>, base: Color32, format: impl Fn(Color32) -> TextFormat) -> LayoutJob {
    let mut job = LayoutJob::default();
    let Some(highlights) = highlights else {
        job.append(&text[span], 0.0, format(base));
        return job;
    };
    let (start, end) = (span.start, span.end);
    let first = highlights.runs.partition_point(|r| (r[1] as usize) <= start);
    let mut at = start;
    for run in &highlights.runs[first..] {
        let (s, e) = ((run[0] as usize).max(start), (run[1] as usize).min(end));
        if s >= end {
            break;
        }
        if s > at {
            job.append(&text[at..s], 0.0, format(base));
        }
        if e > s.max(at) {
            let color = highlights.names.get(run[2] as usize).and_then(|n| colour(n)).unwrap_or(base);
            job.append(&text[s.max(at)..e], 0.0, format(color));
            at = e;
        }
    }
    if at < end {
        job.append(&text[at..end], 0.0, format(base));
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What came back is kept, a failed call asked again only after a
    /// while; past what's kept, the least recently asked go first.
    #[test]
    fn what_comes_back_is_kept_and_a_failure_retried_later() {
        let mut h = Highlighter::default();
        let (got, failed) = (key("rust", "fn x() {}"), key("rust", "fn y() {}"));
        h.mailbox.lock().unwrap().extend([(got, Ok(Some(Highlights { names: vec!["keyword".into()], runs: vec![[0, 2, 0]] }))), (failed, Err(()))]);
        h.now = 100.0;
        h.take_came();
        assert!(matches!(h.colours("rust", "fn x() {}"), Colours::Known(Some(c)) if c.names == ["keyword"]));
        assert!(matches!(h.colours("rust", "fn y() {}"), Colours::Unknown { asked: true }), "failed just now: not asked again yet");
        h.now = 100.0 + RETRY_S + 1.0;
        h.want(vec![("rust".into(), "fn y() {}".into())]);
        assert_eq!(h.wanted.len(), 1, "asked again after a while");
        for n in 0..KEPT {
            h.keep(key("rust", &n.to_string()), Known::Asked);
        }
        assert!(matches!(h.colours("rust", "fn x() {}"), Colours::Unknown { asked: false }), "the least recently asked gone");
        assert_eq!(h.known.len(), KEPT);
    }

    /// Text is coloured by its runs within the span asked for; the rest in
    /// the base colour.
    #[test]
    fn text_is_coloured_by_its_runs() {
        let text = "fn run() {}\nlet x = 1;";
        let highlights = Highlights { names: vec!["keyword".into()], runs: vec![[0, 2, 0], [12, 15, 0]] };
        let red = Color32::RED;
        let job = coloured(text, 12..text.len(), Some(&highlights), |n| (n == "keyword").then_some(red), Color32::GRAY, |color| TextFormat { color, ..Default::default() });
        let parts: Vec<(&str, Color32)> = job.sections.iter().map(|s| (&job.text[s.byte_range.clone()], s.format.color)).collect();
        assert_eq!(parts, [("let", red), (" x = 1;", Color32::GRAY)]);
    }
}
