//! What a view tells its host about itself, and the header the host draws
//! from it. A view draws its content; its top line (a title, what it's
//! doing, and the controls for the whole view) is the host's, the same for
//! every view: the title, then each status, then the view's header buttons
//! (one line that wraps), and its settings on a row under them.

use egui::RichText;

use crate::actions::{Action, At, Control, RunCx, Style};

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
pub trait Panel {
    /// Its name on the top line.
    fn title(&self) -> Status;

    /// What it's doing, after the title.
    fn status(&self) -> Vec<Status> {
        Vec::new()
    }

    /// Actions it shows (its own or the shell's, by id), with the control
    /// it gives them here: the graph's back and forward on its top line, a
    /// card's go-tos under it.
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

/// The panel's top line: the title, its status, the header controls of
/// `actions` (buttons on the right; settings on a row under).
pub fn header(ui: &mut egui::Ui, panel: &dyn Panel, actions: &[Action], cx: &mut RunCx<'_>) {
    let look = crate::theme::look();
    let at_header = |style: Style| actions.iter().filter(move |a| a.control.as_ref().is_some_and(|c| c.at == At::Header && c.style == style));
    let said = |ui: &mut egui::Ui, status: &Status, strong: bool| {
        let mut text = RichText::new(&status.text).size(if strong { look.text_label } else { look.text_small }).color(status.tone.color());
        if strong {
            text = text.strong();
        }
        let label = ui.add(egui::Label::new(text).extend());
        if let Some(detail) = &status.detail {
            label.on_hover_text(detail);
        }
    };
    // One line that wraps (a narrow column, a phone): nothing overlaps.
    ui.horizontal_wrapped(|ui| {
        said(ui, &panel.title(), true);
        for status in panel.status() {
            said(ui, &status, false);
        }
        crate::actions::controls(ui, at_header(Style::Button), cx);
    });
    if at_header(Style::Setting).next().is_some() {
        ui.horizontal_wrapped(|ui| crate::actions::controls(ui, at_header(Style::Setting), cx));
    }
}

/// A panel's actions as it shows them: its own (`own`), with the controls
/// it places, and the shell's it places too.
pub fn placed(panel: &dyn Panel, mut own: Vec<Action>, shell: &[Action]) -> Vec<Action> {
    for (id, control) in panel.placed() {
        match own.iter_mut().find(|a| a.id == id) {
            Some(a) => a.control = Some(control),
            None => {
                if let Some(a) = shell.iter().find(|a| a.id == id) {
                    let mut a = a.clone();
                    a.control = Some(control);
                    own.push(a);
                }
            }
        }
    }
    own
}
