//! What a view tells its host about itself, and the header the host draws
//! from it. A view draws its content; its top line (a title, what it's
//! doing, and the controls for the whole view) is the host's, the same for
//! every view: the title, then each status, then the view's header controls
//! (buttons on the right, settings on a row under them).

use egui::RichText;

use crate::actions::{Action, At, Control, RunCx, Style};
use crate::theme::colors;

/// How a status reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Muted,
    Accent,
    Info,
    Ok,
    Warn,
    Error,
}

impl Tone {
    fn color(self) -> egui::Color32 {
        match self {
            Self::Plain => colors().text,
            Self::Muted => colors().muted,
            Self::Accent => colors().accent,
            Self::Info => colors().info,
            Self::Ok => colors().ok,
            Self::Warn => colors().warn,
            Self::Error => colors().error,
        }
    }
}

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

    /// The shell's actions it shows on its top line too (by id), with their
    /// controls: the graph's back and forward, its re-read.
    fn shell_controls(&self) -> Vec<(&'static str, Control)> {
        Vec::new()
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
    ui.horizontal(|ui| {
        said(ui, &panel.title(), true);
        for status in panel.status() {
            said(ui, &status, false);
        }
        // On the right, in the order listed (drawn right to left).
        let buttons: Vec<&Action> = at_header(Style::Button).collect();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| crate::actions::controls(ui, buttons.into_iter().rev(), cx));
    });
    if at_header(Style::Setting).next().is_some() {
        ui.horizontal_wrapped(|ui| crate::actions::controls(ui, at_header(Style::Setting), cx));
    }
}

/// The shell actions a panel shows, with the controls it gives them.
pub fn borrowed(panel: &dyn Panel, shell: &[Action]) -> Vec<Action> {
    panel
        .shell_controls()
        .into_iter()
        .filter_map(|(id, control)| {
            shell.iter().find(|a| a.id == id).map(|a| {
                let mut a = a.clone();
                a.control = Some(control);
                a
            })
        })
        .collect()
}
