//! What a buffer tells its host (a panel, the quick look) about itself, and
//! the top line the host draws from it. A buffer draws its content; its top
//! line is the host's, the same for every buffer: its menus first (the
//! leader's groups about where it is), where nothing that comes and goes can
//! push them; then a place kept for its busy mark; then its title and each
//! status, wrapping after them (one line that wraps); its settings on a row
//! under.

use egui::RichText;

/// Where a host shows the buttons of what you're on (the cursor's item,
/// the box being typed in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemButtons {
    /// In a box under the item (over what's below it), following it.
    #[default]
    Floating,
    /// In a row of their own along the host's bottom, kept whether there
    /// are any or not: nothing is covered and nothing moves.
    Bar,
}

impl crate::config::Named for ItemButtons {
    const WHAT: &'static str = "place for the buttons";
    const ALL: &'static [Self] = &[Self::Floating, Self::Bar];

    fn name(self) -> &'static str {
        match self {
            Self::Floating => "floating",
            Self::Bar => "bar",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Self::Floating => "Floating",
            Self::Bar => "A bar",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Floating => "In a box under the item, following it",
            Self::Bar => "In a row along each panel's bottom: nothing covered, nothing moves",
        }
    }
}

use crate::actions::{At, Bound, Control, RunCx};

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
    /// it gives them here: a card's go-tos under it, re-reading a file
    /// that changed under the line you're on. Under the item, they're what
    /// you're on.
    fn placed(&self) -> Vec<(String, Control)> {
        Vec::new()
    }

    /// Where you are, when it isn't the cursor's item (a chat following
    /// the latest: what's waiting on you there).
    fn here(&self) -> Option<String> {
        None
    }

    /// Where a place `here` names is drawn when it's no item (the chat's
    /// banners, the graph's size note): items are found among the spots,
    /// which hints label; these aren't.
    fn drawn_at(&self, _place: &str) -> Option<egui::Rect> {
        None
    }

    /// It reads the pointer on its content itself (the graph's canvas:
    /// cards, dragging, two fingers); the host's clicks leave it alone.
    fn own_pointer(&self) -> bool {
        false
    }

    /// Where its typing box is, while it's typed in, with what's drawn on
    /// it (a chat's sub-agent strip): the box's controls go under it, or
    /// over it, never on it.
    fn typing_rect(&self) -> Option<egui::Rect> {
        None
    }

    /// What its field takes while typed in: text (the way back out leaves
    /// it), or every key (only `[keymap.input] leave` leaves it).
    fn field(&self) -> crate::keymap::Field {
        crate::keymap::Field::Text
    }
}

/// What kind of note the panel's top line names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteMark {
    Tag,
    Discussions,
}

/// A group of the leader's, as a line offers it: its key (after the
/// leader), its name, the keys that open it (empty: not said here), what it
/// is (on hover, or a finger held on it), and what tapping it asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupButton {
    pub key: char,
    pub name: String,
    pub keys: String,
    pub tip: String,
    pub asks: (String, Option<String>),
}

impl GroupButton {
    /// Drawn where its keys don't open it (a panel not focused): its keys
    /// unsaid.
    pub fn unkeyed(mut self) -> Self {
        self.keys.clear();
        self
    }
}

/// What a host's top line starts with: the panel's number (none for the
/// quick look), whether it's focused, and its groups.
pub struct TopLine<'a> {
    pub number: Option<usize>,
    pub focused: bool,
    pub groups: &'a [GroupButton],
}

/// The panel's top line: its number, its groups as buttons (each its name
/// and the keys that open it; tapped, its menu opens, asked for as every
/// control asks), the place for its busy mark, its title and status, the
/// notes on what the cursor's on (each tag a solid tag in its colour and its
/// name; its discussions a speech bubble and a few words); under it, its
/// settings (`bound`: the panel's layer of where you are). Where the top
/// line was drawn.
pub fn header(ui: &mut egui::Ui, panel: &dyn Hosted, top: &TopLine<'_>, bound: &[Bound], notes: &[(NoteMark, String, egui::Color32)], cx: &mut RunCx<'_>) -> egui::Rect {
    let look = crate::theme::look();
    // One line that wraps (a narrow panel, a phone): nothing overlaps.
    // The groups first: what follows them grows and wraps after them.
    let line = ui.horizontal_wrapped(|ui| {
        if let Some(number) = top.number {
            let colour = if top.focused { crate::theme::colors().focus } else { crate::theme::colors().faint };
            ui.label(RichText::new((number + 1).to_string()).strong().monospace().size(look.text_body).color(colour));
        }
        groups(ui, top.groups, cx);
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
        for (mark, name, colour) in notes {
            match mark {
                NoteMark::Tag => ui.label(crate::theme::tag_mark(ui, *colour, look.text_small)),
                NoteMark::Discussions => ui.label(RichText::new(egui_phosphor::fill::CHAT_CIRCLE).font(crate::theme::solid_font(ui, look.text_small)).color(*colour)),
            };
            ui.add(egui::Label::new(RichText::new(name).size(look.text_small).color(*colour)).truncate());
        }
    });
    let settings = || bound.iter().filter(|b| b.action.at() == Some(At::Settings));
    if settings().next().is_some() {
        ui.horizontal_wrapped(|ui| crate::actions::controls(ui, settings(), cx));
    }
    line.response.rect
}

/// Groups as buttons, each said as every control says itself (its name,
/// then its keys when there's room); a tap asks for what it asks, as every
/// control's does.
pub fn groups(ui: &mut egui::Ui, groups: &[GroupButton], cx: &mut RunCx<'_>) {
    for group in groups {
        let text = crate::actions::caption_and_key(ui, &group.name, &group.keys, None);
        if ui.button(text).on_hover_text(&group.tip).clicked() {
            *cx.asked = Some(group.asks.clone());
        }
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

/// One thing said, in its tone (more on hover). Cut to fit the line, all
/// of it on hover: what a buffer says (a terminal's title is whatever its
/// program sets) never makes its panel wider.
fn said(ui: &mut egui::Ui, status: &Status, size: f32, strong: bool) {
    let mut text = RichText::new(&status.text).size(size).color(status.tone.color());
    if strong {
        text = text.strong();
    }
    let label = ui.add(egui::Label::new(text).truncate());
    if let Some(detail) = &status.detail {
        label.on_hover_text(detail);
    }
}
