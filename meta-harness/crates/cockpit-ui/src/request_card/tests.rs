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

/// Clicking an option hands it back (to be sent as a key sends it).
#[test]
fn clicking_an_option_says_which() {
    let ctx = egui::Context::default();
    let request = request();
    let mut drafts = Drafts::default();
    let mut picked = None;
    let mut run = |events: Vec<egui::Event>| {
        let input = egui::RawInput { events, screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(option) = render_request(ui, &request, &mut drafts, &Shown::default()).clicked {
                    picked = Some(option);
                }
            });
        })
    };
    fn find(shape: &egui::Shape, label: &str) -> Option<egui::Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == label => Some(egui::Rect::from_min_size(t.pos, t.galley.size()).center()),
            egui::Shape::Vec(list) => list.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    let drawn = run(vec![]);
    let at = drawn.shapes.iter().find_map(|c| find(&c.shape, "Skip")).expect("the Skip button is drawn");
    let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    run(vec![egui::Event::PointerMoved(at)]);
    run(vec![button(true)]);
    run(vec![button(false)]);
    assert_eq!(picked.as_deref(), Some("decline"));
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
fn texts(request: &RuntimeRequest, hints: &[(String, String)], cursor: Option<&str>) -> (String, Vec<String>) {
    let ctx = egui::Context::default();
    let mut drafts = Drafts::default();
    let mut rows = Vec::new();
    let out = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            rows = render_request(ui, request, &mut drafts, &Shown { cursor, hints }).rows.into_iter().map(|(o, _)| o).collect();
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
    (text.join("\n"), rows)
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
fn a_question_reads_as_one_with_its_answers_and_their_keys_and_an_approval_as_what_it_would_do() {
    let question = asked(RequestKind::Question, &[("1", ApprovalDecision::Answer, "Redis"), ("2", ApprovalDecision::Answer, "Postgres")]);
    let hints = [("request.q.1".to_owned(), "SPC r 1".to_owned()), ("request.q.2".to_owned(), "SPC r 2".to_owned())];
    let (text, rows) = texts(&question, &hints, Some("2"));
    assert!(text.contains("Asks you") && !text.to_lowercase().contains("permission"), "{text}");
    assert!(text.contains("Redis   SPC r 1") && text.contains("Postgres   SPC r 2"), "each answer with its key: {text}");
    assert_eq!(rows, ["1", "2"], "where each answer is, for a cursor");
    let command = asked(RequestKind::Approval { action: harness_contracts::ToolAction::Command }, &[("allow", ApprovalDecision::Accept, "Allow"), ("deny", ApprovalDecision::Decline, "Deny")]);
    let (text, _) = texts(&command, &[], None);
    assert!(text.contains("Wants to run a command") && text.contains("Allow") && text.contains("Deny"), "{text}");
}
