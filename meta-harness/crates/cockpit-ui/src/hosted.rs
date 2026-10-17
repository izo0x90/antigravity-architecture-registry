//! What a buffer tells its host (a panel, the quick look) about itself, and
//! the header the host draws from it. A buffer draws its content; its top
//! line (its controls, what it's doing, its title) is the host's, the same
//! for every buffer: its
//! header buttons first, where nothing that comes and goes can push them;
//! then a place kept for its busy mark; then its title and each status,
//! wrapping after them (one line that wraps); its settings on a row under.

use egui::RichText;
use serde::{Deserialize, Serialize};

/// Where a host shows the buttons of what you're on (the cursor's item,
/// the box being typed in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemButtons {
    /// In a box under the item (over what's below it), following it.
    #[default]
    Floating,
    /// In a row of their own along the host's bottom, kept whether there
    /// are any or not: nothing is covered and nothing moves.
    Bar,
}

use crate::actions::{At, Bound, Control, RunCx, Style};

pub use crate::actions::Tone;

/// One thing the view says about what it's doing (signed in as, loading,
/// files changed, tokens used…).
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub text: String,
    pub tone: Tone,
    /// More, on hover.
    pub detail: Option<String>,
}

impl Status {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Self { text: text.into(), tone, detail: None }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A view as its host shows it.
pub trait Hosted {
    /// Its name on the top line.
    fn title(&self) -> Status;

    /// What it says about itself, after the title.
    fn status(&self) -> Vec<Status> {
        Vec::new()
    }

    /// What it has to say to the whole desk (the top bar shows it): an
    /// answer cut short or slow.
    fn alert(&self) -> Option<Status> {
        None
    }

    /// What it's waiting on right now (loading, connecting), in words: a
    /// mark in a place kept for it, so it comes and goes without moving
    /// anything.
    fn busy(&self) -> Option<String> {
        None
    }

    /// Actions it shows (its own or the shell's, by id), with the control
    /// it gives them here: the graph's back and forward on its top line, a
    /// card's go-tos under it. Placed by it, they're its panel's (or, under
    /// the item, what you're on).
    fn placed(&self) -> Vec<(String, Control)> {
        Vec::new()
    }

    /// Where you are, when it isn't the cursor's item (a chat following
    /// the latest: what's waiting on you there).
    fn here(&self) -> Option<String> {
        None
    }

    /// It reads the pointer on its content itself (the graph's canvas:
    /// cards, dragging, two fingers); the host's clicks leave it alone.
    fn own_pointer(&self) -> bool {
        false
    }

    /// Where its typing box is, while it's typed in (its controls go under).
    fn typing_rect(&self) -> Option<egui::Rect> {
        None
    }
}

/// The panel's top line: its bound actions' header buttons (`bound`: the
/// panel's layer of where you are), the place for its busy mark, its title
/// and status; its settings on a row under.
pub fn header(ui: &mut egui::Ui, panel: &dyn Hosted, bound: &[Bound], cx: &mut RunCx<'_>) {
    let look = crate::theme::look();
    let at_header = |style: Style| bound.iter().filter(move |b| b.action.control.as_ref().is_some_and(|c| c.at == At::Header && c.style == style));
    // One line that wraps (a narrow panel, a phone): nothing overlaps.
    // The buttons first: what follows them grows and wraps after them.
    ui.horizontal_wrapped(|ui| {
        crate::actions::controls(ui, at_header(Style::Button), cx);
        // The busy mark's place, kept whether it shows or not.
        let mark = egui::WidgetText::from(RichText::new(egui_phosphor::regular::HOURGLASS).size(look.text_label).color(Tone::Warn.color()));
        let galley = mark.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Body);
        let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
        if let Some(doing) = panel.busy() {
            ui.painter().galley(rect.min, galley, Tone::Warn.color());
            response.on_hover_text(doing);
        }
        said(ui, &panel.title(), look.text_label, true);
        for status in panel.status() {
            said(ui, &status, look.text_small, false);
        }
    });
    if at_header(Style::Setting).next().is_some() {
        ui.horizontal_wrapped(|ui| crate::actions::controls(ui, at_header(Style::Setting), cx));
    }
}

/// What other buffers say about what a card shows (the quick look's,
/// above its card): one line that wraps.
pub fn says(ui: &mut egui::Ui, statuses: &[Status]) {
    ui.horizontal_wrapped(|ui| {
        for status in statuses {
            said(ui, status, crate::theme::look().text_body, false);
        }
    });
}

/// One thing said, in its tone (more on hover).
fn said(ui: &mut egui::Ui, status: &Status, size: f32, strong: bool) {
    let mut text = RichText::new(&status.text).size(size).color(status.tone.color());
    if strong {
        text = text.strong();
    }
    let label = ui.add(egui::Label::new(text).extend());
    if let Some(detail) = &status.detail {
        label.on_hover_text(detail);
    }
}
