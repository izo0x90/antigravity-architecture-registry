//! Drawing a request: by what's asked.

use egui::{Frame, RichText, Stroke};
use egui_phosphor::regular as icon;
use harness_contracts::{ApprovalDecision, ApprovalOption, Question, RequestKind, RequestStatus, RuntimeRequest, ToolAction};

use super::drafts::{Drafts, value};
use crate::theme::colors;

/// How it's shown: the answer the cursor is on (its option), and each
/// action's key as bound now (an answer's action: `request.<id>.<option>`).
#[derive(Default)]
pub struct Shown<'a> {
    pub cursor: Option<&'a str>,
    pub hints: &'a [(String, String)],
}

/// What drawing it gave: the answer clicked, and where each answer is.
#[derive(Default)]
pub struct Drawn {
    pub clicked: Option<String>,
    pub rows: Vec<(String, egui::Rect)>,
}

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

fn says_yes(option: &ApprovalOption) -> bool {
    matches!(option.decision, ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways)
}

pub fn render_request(ui: &mut egui::Ui, request: &RuntimeRequest, drafts: &mut Drafts, shown: &Shown) -> Drawn {
    let look = crate::theme::look();
    let (glyph, what, color, fill) = heading(request.kind);
    let mut drawn = Drawn::default();
    Frame::NONE.fill(fill).stroke(Stroke::new(look.line_thin, color)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_m)).show(ui, |ui| {
        ui.label(RichText::new(format!("{glyph} {what}")).strong().size(look.text_small).color(color));
        let title = RichText::new(&request.title).size(look.text_label).color(colors().text_strong);
        ui.label(if request.kind == (RequestKind::Approval { action: ToolAction::Command }) { title.monospace() } else { title.strong() });
        if let Some(detail) = &request.detail {
            if detail.starts_with("https://") || detail.starts_with("http://") {
                ui.hyperlink_to(RichText::new(detail).size(look.text_body), detail);
            } else {
                ui.label(RichText::new(detail).size(look.text_body).color(colors().muted));
            }
        }
        if request.status != RequestStatus::Pending {
            render_outcome(ui, request);
            return;
        }
        for question in &request.questions {
            render_question(ui, request, question, drafts);
        }
        // A question's answers are a list to pick from; anything else
        // (allow or deny, a form's submit or skip, done) a row of buttons.
        let list = request.kind == RequestKind::Question && request.questions.is_empty();
        let complete = drafts.complete(request);
        let key = |option: &ApprovalOption| {
            let id = format!("request.{}.{}", request.id, option.id);
            crate::actions::key_of(shown.hints, &id).map(str::to_owned)
        };
        let mut row = |ui: &mut egui::Ui, option: &ApprovalOption| {
            let enabled = !says_yes(option) || complete;
            let tint = match option.decision {
                ApprovalDecision::Decline | ApprovalDecision::Cancel => colors().error,
                ApprovalDecision::Answer => colors().text,
                _ => colors().ok,
            };
            let text = match key(option) {
                Some(k) => format!("{}   {k}", option.label),
                None => option.label.clone(),
            };
            let on = shown.cursor == Some(option.id.as_str());
            let response = if list {
                ui.add_enabled(enabled, egui::Button::selectable(on, RichText::new(format!("{} {text}", icon::CARET_RIGHT)).size(look.text_body).color(tint)).min_size(egui::vec2(ui.available_width(), 0.0)))
            } else {
                let button = egui::Button::new(RichText::new(text).size(look.text_body).color(tint)).selected(on);
                ui.add_enabled(enabled, button)
            };
            let response = match &option.warning {
                Some(w) => response.on_hover_text(w),
                None if !enabled => response.on_disabled_hover_text("Answer the questions it needs first."),
                None => response,
            };
            if on {
                ui.painter().rect_stroke(response.rect.expand(2.0), look.radius_m, Stroke::new(look.line_thin, colors().focus), egui::StrokeKind::Outside);
            }
            if response.clicked() {
                drawn.clicked = Some(option.id.clone());
            }
            drawn.rows.push((option.id.clone(), response.rect));
        };
        if list {
            ui.vertical(|ui| request.options.iter().for_each(|o| row(ui, o)));
        } else {
            ui.horizontal_wrapped(|ui| request.options.iter().for_each(|o| row(ui, o)));
        }
    });
    drawn
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
