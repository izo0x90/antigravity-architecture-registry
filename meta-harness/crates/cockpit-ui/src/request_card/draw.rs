//! Drawing a request: by what's asked.

use egui::{Frame, RichText, Stroke};
use egui_phosphor::regular as icon;
use harness_contracts::{Question, RequestKind, RequestStatus, RuntimeRequest, ToolAction};

use super::drafts::{Drafts, value};
use crate::theme::colors;

/// What it says it is, in words, with its mark and colours.
fn heading(kind: RequestKind) -> (&'static str, &'static str, egui::Color32, egui::Color32) {
    let c = colors();
    match kind {
        RequestKind::Approval { action: ToolAction::Command } => (icon::TERMINAL_WINDOW, "Wants to run a command", c.warn, c.warn_bg),
        RequestKind::Approval { action: ToolAction::FileChange } => (icon::PENCIL_SIMPLE, "Wants to change a file", c.warn, c.warn_bg),
        RequestKind::Approval { action: ToolAction::FileRead } => (icon::FILE_TEXT, "Wants to read a file", c.warn, c.warn_bg),
        RequestKind::Approval { action: ToolAction::Tool } => (icon::WRENCH, "Wants to use a tool", c.warn, c.warn_bg),
        RequestKind::Question => (icon::CHAT_CIRCLE_DOTS, "Asks you", c.focus, c.info_bg),
        RequestKind::Link => (icon::ARROW_SQUARE_OUT, "Asks you to open a page", c.focus, c.info_bg),
    }
}

/// Draws what's asked (and a form's fields); its answers are its actions,
/// shown under it while the cursor is on it. Where it's drawn.
pub fn render_request(ui: &mut egui::Ui, request: &RuntimeRequest, drafts: &mut Drafts) -> egui::Rect {
    let look = crate::theme::look();
    let (glyph, what, color, fill) = heading(request.kind);
    Frame::NONE.fill(fill).stroke(Stroke::new(look.line_thin, color)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_m)).show(ui, |ui| {
        ui.label(RichText::new(format!("{glyph} {what}")).strong().size(look.text_small).color(color));
        let title = RichText::new(&request.title).size(look.text_label).color(colors().text_strong);
        ui.label(if request.kind == (RequestKind::Approval { action: ToolAction::Command }) { title.monospace() } else { title.strong() });
        // A page to open: its address (opening it is its control).
        if let Some(detail) = &request.detail {
            let link = detail.starts_with("https://") || detail.starts_with("http://");
            ui.label(if link { RichText::new(detail).monospace().size(look.text_body).color(colors().info) } else { RichText::new(detail).size(look.text_body).color(colors().muted) });
        }
        if request.status != RequestStatus::Pending {
            render_outcome(ui, request);
            return;
        }
        for question in &request.questions {
            render_question(ui, request, question, drafts);
        }
    })
    .response
    .rect
}

fn render_question(ui: &mut egui::Ui, request: &RuntimeRequest, question: &Question, drafts: &mut Drafts) {
    let look = crate::theme::look();
    ui.add_space(look.space_xs);
    let required = if question.required { " *" } else { "" };
    if question.header != question.question {
        ui.label(RichText::new(format!("{}{required}", question.header)).strong().size(look.text_body));
    }
    // A lone question is often the request's title too: said once.
    if question.question != request.title {
        ui.label(RichText::new(&question.question).size(look.text_body));
    }
    ui.horizontal_wrapped(|ui| {
        for option in &question.options {
            let mark = match (question.multi_select, drafts.picked(&request.id, &question.id, value(option))) {
                (true, true) => icon::CHECK_SQUARE,
                (true, false) => icon::SQUARE,
                (false, true) => icon::RADIO_BUTTON,
                (false, false) => icon::CIRCLE,
            };
            let picked = drafts.picked(&request.id, &question.id, value(option));
            let label = ui.selectable_label(picked, format!("{mark} {}", option.label));
            let label = match &option.description {
                Some(d) => label.on_hover_text(d),
                None => label,
            };
            if label.clicked() {
                drafts.pick(&request.id, question, value(option));
            }
        }
    });
    if question.allow_custom_answer {
        let hint = if question.options.is_empty() { "Your answer…" } else { "Or type your own…" };
        let typed = drafts.drafts.entry(request.id.clone()).or_default().typed.entry(question.id.clone()).or_default();
        ui.add(egui::TextEdit::singleline(typed).id(egui::Id::new(("question", request.id.as_str(), question.id.as_str()))).hint_text(hint).desired_width(f32::INFINITY));
    }
}

/// What was chosen, and answered.
fn render_outcome(ui: &mut egui::Ui, request: &RuntimeRequest) {
    let status = request.status;
    let look = crate::theme::look();
    let response = request.response.as_ref();
    let chosen = response.and_then(|r| r.option_id.as_ref()).and_then(|id| request.options.iter().find(|o| &o.id == id)).map(|o| o.label.clone());
    let text = match (status, chosen) {
        (RequestStatus::Resolved, Some(label)) => format!("{} {label}", icon::CHECK),
        (RequestStatus::Resolved, None) => format!("{} Answered", icon::CHECK),
        (RequestStatus::Cancelled, _) => "No longer asked".to_owned(),
        (other, _) => format!("{other:?}"),
    };
    ui.label(RichText::new(text).size(look.text_body).color(colors().ok));
    for question in &request.questions {
        if let Some(answer) = response.and_then(|r| r.answers.get(&question.id)) {
            ui.label(RichText::new(format!("{}: {}", question.header, answer.join(", "))).size(look.text_small).color(colors().muted));
        }
    }
}
