//! A buffer: what a panel (or the quick look) shows. The shell holds every
//! buffer as one of these, by its id, and drives it only through this
//! trait and the ones it builds on (moving and landing: `Navigable`; its
//! actions: `ActionProvider`; its top line: `Hosted`). What a buffer needs
//! from the rest of the cockpit it reads from what the desk shares
//! (`Navigable::share`) and asks for (`Navigable::take_asks`).

use std::any::Any;

use arch_graph::{ArchEvent, CodeAnchor, ComponentId};

use crate::actions::{ActionProvider, RunCx};
use crate::kind::BufferKind;
use crate::feedback::FeedbackInspectorState;
use crate::hosted::{Hosted, Status};
use crate::nav::Navigable;
use crate::net::NetClient;

/// How it's drawn this frame, beside what any action has (`RunCx`).
pub struct Drawing<'a> {
    /// The developer inspector (what's drawn can be pointed at).
    pub feedback: &'a mut FeedbackInspectorState,
    /// Its panel has focus.
    pub focused: bool,
    /// It has the keyboard: its text field takes what's typed. Its host
    /// says so; a buffer never takes the keyboard on its own.
    pub typing: bool,
    /// The leader key is down (the pointer pans what can be panned).
    pub leader_held: bool,
}

/// What a buffer knows about a component.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Known {
    pub name: Option<String>,
    /// Where its code is (file, first and last line); `Some(None)`: it
    /// knows the component isn't in the code.
    pub code: Option<Option<(String, u32, u32)>>,
    /// It isn't written yet: its code place is the file it's planned in.
    pub planned: bool,
    /// What it says about it beside a card of it (the quick look).
    pub says: Vec<Status>,
}

impl Known {
    /// What a component says of itself: its name and where its code is.
    pub fn of(c: &arch_graph::Component) -> Self {
        Self { name: Some(c.name.clone()), code: Some(code_of(c)), planned: c.anchor.is_none() && c.proposed.is_some(), says: Vec::new() }
    }
}

pub trait Buffer: Navigable + ActionProvider + Hosted + Any {
    /// Draws its content (its host draws its top line and controls).
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut Drawing<'_>);

    /// Every frame while it's the one of its kind in use: replies taken,
    /// what it needs asked for.
    fn tick(&mut self, _net: &NetClient, _ctx: &egui::Context) {}

    /// It was drawn last frame and isn't now (its panel went, another
    /// buffer is shown there, the desk was put away): what it holds open
    /// while it's drawn, it lets go of.
    fn hidden(&mut self) {}

    /// The architecture changed on the server (an event).
    fn on_event(&mut self, _event: &ArchEvent) {}

    /// Everything read again (events may have been missed).
    fn invalidate(&mut self) {}

    /// Its own state that outlives a reload (saved with the layout).
    fn keep(&self) -> Option<serde_json::Value> {
        None
    }

    fn restore_kept(&mut self, _kept: &serde_json::Value) {}

    /// What it knows about component `id`.
    fn knows(&self, _id: &ComponentId) -> Option<Known> {
        None
    }

    /// Shown as a card over a panel (the quick look), or in a panel again.
    fn as_card(&mut self, _card: bool) {}

    /// Where the server says component `id`'s code is now (what it shows
    /// of it may be out of date).
    fn located(&mut self, _id: &ComponentId, _anchor: Option<&CodeAnchor>) {}

    /// What it shows, in words (the buffer list's lines).
    fn describe(&self) -> String;
}

/// Where a component's code is: its anchor's lines (a folder has none),
/// or the file it's planned in; none when it isn't in the code.
pub fn code_of(c: &arch_graph::Component) -> Option<(String, u32, u32)> {
    match (&c.anchor, &c.proposed) {
        (Some(a), _) if crate::status::is_folder(a) => None,
        (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
        (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
        (None, None) => None,
    }
}

/// A new buffer of `kind`.
pub fn new(kind: BufferKind) -> Box<dyn Buffer> {
    match kind {
        BufferKind::Chat => Box::<crate::chat::ChatStreamState>::default(),
        BufferKind::Code => Box::<crate::code::CodeViewerState>::default(),
        BufferKind::Graph => Box::<crate::graph::GraphState>::default(),
        BufferKind::Tree => Box::new(crate::tree::TreeBuffer::new(Box::<crate::tree::CodeOutline>::default())),
        BufferKind::Spec => Box::<crate::spec::SpecState>::default(),
        BufferKind::Queue => Box::<crate::queue::QueueState>::default(),
        BufferKind::Inbox => Box::<crate::inbox::InboxView>::default(),
        BufferKind::Changes => Box::<crate::changes::ChangesView>::default(),
        BufferKind::Terminal => Box::<crate::terminal::TerminalBuffer>::default(),
        BufferKind::Tags => Box::<crate::tag_list::TagList>::default(),
    }
}
