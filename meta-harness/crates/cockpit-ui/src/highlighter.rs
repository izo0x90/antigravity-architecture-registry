//! Code coloured as the code view colours a file. A code block in a
//! message is read by the server with the language pack its fence names
//! (`/api/code/highlight`), once per block; what comes back is kept for
//! every view that draws one. Views read it (`Reader<Highlighter>`):
//! asking for a block not known yet queues it, and the shell sends what's
//! queued each frame. One way of turning colours into text (`coloured`),
//! for the code view's lines and for code blocks.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use arch_graph::code::Highlights;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

use crate::config::CodeUi;

/// Blocks kept at the most; past that, what's kept starts afresh (what's
/// on screen is asked for again).
const KEPT: usize = 1024;

/// A block's colours: asked for, or come (none: no pack knows its fence).
enum Known {
    Asked,
    Got(Option<Rc<Highlights>>),
}

/// What came back, by block.
type Mailbox = Arc<Mutex<Vec<(u64, Option<Highlights>)>>>;

#[derive(Default)]
pub struct Highlighter {
    known: HashMap<u64, Known>,
    /// Blocks asked for since the last frame: their key, fence and text.
    queued: RefCell<Vec<(u64, String, String)>>,
    mailbox: Mailbox,
    /// Each capture's theme token (the code view's settings), and which
    /// change of the settings it took.
    tokens: CodeUi,
    seen_settings: u64,
}

impl Highlighter {
    /// `text`'s colours as the language `fence` names; none while they
    /// haven't come (asked for, the first time), or when no pack knows it.
    pub fn colours(&self, fence: &str, text: &str) -> Option<Rc<Highlights>> {
        let fence = fence.trim();
        if fence.is_empty() {
            return None;
        }
        let key = key(fence, text);
        match self.known.get(&key) {
            Some(Known::Got(colours)) => colours.clone(),
            Some(Known::Asked) => None,
            None => {
                let mut queued = self.queued.borrow_mut();
                if !queued.iter().any(|(k, _, _)| *k == key) {
                    queued.push((key, fence.to_owned(), text.to_owned()));
                }
                None
            }
        }
    }

    /// A capture's colour (`keyword`, `function.method`), by the code
    /// view's settings and the theme.
    pub fn colour(&self, capture: &str) -> Option<Color32> {
        self.tokens.token(capture).and_then(|t| crate::theme::colors().token(t))
    }

    /// Once a frame (the shell): the code view's settings taken when they
    /// changed (each capture's token), what came back kept, what's queued
    /// asked for.
    pub fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context, settings: &crate::shared::Kept<crate::shared::Settings>) {
        if let Some(settings) = settings.since(&mut self.seen_settings) {
            self.tokens = settings.code.clone();
        }
        self.take_came();
        let queued = std::mem::take(self.queued.get_mut());
        if queued.is_empty() {
            return;
        }
        if self.known.len() + queued.len() > KEPT {
            self.known.clear();
        }
        for (key, fence, text) in queued {
            if self.known.contains_key(&key) {
                continue;
            }
            self.known.insert(key, Known::Asked);
            let mailbox = self.mailbox.clone();
            // (Refused, or not reached: plain, not asked again.)
            net.code_highlight(&fence, &text, ctx.clone(), move |result| {
                mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push((key, result.ok().flatten()));
            });
        }
    }
}

impl Highlighter {
    /// As if the server had said `fence`'s `text` is coloured so.
    #[cfg(test)]
    pub(crate) fn knows(&mut self, fence: &str, text: &str, colours: Highlights) {
        self.known.insert(key(fence, text), Known::Got(Some(Rc::new(colours))));
    }

    /// What came back, kept.
    fn take_came(&mut self) {
        let came = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for (key, colours) in came {
            self.known.insert(key, Known::Got(colours.map(Rc::new)));
        }
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

    /// A block is asked for once, its colours kept when they come; a block
    /// with no fence isn't asked for.
    #[test]
    fn a_block_is_asked_for_once_and_kept() {
        let mut h = Highlighter::default();
        assert!(h.colours("rust", "fn x() {}").is_none() && h.colours("rust", "fn x() {}").is_none());
        assert!(h.colours("", "plain").is_none());
        assert_eq!(h.queued.borrow().len(), 1, "asked once");
        let key = key("rust", "fn x() {}");
        h.known.insert(key, Known::Asked);
        h.queued.borrow_mut().clear();
        assert!(h.colours("rust", "fn x() {}").is_none() && h.queued.borrow().is_empty(), "asked: not again");
        h.mailbox.lock().unwrap().push((key, Some(Highlights { names: vec!["keyword".into()], runs: vec![[0, 2, 0]] })));
        h.take_came();
        assert_eq!(h.colours("rust", "fn x() {}").map(|c| c.names.clone()), Some(vec!["keyword".to_owned()]));
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
