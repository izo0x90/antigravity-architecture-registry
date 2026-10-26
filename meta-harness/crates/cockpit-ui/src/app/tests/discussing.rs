//! Discussions in the cockpit: one started on what the cursor's on (the
//! shell's `C`, as anywhere: what it is, then what it says), opened over
//! what you're on once the server has it, and replied to, settled and
//! reopened there; what's sent is what the server's routes take.

use super::*;

fn told_discussion(app: &CockpitApp, d: harness_contracts::Discussion) {
    told(app, harness_contracts::ClientEvent::DiscussionUpdated { discussion: d });
}

#[test]
fn a_discussion_is_started_on_what_youre_on_opened_and_replied_to_settled_and_reopened_there() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Spec);
    let step = ComponentId::new("rust:a::step");
    app.desk.spec_mut().show(step.clone());
    app.desk.spec_mut().detail = Some(crate::spec::tests::detail(&step, &ComponentId::new("rust:a::save")));
    let _ = bodies(&app, "");
    // C on the logic section: what it is, then what it says.
    app.desk.spec_mut().move_to(&crate::spec::Entry { section: crate::card::Section::Logic, item: crate::spec::SpecItem::Header }.key());
    keys(&ctx, &mut app, "C");
    assert_eq!(app.palette.picking(), Some("discuss.start"));
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("Discuss plan p component rust:a::step section logic"), "what it's on, said at the top: {drawn}");
    keys(&ctx, &mut app, "change");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("discuss.write"), "then what it says");
    keys(&ctx, &mut app, "Split step 2.");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let logic = harness_contracts::Subject::PlanSection { plan: PlanId::new("p").unwrap(), id: step.clone(), section: "logic".into() };
    assert_eq!(
        bodies(&app, "POST /api/discussions"),
        [serde_json::json!({ "subject": logic, "kind": "change_request", "text": "Split step 2." })]
    );
    // The server has it: it opens over what you're on.
    let mut d = crate::review::tests::discussion(1, logic, harness_contracts::DiscussionStatus::Open);
    d.kind = harness_contracts::DiscussionKind::ChangeRequest;
    d.said[0].text = "Split step 2.".into();
    told_discussion(&app, d.clone());
    assert!(app.desk.net.answer("POST /api/discussions", 200, &serde_json::to_value(&d).unwrap()));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.quick_look.as_ref().map(|q| q.kind()), Some(BufferKind::Discussion), "opened in the quick look");
    // What it's on, as it is now, read from the server.
    let now = harness_contracts::OnNow {
        what: "plan p component rust:a::step section logic".into(),
        held: harness_contracts::Held::Source,
        text: Some("Read the input.\nSave it.".into()),
        lines: 2,
    };
    assert!(app.desk.net.answer("GET /api/discussions/d1/on", 200, &serde_json::to_value(&now).unwrap()), "{:?}", sent(&app));
    frame(&ctx, &mut app, vec![]);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(
        drawn.contains("OPEN") && drawn.contains("Split step 2.") && drawn.contains("plan p component rust:a::step section logic"),
        "the one just sent: {drawn}"
    );
    assert!(drawn.contains("as it is now") && drawn.contains("Read the input.\nSave it."), "{drawn}");
    // Replied to there (r, writing, Enter).
    keys(&ctx, &mut app, "r");
    keys(&ctx, &mut app, "And the third?");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/discussions/d1/reply"), [serde_json::json!({ "text": "And the third?" })]);
    // Answered: settled (a), then reopened with why (x).
    let mut answered = d.clone();
    answered.status = harness_contracts::DiscussionStatus::Answered;
    answered.said.push(harness_contracts::Said {
        by: "lead".into(),
        at_ms: 9,
        kind: harness_contracts::SaidKind::Answer,
        text: "Split.".into(),
        links: Vec::new(),
        changed: Vec::new(),
    });
    told_discussion(&app, answered);
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "a");
    assert_eq!(bodies(&app, "POST /api/discussions/d1/settle"), [serde_json::json!({})]);
    keys(&ctx, &mut app, "x");
    keys(&ctx, &mut app, "Not yet.");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/discussions/d1/reopen"), [serde_json::json!({ "text": "Not yet." })]);
    // Not sent (the server refused): what was written comes back to its box.
    assert!(app.desk.net.answer("POST /api/discussions/d1/reopen", 400, &serde_json::json!({ "error": "d1 is open already" })));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    let view = app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion);
    assert_eq!((view.writing.text.as_str(), view.purpose), ("Not yet.", crate::discussion::Writing::Reopen), "kept to send again");
    // A question made a change request: open, at once; answered, why first.
    let mut question = d.clone();
    (question.kind, question.status) = (harness_contracts::DiscussionKind::Question, harness_contracts::DiscussionStatus::Open);
    told_discussion(&app, question.clone());
    frame(&ctx, &mut app, vec![]);
    app.desk.typed_mut::<crate::discussion::DiscussionView>(BufferKind::Discussion).writing = Default::default();
    keys(&ctx, &mut app, "e");
    assert_eq!(bodies(&app, "POST /api/discussions/d1/change"), [serde_json::json!({})], "open: at once");
    question.status = harness_contracts::DiscussionStatus::Answered;
    told_discussion(&app, question);
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "e");
    keys(&ctx, &mut app, "Name the step.");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    assert_eq!(bodies(&app, "POST /api/discussions/d1/change"), [serde_json::json!({ "text": "Name the step." })], "answered: why first");
    // `#` in its box: the tag picker, the name put after it.
    told(
        &app,
        harness_contracts::ClientEvent::TagUpdated {
            tag: harness_contracts::Tag {
                id: harness_contracts::TagId::new("x1"),
                name: "flaky".into(),
                colour: 2,
                note: String::new(),
                noted_by: String::new(),
                on: Vec::new(),
            },
        },
    );
    keys(&ctx, &mut app, "r");
    keys(&ctx, &mut app, "see #");
    frame(&ctx, &mut app, vec![]);
    assert!(app.palette.is_open(), "the tag picker");
    keys(&ctx, &mut app, "fla");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(
        app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion).writing.text,
        "see #flaky ",
        "the name after the #"
    );
}

/// The glyph `text` drawn, and its colour.
fn glyphs(output: &egui::FullOutput, text: &str) -> Vec<(egui::Rect, egui::Color32)> {
    fn collect(shape: &egui::Shape, text: &str, out: &mut Vec<(egui::Rect, egui::Color32)>) {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == text => {
                out.push((egui::Rect::from_min_size(t.pos, t.galley.size()), t.galley.job.sections[0].format.color))
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, text, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, text, &mut out);
    }
    out
}

/// A thing with discussions is marked in the gutter (G.4): one speech
/// bubble, in the colour of whose move it is (yours first); the panel's
/// top line says them in a few words; a long press says each; clicked,
/// which is asked (the one waiting on you first), and it opens.
#[test]
fn a_thing_with_discussions_is_marked_in_the_colour_of_whose_move_said_on_the_top_line_and_opened_by_a_click() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![super::terminal::with_commands(1000, vec![super::terminal::command(1, 1003, Some(1008), "make")])]);
    let on = harness_contracts::Subject::TerminalCommand {
        terminal: harness_contracts::TerminalId::new("t1"),
        shell: 1,
        command: 1,
        typed: String::new(),
    };
    told_discussion(&app, crate::discussions::tests::discussion(1, on.clone(), harness_contracts::DiscussionStatus::Open));
    told_discussion(&app, crate::discussions::tests::discussion(2, on, harness_contracts::DiscussionStatus::Answered));
    super::terminal::not_typing(&ctx, &mut app);
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let theme = crate::theme::colors();
    let line = drawn_rects(&output, "line 1003")[0];
    let marks: Vec<_> = glyphs(&output, egui_phosphor::fill::CHAT_CIRCLE).into_iter().filter(|(r, _)| r.right() <= line.left()).collect();
    assert_eq!(marks.len(), 1, "one mark for the two: {marks:?}");
    let (mark, colour) = marks[0];
    assert!(mark.y_range().contains(line.center().y) && colour == theme.warn, "at the command's top, yours: {marks:?}");
    // On the command: the top line says them.
    app.desk.terminal_mut().move_to("1003");
    frame_at(&ctx, &mut app, vec![], 60.0);
    let output = frame_at(&ctx, &mut app, vec![], 69.0);
    let drawn = texts(&output);
    assert!(drawn.contains("2 discussions, 1 waiting on you"), "{drawn}");
    let line = drawn_rects(&output, "line 1003")[0];
    let (mark, _) = glyphs(&output, egui_phosphor::fill::CHAT_CIRCLE)
        .into_iter()
        .find(|(r, _)| r.right() <= line.left())
        .expect("the mark, where the command now is");
    // A long press says each.
    let touch =
        |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: mark.center(), force: None };
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start)], 70.0);
    frame_at(&ctx, &mut app, vec![], 70.8);
    frame_at(&ctx, &mut app, vec![touch(egui::TouchPhase::End)], 70.9);
    let output = frame_at(&ctx, &mut app, vec![], 71.0);
    assert_eq!(app.note.as_deref(), Some("2 discussions, 1 waiting on you\nd2 question: point 2\nd1 question: point 1"));
    // (The note, said under the panels, moved them.)
    let line = drawn_rects(&output, "line 1003")[0];
    let (mark, _) =
        glyphs(&output, egui_phosphor::fill::CHAT_CIRCLE).into_iter().find(|(r, _)| r.right() <= line.left()).expect("the mark");
    // Clicked: the one waiting on you opens.
    let button = |pressed| egui::Event::PointerButton {
        pos: mark.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut t = 72.0;
    for events in [vec![egui::Event::PointerMoved(mark.center())], vec![button(true)], vec![button(false)], vec![], vec![]] {
        t += 0.05;
        frame_at(&ctx, &mut app, events, t);
    }
    // Two on it: which, first (the one waiting on you at the top).
    assert_eq!(app.palette.picking(), Some("discuss.open"));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.quick_look.as_ref().map(|q| q.kind()), Some(BufferKind::Discussion));
    assert_eq!(app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion).id.as_ref().map(|d| d.as_str()), Some("d2"));
}

/// A graph's cards aren't rows: a card's notes are on its top right
/// corner (a dot per tag, the speech bubble), never in a gutter (#14).
#[test]
fn a_cards_notes_are_on_its_corner() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Graph);
    let step = harness_contracts::Subject::Component { id: ComponentId::new("rust:a::step"), was: None };
    let mut tag = harness_contracts::Tag {
        id: harness_contracts::TagId::new("x1"),
        name: "seen".into(),
        colour: 2,
        note: String::new(),
        noted_by: String::new(),
        on: Vec::new(),
    };
    tag.on.push(harness_contracts::Tagged { subject: step.clone(), what: "rust:a::step".into(), at_ms: 0, by: "user".into(), kept: None });
    told(&app, harness_contracts::ClientEvent::TagUpdated { tag });
    // The graph shows plan p: its cards are the plan's components (what approval waits on).
    let planned = harness_contracts::Subject::PlanComponent { plan: PlanId::new("p").unwrap(), id: ComponentId::new("rust:a::step") };
    told_discussion(&app, crate::discussions::tests::discussion(1, planned.clone(), harness_contracts::DiscussionStatus::Open));
    let _ = step;
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let card = app.desk.graph().to_screen(&app.desk.graph().card("rust:a::step").expect("the card").rect);
    let dots: Vec<_> = bars(&output, crate::theme::colors().tag(2)).into_iter().filter(|d| card.x_range().contains(d.center().x)).collect();
    assert!(
        dots.len() == 1 && card.x_range().contains(dots[0].center().x) && (dots[0].center().y - card.top()).abs() < 1.0,
        "on its top edge: {dots:?} {card:?}"
    );
    let marks = glyphs(&output, egui_phosphor::fill::CHAT_CIRCLE);
    assert!(
        marks.iter().any(|(r, c)| card.x_range().contains(r.center().x)
            && (r.center().y - card.top()).abs() < 2.0
            && *c == crate::theme::colors().info),
        "the agent's move: {marks:?} {card:?}"
    );
    // What's discussed from a card is the plan's component.
    app.desk.graph_mut().select(Some("rust:a::step".into()));
    let things = app.desk.view(BufferKind::Graph).things("rust:a::step");
    assert_eq!(things.at.iter().map(|t| t.subject.clone()).collect::<Vec<_>>(), [planned]);
}

/// Started from wherever you are (G.7): a code line, a terminal command, a
/// chat message; each on the thing the cursor's on, by its one name.
#[test]
fn one_is_started_from_a_code_line_a_terminal_command_and_a_chat_message() {
    let (ctx, mut app) = full_app();
    let start = |ctx: &egui::Context, app: &mut CockpitApp, on: harness_contracts::Subject, known: harness_contracts::Known| {
        keys(ctx, app, "C");
        let drawn = texts(&frame(ctx, app, vec![]));
        assert!(
            drawn.contains(&format!("Discuss {}", on.name(&known).split(" (").next().unwrap_or_default())),
            "what it's on, at the top: {drawn}"
        );
        keys(ctx, app, "question");
        frame(ctx, app, press(egui::Key::Enter, false));
        frame(ctx, app, vec![]);
        keys(ctx, app, "Why?");
        bodies(app, "");
        frame(ctx, app, press(egui::Key::Enter, false));
        assert_eq!(bodies(app, "POST /api/discussions"), [serde_json::json!({ "subject": on, "kind": "question", "text": "Why?" })]);
    };
    // A code line.
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code_mut().move_to("1");
    let file = app.desk.code().current_file.clone();
    start(
        &ctx,
        &mut app,
        harness_contracts::Subject::CodeLines { path: file.clone(), first: 1, last: 1, snapshot: String::new() },
        Default::default(),
    );
    // Tab while starting: the next of the things there (the whole file).
    keys(&ctx, &mut app, "C");
    frame(&ctx, &mut app, press(egui::Key::Tab, false));
    frame(&ctx, &mut app, vec![]);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains(&format!("Discuss file {file}")), "{drawn}");
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    // A terminal command (on its prompt line: the command is the unit).
    focus(&ctx, &mut app, BufferKind::Terminal);
    app.desk.terminal_mut().arrive(vec![super::terminal::with_commands(1000, vec![super::terminal::command(1, 1003, Some(1008), "make")])]);
    super::terminal::not_typing(&ctx, &mut app);
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1005)), None, &ctx);
    start(
        &ctx,
        &mut app,
        harness_contracts::Subject::TerminalCommand {
            terminal: harness_contracts::TerminalId::new("t1"),
            shell: 1,
            command: 1,
            typed: "make".into(),
        },
        Default::default(),
    );
    // A chat message.
    for event in super::tags::named_conversation() {
        told(&app, event);
    }
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, vec![]);
    let answer = app
        .desk
        .view(BufferKind::Chat)
        .items()
        .into_iter()
        .rev()
        .find(|i| i.text.contains("Looking at it."))
        .map(|i| i.key)
        .expect("the answer");
    app.desk.view_mut(BufferKind::Chat).move_to(&answer);
    let things = app.desk.view(BufferKind::Chat).things(&answer);
    let message = things.picked(None).map(|t| t.subject.clone()).expect("the message");
    assert!(matches!(message, harness_contracts::Subject::ChatMessage { .. }));
    let known = harness_contracts::Known {
        chat: harness_contracts::chat_who(Some("lead"), &harness_contracts::NativeId::new("n1")),
        ..Default::default()
    };
    start(&ctx, &mut app, message, known);
}

/// One waiting on you is in the inbox (G.5): Enter there opens it.
#[test]
fn one_waiting_on_you_is_in_the_inbox_and_enter_opens_it() {
    let (ctx, mut app) = full_app();
    let on = harness_contracts::Subject::Component { id: ComponentId::new("rust:a::step"), was: None };
    told_discussion(&app, crate::discussions::tests::discussion(3, on, harness_contracts::DiscussionStatus::Answered));
    let kind = harness_contracts::NotificationKind::Discussion;
    let notification = harness_contracts::Notification {
        id: harness_contracts::NotificationId::discussion(&harness_contracts::DiscussionId::new("d3"), 2),
        server: None,
        level: kind.level(),
        kind,
        slot: None,
        thread: None,
        request: None,
        terminal: None,
        discussion: Some(harness_contracts::DiscussionId::new("d3")),
        title: "d3 answered".into(),
        detail: None,
        at_ms: 1,
        state: harness_contracts::NotificationState::Open,
    };
    told(&app, harness_contracts::ClientEvent::NotificationUpdated { notification });
    focus(&ctx, &mut app, BufferKind::Inbox);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    let open = app.desk.typed::<crate::discussion::DiscussionView>(BufferKind::Discussion).id.clone();
    assert_eq!(open.as_ref().map(|d| d.as_str()), Some("d3"));
}

/// Every text drawn inside `within`, where its rows are.
fn texts_in(output: &egui::FullOutput, within: egui::Rect) -> Vec<(String, egui::Rect)> {
    fn collect(shape: &egui::Shape, within: egui::Rect, out: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) if !t.galley.text().trim().is_empty() => {
                // Where its rows are (a label wrapped onto a line others
                // started is placed at the line's start, its text after them).
                let Some(rect) =
                    t.galley.rows.iter().map(|r| r.rect_without_leading_space().translate(t.pos.to_vec2())).reduce(|a, b| a.union(b))
                else {
                    return;
                };
                if within.contains_rect(rect) {
                    out.push((t.galley.text().to_owned(), rect));
                }
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, within, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, within, &mut out);
    }
    out
}

/// At a phone's width, by touch (G.7): the gutter's mark beside the
/// lines, over none of them; a tap opens the discussion; in it, no text
/// drawn over other text.
#[test]
fn at_a_phones_width_a_tap_on_the_mark_opens_it_and_nothing_is_drawn_over_anything() {
    let (ctx, mut app) = full_app();
    app.run_shell(ShellCommand::SetLayout(1), None, None, &ctx);
    focus(&ctx, &mut app, BufferKind::Terminal);
    app.desk.terminal_mut().arrive(vec![super::terminal::with_commands(1000, vec![super::terminal::command(1, 1003, Some(1008), "make")])]);
    let on = harness_contracts::Subject::TerminalCommand {
        terminal: harness_contracts::TerminalId::new("t1"),
        shell: 1,
        command: 1,
        typed: String::new(),
    };
    let mut d = crate::discussions::tests::discussion(1, on, harness_contracts::DiscussionStatus::Answered);
    d.said.push(harness_contracts::Said {
        by: "lead".into(),
        at_ms: 9,
        kind: harness_contracts::SaidKind::Answer,
        text: "It fails on the second step: the cache is stale, so the build reads the old lock file.".into(),
        links: Vec::new(),
        changed: Vec::new(),
    });
    told_discussion(&app, d);
    super::terminal::not_typing(&ctx, &mut app);
    let phone = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(390.0, 844.0));
    let mut t = 10.0;
    let mut at = |ctx: &egui::Context, app: &mut CockpitApp, events: Vec<egui::Event>| {
        t += 0.05;
        run(ctx, app, egui::RawInput { events, time: Some(t), screen_rect: Some(phone), ..Default::default() })
    };
    at(&ctx, &mut app, vec![]);
    let output = at(&ctx, &mut app, vec![]);
    let line = drawn_rects(&output, "line 1003")[0];
    let (mark, _) = glyphs(&output, egui_phosphor::fill::CHAT_CIRCLE)
        .into_iter()
        .find(|(r, _)| r.y_range().contains(line.center().y))
        .expect("the mark");
    let lines: Vec<_> = texts_in(&output, phone).into_iter().filter(|(text, _)| text.starts_with("line ")).collect();
    assert!(lines.iter().all(|(_, r)| !r.intersects(mark)), "over no line: {mark:?}");
    let touch =
        |phase| egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(1), phase, pos: mark.center(), force: None };
    let button = |pressed| egui::Event::PointerButton {
        pos: mark.center(),
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    // A tap: as a browser gives it (the touch, and the pointer it moves).
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::Start), egui::Event::PointerMoved(mark.center()), button(true)]);
    at(&ctx, &mut app, vec![touch(egui::TouchPhase::End), button(false), egui::Event::PointerGone]);
    at(&ctx, &mut app, vec![]);
    assert_eq!(app.quick_look.as_ref().map(|q| q.kind()), Some(BufferKind::Discussion), "a tap opens it");
    at(&ctx, &mut app, vec![]);
    let output = at(&ctx, &mut app, vec![]);
    // The quick look's (painted from its title on, over what's under it).
    let from = output
        .shapes
        .iter()
        .position(|c| matches!(&c.shape, egui::Shape::Text(t) if t.galley.text().ends_with("Quick look")))
        .expect("the quick look");
    let over = egui::FullOutput { shapes: output.shapes[from..].to_vec(), ..Default::default() };
    let drawn = texts_in(&over, phone);
    assert!(drawn.iter().any(|(text, _)| text.contains("cache is stale")), "{drawn:?}");
    for (n, (a, ra)) in drawn.iter().enumerate() {
        for (b, rb) in &drawn[n + 1..] {
            assert!(ra.intersect(*rb).area() < 1.0, "{a:?} {ra:?} drawn over {b:?} {rb:?}");
        }
    }
}

/// Starting one with other agent slots than the lead: who answers is
/// picked (the lead first); `#` in its first message names a tag there;
/// let go of halfway (Escape), nothing of it is left to run.
#[test]
fn one_is_started_asking_the_slot_picked_with_a_tag_named_in_it_and_escape_leaves_nothing() {
    let (ctx, mut app) = full_app();
    let mut sub = crate::chat::tests::lead(None);
    (sub.id, sub.role, sub.name) = (harness_contracts::SlotId::new("sub-1a2b"), "sub".into(), "reviewer".into());
    for slot in [crate::chat::tests::lead(None), sub] {
        app.desk.shared.agents.get_mut().apply(harness_contracts::ClientEvent::SlotUpdated { slot });
    }
    told(
        &app,
        harness_contracts::ClientEvent::TagUpdated {
            tag: harness_contracts::Tag {
                id: harness_contracts::TagId::new("x1"),
                name: "flaky".into(),
                colour: 2,
                note: String::new(),
                noted_by: String::new(),
                on: Vec::new(),
            },
        },
    );
    focus(&ctx, &mut app, BufferKind::Code);
    app.desk.code_mut().move_to("1");
    let file = app.desk.code().current_file.clone();
    keys(&ctx, &mut app, "C");
    keys(&ctx, &mut app, "question");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("discuss.ask"), "who answers, with another slot to ask");
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.find("lead").is_some_and(|lead| drawn.find("reviewer").is_some_and(|r| lead < r)), "the lead first: {drawn}");
    keys(&ctx, &mut app, "reviewer");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("discuss.write"));
    // `#`: the tag picker; its name put in after it, back to writing.
    keys(&ctx, &mut app, "see #");
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("tag.insert"), "the tag picker");
    keys(&ctx, &mut app, "fla");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("discuss.write"), "back to what's written");
    keys(&ctx, &mut app, "now");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let line = harness_contracts::Subject::CodeLines { path: file, first: 1, last: 1, snapshot: String::new() };
    // The tag picked is a link (its span, the tag), not words to find.
    let link = serde_json::json!({ "start": 4, "end": 10, "to": { "kind": "tag", "tag": "x1" }, "include": "auto" });
    assert_eq!(
        bodies(&app, "POST /api/discussions"),
        [serde_json::json!({ "subject": line, "kind": "question", "text": "see #flaky now", "links": [link], "asked": "reviewer" })]
    );
    // Not started (the server refused): what was written is there to send again.
    assert!(app.desk.net.answer("POST /api/discussions", 400, &serde_json::json!({ "error": "it's gone" })));
    frame(&ctx, &mut app, vec![]);
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(
        app.palette.picking() == Some("discuss.write")
            && drawn.contains("see #flaky now")
            && app.note.as_deref().is_some_and(|n| n.contains("it's gone")),
        "{drawn}"
    );
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    // Let go of halfway: nothing left to run.
    keys(&ctx, &mut app, "C");
    keys(&ctx, &mut app, "change");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, press(egui::Key::Escape, false));
    frame(&ctx, &mut app, vec![]);
    assert!(!app.palette.is_open() && app.desk.discussing.is_none(), "let go of: gone");
    assert!(!app.palette_actions().iter().any(|b| b.action.id == "discuss.write" || b.action.id == "discuss.ask"));
}

/// One an agent asks you: you say your answer, then settle it (no answer
/// of an agent's comes first); the discussions on what you're on open by
/// their action as by their mark.
#[test]
fn one_asked_of_you_is_settled_once_youve_answered_and_those_here_open_by_their_action() {
    let (ctx, mut app) = full_app();
    app.desk.terminal_mut().arrive(vec![super::terminal::with_commands(1000, vec![super::terminal::command(1, 1003, Some(1008), "make")])]);
    let on = harness_contracts::Subject::TerminalCommand {
        terminal: harness_contracts::TerminalId::new("t1"),
        shell: 1,
        command: 1,
        typed: String::new(),
    };
    let mut d = crate::discussions::tests::discussion(1, on, harness_contracts::DiscussionStatus::Open);
    (d.by, d.asked) = ("lead".into(), "user".into());
    d.said[0].by = "lead".into();
    told_discussion(&app, d);
    super::terminal::not_typing(&ctx, &mut app);
    app.run_move(BufferKind::Terminal, Move::Motion(Motion::Line(1005)), None, &ctx);
    frame(&ctx, &mut app, vec![]);
    // Its action, where you are: one there, so which is asked, then it opens.
    let offered = app.palette_actions().into_iter().find(|b| b.action.id == "discuss.open").map(|b| b.action.when);
    assert_eq!(offered, Some(crate::actions::When::Always), "offered on what has one");
    app.run_by_id("discuss.open", None, &ctx);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.quick_look.as_ref().map(|q| q.kind()), Some(BufferKind::Discussion));
    // Asked of you: Settle is there while it's open.
    let drawn = texts(&frame(&ctx, &mut app, vec![]));
    assert!(drawn.contains("waiting on you") && drawn.contains("asks you"), "{drawn}");
    bodies(&app, "");
    keys(&ctx, &mut app, "a");
    assert_eq!(bodies(&app, "POST /api/discussions/d1/settle"), [serde_json::json!({})], "settled from the cockpit");
}
