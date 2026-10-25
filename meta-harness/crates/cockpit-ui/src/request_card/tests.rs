//! The card: what it hands back, the answers' keys, the drafts.

use harness_contracts::{ApprovalDecision, ApprovalOption, Question, QuestionOption, RequestId, RequestKind, RequestStatus, RuntimeRequest, ThreadId};

use super::*;

fn request() -> RuntimeRequest {
    let option = |label: &str| QuestionOption { label: label.into(), description: None, value: None };
    let question = |id: &str, multi: bool, required: bool| Question { id: id.into(), header: id.into(), question: format!("{id}?"), options: vec![option("a"), option("b")], multi_select: multi, allow_custom_answer: true, required };
    RuntimeRequest {
        id: RequestId::new("r"),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Question,
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
fn an_answers_key_is_its_kinds_only_when_its_the_only_one_of_its_kind() {
    let mut asked = request();
    let names = |r: &RuntimeRequest| answers(r).into_iter().map(|a| (a.option, a.alias)).collect::<Vec<_>>();
    assert_eq!(names(&asked), [("submit".to_owned(), Some("request.accept".to_owned())), ("decline".to_owned(), Some("request.decline".to_owned()))]);
    asked.options.push(ApprovalOption { id: "other".into(), decision: ApprovalDecision::Accept, label: "Other answer".into(), warning: None });
    assert_eq!(names(&asked), [("submit".to_owned(), None), ("decline".to_owned(), Some("request.decline".to_owned())), ("other".to_owned(), None)], "two answers that accept: numbered, not both y");
    asked.status = RequestStatus::Resolved;
    assert!(answers(&asked).is_empty(), "answered: nothing to offer");
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

/// Everything drawn, as text.
fn texts(request: &RuntimeRequest) -> String {
    let ctx = egui::Context::default();
    let mut drafts = Drafts::default();
    let out = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            render_request(ui, request, &mut drafts);
        });
    });
    fn all(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(t) => out.push(t.galley.text().to_owned()),
            egui::Shape::Vec(list) => list.iter().for_each(|s| all(s, out)),
            _ => {}
        }
    }
    let mut text = Vec::new();
    out.shapes.iter().for_each(|c| all(&c.shape, &mut text));
    text.join("\n")
}

fn asked(kind: RequestKind, options: &[(&str, ApprovalDecision, &str)]) -> RuntimeRequest {
    RuntimeRequest {
        id: RequestId::new("q"),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind,
        status: RequestStatus::Pending,
        title: "Which store?".into(),
        detail: None,
        options: options.iter().map(|(id, decision, label)| ApprovalOption { id: (*id).into(), decision: *decision, label: (*label).into(), warning: None }).collect(),
        questions: Vec::new(),
        response: None,
    }
}

#[test]
fn a_question_reads_as_one_its_answers_a_list_under_it_and_an_approval_as_what_it_would_do() {
    use crate::actions::{Action, At, Style, Tone};
    let question = asked(RequestKind::Question, &[("1", ApprovalDecision::Answer, "Redis"), ("2", ApprovalDecision::Answer, "Postgres")]);
    let text = texts(&question);
    assert!(text.contains("Asks you") && !text.to_lowercase().contains("permission"), "{text}");
    assert!(!text.contains("Redis"), "its answers aren't the card's: they're its actions: {text}");
    let shown: Vec<(String, Style, Tone)> = answers(&question).into_iter().map(|a| (a.caption, a.style, a.tone)).collect();
    assert_eq!(shown, [("Redis".to_owned(), Style::Choice, Tone::Plain), ("Postgres".to_owned(), Style::Choice, Tone::Plain)], "a list to pick from");
    let answer = answers(&question).remove(0);
    assert_eq!(answer.place(Action::new(answer.id.clone(), answer.label.clone(), "Answers", ()), false).at(), None, "away from it: keys and the palette only");
    assert_eq!(answer.place(Action::new(answer.id.clone(), answer.label.clone(), "Answers", ()), true).at(), Some(At::Waiting), "on it: under it, typing or not");
    let command = asked(RequestKind::Approval { action: harness_contracts::ToolAction::Command }, &[("allow", ApprovalDecision::Accept, "Allow"), ("deny", ApprovalDecision::Decline, "Deny")]);
    assert!(texts(&command).contains("Wants to run a command"));
    let shown: Vec<(String, Style, Tone)> = answers(&command).into_iter().map(|a| (a.caption, a.style, a.tone)).collect();
    assert_eq!(shown, [("Allow".to_owned(), Style::Button, Tone::Ok), ("Deny".to_owned(), Style::Button, Tone::Error)]);
}
