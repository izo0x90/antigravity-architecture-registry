//! Code coloured as the code view colours a file. A code block in a
//! message is read by the server with the language pack its fence names
//! (`/api/code/highlight`), once per block; what comes back is kept for
//! every view that draws one. Views read it (`Reader<Highlighter>`) and
//! ask for a block not known yet (`Ask::Colour`); the shell sends what's
//! asked each frame, many blocks in one call. One way of turning colours
//! into text (`coloured`), for the code view's lines and for code blocks.

use std::rc::Rc;

use arch_graph::code::Highlights;
use egui::text::{LayoutJob, TextFormat};
use egui::Color32;

use crate::asked::{Asked, Bounds, Found};
use crate::config::CodeUi;

/// Blocks kept at the most, in one call at the most, and their text in
/// one call (bytes: the server's bound).
const BOUNDS: Bounds = Bounds { kept: 1024, per_call: 64, call_bytes: 256 * 1024 };

/// A block's colours, as a view finds them.
pub enum Colours {
    /// Known: none when no pack knows its fence.
    Known(Rc<Option<Highlights>>),
    /// Not known yet: drawn plain; `asked` when it's on its way.
    Unknown { asked: bool },
}

impl Colours {
    /// Its colours, when known and there are any.
    pub fn highlights(&self) -> Option<&Highlights> {
        match self {
            Self::Known(colours) => colours.as_ref().as_ref(),
            Self::Unknown { .. } => None,
        }
    }
}

pub struct Highlighter {
    /// Each block (its fence and text): its colours, none when no pack
    /// knows its fence.
    asked: Asked<(String, String), Option<Highlights>>,
    /// What a block with no fence is: plain.
    plain: Rc<Option<Highlights>>,
    /// Each capture's theme token (the code view's settings), and which
    /// change of the settings it took.
    tokens: CodeUi,
    seen_settings: u64,
}

impl Default for Highlighter {
    fn default() -> Self {
        Self { asked: Asked::new(BOUNDS), plain: Rc::new(None), tokens: CodeUi::default(), seen_settings: 0 }
    }
}

impl Highlighter {
    /// `text`'s colours as the language `fence` names. A fence that names
    /// none (an empty one) is known: plain.
    pub fn colours(&self, fence: &str, text: &str) -> Colours {
        let fence = fence.trim();
        if fence.is_empty() {
            return Colours::Known(self.plain.clone());
        }
        match self.asked.get(crate::asked::key(&(fence, text))) {
            Found::Known(colours) => Colours::Known(colours),
            Found::Unknown { asked } => Colours::Unknown { asked },
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
            let fence = fence.trim().to_owned();
            let (key, size) = (crate::asked::key(&(fence.as_str(), text.as_str())), text.len());
            self.asked.want(key, (fence, text), size);
        }
    }

    /// Once a frame (the shell): the code view's settings taken when they
    /// changed (each capture's token), what came back kept, what's asked
    /// sent.
    pub fn tick(&mut self, net: &crate::net::NetClient, ctx: &egui::Context, settings: &crate::shared::Kept<crate::shared::Settings>) {
        if let Some(settings) = settings.since(&mut self.seen_settings) {
            self.tokens = settings.code.clone();
        }
        self.asked.tick(ctx.input(|i| i.time), |blocks, reply| {
            net.code_highlight(&blocks, ctx.clone(), move |result| reply.answered(result.ok()));
        });
    }
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
