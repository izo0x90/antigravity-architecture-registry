//! A request the agent waits on, drawn in the chat: a permission to give
//! (its options), or questions to answer, each with its choices and, when
//! it takes one, a typed answer of its own. What's picked and typed is kept
//! here until it's sent.

use std::collections::{BTreeMap, HashMap};

use egui::{Frame, RichText, Stroke};
use egui_phosphor::regular as icon;
use harness_contracts::{ApprovalDecision, Question, RequestId, RequestKind, RequestResponse, RequestStatus, RuntimeRequest, ThreadId};

use crate::net::NetClient;
use crate::theme::colors;

/// What's picked and typed for each request's questions, by question.
#[derive(Default)]
pub struct Drafts {
    drafts: HashMap<RequestId, Draft>,
}

#[derive(Default)]
struct Draft {
    picks: BTreeMap<String, Vec<String>>,
    typed: BTreeMap<String, String>,
}

/// The value an option answers with.
fn value(option: &harness_contracts::QuestionOption) -> &str {
    option.value.as_deref().unwrap_or(&option.label)
}

impl Drafts {
    /// The answers to `request`'s questions so far: the picks, then what
    /// was typed.
    pub fn answers(&self, request: &RuntimeRequest) -> BTreeMap<String, Vec<String>> {
        let Some(draft) = self.drafts.get(&request.id) else { return BTreeMap::new() };
        request
            .questions
            .iter()
            .filter_map(|q| {
                let mut answer = draft.picks.get(&q.id).cloned().unwrap_or_default();
                answer.extend(draft.typed.get(&q.id).map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()));
                (!answer.is_empty()).then(|| (q.id.clone(), answer))
            })
            .collect()
    }

    /// Every question that must be answered is.
    pub fn complete(&self, request: &RuntimeRequest) -> bool {
        let answers = self.answers(request);
        request.questions.iter().all(|q| !q.required || answers.contains_key(&q.id))
    }

    /// The response choosing `option`: with the answers when it accepts.
    pub fn response(&self, request: &RuntimeRequest, option: &str) -> RequestResponse {
        let accepts = request.options.iter().any(|o| o.id == option && matches!(o.decision, ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways));
        RequestResponse { option_id: Some(option.to_owned()), answers: if accepts { self.answers(request) } else { BTreeMap::new() } }
    }

    fn pick(&mut self, request: &RequestId, question: &Question, value: &str) {
        let picks = self.drafts.entry(request.clone()).or_default().picks.entry(question.id.clone()).or_default();
        match picks.iter().position(|p| p == value) {
            Some(at) => {
                picks.remove(at);
            }
            None if question.multi_select => picks.push(value.to_owned()),
            None => *picks = vec![value.to_owned()],
        }
    }

    fn picked(&self, request: &RequestId, question: &str, value: &str) -> bool {
        self.drafts.get(request).and_then(|d| d.picks.get(question)).is_some_and(|p| p.iter().any(|v| v == value))
    }
}

fn heading(kind: RequestKind) -> String {
    match kind {
        RequestKind::UserInput => format!("{} THE AGENT ASKS", icon::CHAT_CIRCLE_DOTS),
        RequestKind::McpElicitation => format!("{} THE AGENT NEEDS YOU", icon::ARROW_SQUARE_OUT),
        _ => format!("{} PERMISSION REQUIRED", icon::WARNING),
    }
}

pub fn render_request(ui: &mut egui::Ui, net: &NetClient, thread: &ThreadId, request: &RuntimeRequest, drafts: &mut Drafts) {
    let look = crate::theme::look();
    Frame::NONE
        .fill(colors().warn_bg)
        .stroke(Stroke::new(look.line_thin, colors().warn))
        .corner_radius(look.radius_m)
        .inner_margin(crate::theme::margin(look.space_m))
        .show(ui, |ui| {
            ui.label(RichText::new(heading(request.kind)).strong().size(look.text_body).color(colors().warn));
            ui.label(RichText::new(&request.title).size(look.text_label));
            if let Some(detail) = &request.detail {
                if detail.starts_with("https://") || detail.starts_with("http://") {
                    ui.hyperlink_to(RichText::new(detail).size(look.text_body), detail);
                } else {
                    ui.label(RichText::new(detail).size(look.text_body).color(colors().muted));
                }
            }
            match request.status {
                RequestStatus::Pending => {
                    for question in &request.questions {
                        render_question(ui, request, question, drafts);
                    }
                    let complete = drafts.complete(request);
                    ui.horizontal_wrapped(|ui| {
                        for option in &request.options {
                            let accepts = matches!(option.decision, ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways);
                            let color = if accepts { colors().ok } else { colors().error };
                            let enabled = !accepts || complete;
                            let button = ui.add_enabled(enabled, egui::Button::new(RichText::new(&option.label).color(color)));
                            let button = match &option.warning {
                                Some(w) => button.on_hover_text(w),
                                None if !enabled => button.on_disabled_hover_text("Answer the questions it needs first."),
                                None => button,
                            };
                            if button.clicked() {
                                net.respond(thread, &request.id, &drafts.response(request, &option.id), ui.ctx().clone(), |_| {});
                            }
                        }
                    });
                }
                status => render_outcome(ui, request, status),
            }
        });
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
fn render_outcome(ui: &mut egui::Ui, request: &RuntimeRequest, status: RequestStatus) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ApprovalOption, QuestionOption};

    fn request() -> RuntimeRequest {
        let option = |label: &str| QuestionOption { label: label.into(), description: None, value: None };
        let question = |id: &str, multi: bool, required: bool| Question { id: id.into(), header: id.into(), question: format!("{id}?"), options: vec![option("a"), option("b")], multi_select: multi, allow_custom_answer: true, required };
        RuntimeRequest {
            id: RequestId::new("r"),
            thread_id: ThreadId::new("t"),
            turn_id: None,
            item_id: None,
            kind: RequestKind::UserInput,
            status: RequestStatus::Pending,
            title: "Questions".into(),
            detail: None,
            options: vec![
                ApprovalOption { id: "submit".into(), decision: ApprovalDecision::Accept, label: "Answer".into(), warning: None },
                ApprovalOption { id: "decline".into(), decision: ApprovalDecision::Decline, label: "Skip".into(), warning: None },
            ],
            questions: vec![question("one", false, true), question("many", true, false)],
            response: None,
        }
    }

    #[test]
    fn picks_and_typed_answers_make_the_response_and_skipping_sends_none() {
        let request = request();
        let mut drafts = Drafts::default();
        assert!(!drafts.complete(&request), "the required one isn't answered");
        drafts.pick(&request.id, &request.questions[0], "a");
        drafts.pick(&request.id, &request.questions[0], "b");
        drafts.pick(&request.id, &request.questions[1], "a");
        drafts.pick(&request.id, &request.questions[1], "b");
        drafts.pick(&request.id, &request.questions[1], "a");
        drafts.drafts.get_mut(&request.id).unwrap().typed.insert("many".into(), " c ".into());
        assert!(drafts.complete(&request));
        let answers = drafts.response(&request, "submit").answers;
        assert_eq!(answers.get("one"), Some(&vec!["b".to_owned()]), "one pick: the last");
        assert_eq!(answers.get("many"), Some(&vec!["b".to_owned(), "c".to_owned()]), "picked again: unpicked; typed: added");
        assert!(drafts.response(&request, "decline").answers.is_empty());
    }
}
