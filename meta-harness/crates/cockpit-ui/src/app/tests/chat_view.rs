//! The chat as it's drawn: whatever its items hold, every one stays inside
//! the panel.

use super::*;
use crate::chat::tests::item;
use harness_contracts::{AdapterEvent, ApprovalDecision, ApprovalOption, ClientEvent, ContentBlock, FileLocation, ItemBody, NoticeLevel, RequestId, RequestKind, RequestStatus, RuntimeRequest, ThreadId, ToolKind};

/// An item with a title (a tool call's, as harnesses give them).
fn titled(n: u32, title: &str, body: ItemBody) -> ClientEvent {
    let mut event = item(n, body);
    if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut event {
        item.title = Some(title.to_owned());
    }
    event
}

/// A command waiting for approval, `id`, titled `title` (`detail` said
/// under it), and its item in the stream (`item`: its place; none: not
/// come yet).
fn request(id: &str, title: &str, detail: Option<&str>, item: Option<u32>) -> Vec<ClientEvent> {
    let option = |o: &str, decision, label: &str| ApprovalOption { id: o.into(), decision, label: label.into(), warning: None };
    let request = RuntimeRequest {
        id: RequestId::new(id),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Approval { action: harness_contracts::ToolAction::Command },
        status: RequestStatus::Pending,
        title: title.to_owned(),
        detail: detail.map(str::to_owned),
        options: vec![option("allow", ApprovalDecision::Accept, "Allow"), option("deny", ApprovalDecision::Decline, "Deny")],
        questions: Vec::new(),
        response: None,
    };
    let mut events = vec![ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }];
    events.extend(item.map(|n| crate::chat::tests::item(n, ItemBody::Request { request_id: RequestId::new(id) })));
    events
}

/// A turn running (the second), holding `items`: the server's events.
fn running_turn(items: Vec<ClientEvent>) -> Vec<ClientEvent> {
    let turn = harness_contracts::Turn { id: harness_contracts::TurnId::new("now"), thread_id: ThreadId::new("t"), ordinal: 2, status: harness_contracts::TurnStatus::Running, failure: None, usage: None };
    let mut events = vec![ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } }];
    for mut event in items {
        if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut event {
            item.turn_id = harness_contracts::TurnId::new("now");
        }
        events.push(event);
    }
    events
}

/// Every text drawn (whatever it's clipped to): its words, where it's laid
/// out, and what it's clipped to.
fn drawn_texts(output: &egui::FullOutput) -> Vec<(String, egui::Rect, egui::Rect)> {
    fn collect(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<(String, egui::Rect, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), egui::Rect::from_min_size(t.pos, t.galley.size()), clip)),
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, clip, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}

/// What's drawn within `inside` (clipped to it, or to a part of it): each
/// text, where it's laid out and how much of it shows (cut by its clip).
fn shown_texts(output: &egui::FullOutput, inside: egui::Rect) -> Vec<(String, egui::Rect, egui::Rect)> {
    drawn_texts(output).into_iter().filter(|(_, _, clip)| inside.expand(1.0).contains_rect(*clip)).map(|(text, rect, clip)| (text, rect, rect.intersect(clip))).collect()
}

/// Every kind of item, each holding a long line with nowhere to break it
/// (a file path), then an ordinary one after it, in a narrow panel, with
/// everything open: the panel keeps its width; nothing of it shows outside
/// the panel, nothing starts left of it (the drift), nothing is cut off at
/// the stream's edge (only a box of its own inside it, a code block's or a
/// table's, scrolls sideways); long words wrap in the column.
#[test]
fn long_unbreakable_lines_stay_inside_a_narrow_panel() {
    use harness_contracts::changes::{DiffLine, Hunk, LineKind};
    let name = "a_very_long_folder_name_with_no_spaces".repeat(4);
    let long = format!("crates/{name}/src/lib.rs");
    let words = "words that wrap in the column ".repeat(30);
    let said = |text: String| ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text }], streaming: false };
    let tool = |title: Option<&str>, locations, output| {
        let body = ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output, locations };
        match title {
            Some(title) => titled(100, title, body),
            None => item(100, body),
        }
    };
    let change = harness_contracts::Diff { path: long.clone(), old_text: Some("a\n".into()), new_text: format!("{long}\n") };
    let mut sub = item(99, said(format!("The sub-agent read {long}. {words}")));
    if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut sub {
        item.parent_id = Some(harness_contracts::ItemId::new("i100"));
    }
    let steps = vec![harness_contracts::PlanStep { text: format!("read {long}"), status: harness_contracts::StepStatus::Pending }];
    let cases: Vec<(&str, Vec<ClientEvent>)> = vec![
        ("your message", vec![item(100, ItemBody::UserMessage { text: format!("read {long} please"), pieces: Vec::new() })]),
        ("a message's words", vec![item(100, said(format!("It's in {long}, and `{long}` too. {words}")))]),
        ("a message's code block", vec![item(100, said(format!("```\n{long} {long}\n```")))]),
        ("a message's table", vec![item(100, said(format!("| file | lines |\n|---|---|\n| {long} | 3 |\n\n{words}")))]),
        ("reasoning", vec![item(100, ItemBody::Reasoning { text: long.clone(), streaming: false })]),
        ("a tool call's title", vec![tool(Some(&format!("Read {long}")), vec![], vec![])]),
        ("a tool call's places", vec![tool(None, vec![FileLocation { path: long.clone(), line: Some(3) }], vec![])]),
        ("a tool call's output", vec![tool(None, vec![], vec![ContentBlock::Text { text: long.clone() }])]),
        ("a tool call's change", vec![tool(None, vec![], vec![ContentBlock::Diff(change.clone())])]),
        ("a sub-agent's work", vec![titled(100, "Task", ItemBody::ToolCall { kind: ToolKind::Other, name: "Task".into(), input: None, output: vec![], locations: vec![] }), sub]),
        ("a command", vec![item(100, ItemBody::CommandExecution { command: format!("cat {long}"), output: Some(long.clone()), exit_code: Some(0) })]),
        ("a file changed", vec![item(100, ItemBody::FileChange { path: long.clone(), diff: Some(change.clone()) })]),
        ("a file written whole", vec![item(100, ItemBody::FileChange { path: long.clone(), diff: Some(harness_contracts::Diff { old_text: None, ..change.clone() }) })]),
        ("a proposed plan", vec![item(100, ItemBody::ProposedPlan { plan_id: harness_contracts::PlanId::new("p"), markdown: format!("1. read {long}\n\n{words}") })]),
        ("a to-do list", vec![item(100, ItemBody::TodoList { plan_id: harness_contracts::PlanId::new("p"), steps: steps.clone() })]),
        ("a notice", vec![item(100, ItemBody::Notice { level: NoticeLevel::Info, text: long.clone() })]),
        ("an error", vec![item(100, ItemBody::Error { message: long.clone() })]),
        ("a request waiting", request("r1", &format!("cat {long}"), Some(&long), Some(100))),
        // (Its words last: what it's doing, a file with a long name.)
        ("a running turn's line", running_turn(vec![item(100, said(words.clone())), item(102, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: format!("src/{name}.rs"), line: None }] })])),
    ];
    let line = |kind, text: &str| DiffLine { kind, old: None, new: None, text: text.into() };
    let hunks = vec![Hunk { old_start: 1, old_lines: 1, new_start: 1, new_lines: 1, lines: vec![line(LineKind::Removed, "a"), line(LineKind::Added, &long)] }];
    let mut broke = Vec::new();
    for (case, events) in cases {
        let (ctx, mut app) = full_app();
        *app.home.ws_state.lock().unwrap() = crate::net::WsState::Open;
        focus(&ctx, &mut app, BufferKind::Chat);
        let shows = app.bound_actions().into_iter().find(|b| b.action.id == "chat.verbosity").expect("the setting");
        app.invoke(Invocation { action: shows.action, origin: shows.origin, value: Some("everything".into()) }, &ctx);
        app.desk.shared.comparer.get_mut().knows(&crate::comparer::Pair { old: "a\n".into(), new: change.new_text.clone(), context: 3 }, hunks.clone());
        frame(&ctx, &mut app, vec![]);
        let panel = app.panel_rects[app.desk.panels.focused()];
        events.into_iter().chain([item(101, said("after it".into()))]).for_each(|e| app.desk.shared.agents.get_mut().apply(e));
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        let output = frame(&ctx, &mut app, vec![]);
        let now = app.panel_rects[app.desk.panels.focused()];
        if now.width() > panel.width() + 0.5 {
            broke.push(format!("{case}: the panel widened from {} to {}", panel.width(), now.width()));
            continue;
        }
        // Its text: the long name, the words, and the one after it (the
        // stream's own clip is the one after it's); the panel's (not the
        // key bar's, across the screen).
        let ours: Vec<_> = drawn_texts(&output).into_iter().filter(|(t, _, clip)| clip.intersects(panel) && clip.left() >= panel.left() - 0.5 && (t.contains("a_very_long_folder") || t.contains("words that wrap") || t == "after it")).collect();
        let Some(stream) = ours.iter().find(|(t, rect, clip)| t == "after it" && rect.intersect(*clip).is_positive()).map(|(_, _, clip)| *clip) else {
            broke.push(format!("{case}: the item after it doesn't show"));
            continue;
        };
        for (text, rect, clip) in &ours {
            let what = text.chars().take(30).collect::<String>();
            let shows = rect.intersect(*clip);
            if rect.left() < panel.left() - 0.5 {
                broke.push(format!("{case}: {what:?} starts left of the panel: {rect:?} in {panel:?}"));
            }
            if shows.is_positive() && shows.right() > panel.right() + 0.5 {
                broke.push(format!("{case}: {what:?} shows past the panel: {shows:?} in {panel:?}"));
            }
            // (Clipped to a box of its own, it scrolls in it; to the
            // stream's own clip, it's cut off.)
            let own_box = (clip.min - stream.min).length() > 0.5 || (clip.max - stream.max).length() > 0.5;
            if shows.is_positive() && rect.right() > clip.right() + 0.5 && !own_box {
                broke.push(format!("{case}: {what:?} is cut off at the stream's edge: {rect:?} in {clip:?}"));
            }
            if text.contains("words that wrap") && (rect.width() > panel.width() || rect.height() < 30.0) {
                broke.push(format!("{case}: the words don't wrap in the column: {rect:?}"));
            }
        }
    }
    assert!(broke.is_empty(), "\n{}", broke.join("\n"));
}

/// Two requests waiting at once (sub-agents asking in parallel), kept in
/// one place under the stream however far the stream has gone on: the
/// same place every frame, nothing asked to be drawn again once settled.
/// Each is answerable; the keys are the one's you're on (the first while
/// you follow the latest): on the second's card, `y` answers it; following
/// the latest again, the first.
#[test]
fn two_requests_at_once_are_kept_in_one_place_and_each_answered() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    request("r1", "cargo test", None, Some(100)).into_iter().chain(request("r2", "cargo build", None, Some(101))).for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    for n in 102..140 {
        app.desk.shared.agents.get_mut().apply(item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: format!("later {n}") }], streaming: false }));
    }
    // (What's under the stream settles in a frame or two.)
    for _ in 0..6 {
        frame(&ctx, &mut app, vec![]);
    }
    let panel = app.panel_rects[app.desk.panels.focused()];
    let places = |output: &egui::FullOutput| {
        let texts = shown_texts(output, panel);
        let typing = texts.iter().find(|(t, _, shows)| t.contains("Type a prompt") && panel.contains_rect(*shows)).map(|(_, r, _)| *r);
        let card = |title: &str| texts.iter().find(|(t, _, shows)| t == title && shows.is_positive() && panel.contains_rect(*shows)).map(|(_, r, _)| *r);
        (card("cargo test"), card("cargo build"), typing)
    };
    let first = places(&frame(&ctx, &mut app, vec![]));
    let (Some(r1), Some(r2), Some(typing)) = first else { panic!("both in sight, and the typing box: {first:?}") };
    assert!(r1.bottom() <= r2.top() && r2.bottom() <= typing.top(), "in order, above the typing box: {first:?}");
    for _ in 0..4 {
        let output = frame(&ctx, &mut app, vec![]);
        assert_eq!(places(&output), first, "the same place every frame");
        assert!(output.viewport_output.values().all(|v| !v.repaint_delay.is_zero()), "nothing asked to be drawn again");
    }

    let keyed = |app: &CockpitApp, id: &str| app.bound_actions().iter().find(|b| b.action.id == id).is_some_and(|b| !b.keys.is_empty());
    let answered = |app: &CockpitApp| sent(app).into_iter().filter(|c| c.starts_with("POST /api/threads/t/requests/")).collect::<Vec<_>>();
    assert!(keyed(&app, "request.r1.allow") && !keyed(&app, "request.r2.allow"), "the keys: the first's, following the latest");
    sent(&app);
    let card = app.desk.chat().spots().into_iter().find(|s| s.rect.contains(r2.center())).expect("the second's card, a place");
    app.desk.chat_mut().move_to(&card.key);
    frame(&ctx, &mut app, vec![]);
    assert!(keyed(&app, "request.r2.allow") && !keyed(&app, "request.r1.allow"), "on the second: its keys");
    frame(&ctx, &mut app, key("y"));
    assert_eq!(answered(&app), ["POST /api/threads/t/requests/r2"], "y answers the one you're on");
    assert_eq!(app.desk.chat_mut().leave(), crate::nav::Step::Done);
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, key("y"));
    assert_eq!(answered(&app), ["POST /api/threads/t/requests/r1"], "following the latest: the first");
}

/// What waits takes at most a share of the chat (it scrolls past that):
/// a request said at great length leaves the typing box in the panel.
/// One whose item hasn't come yet is shown and answerable all the same.
#[test]
fn what_waits_is_bounded_and_answerable_before_its_item_comes() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let heredoc = (0..120).map(|n| format!("line {n} of a long command")).collect::<Vec<_>>().join("\n");
    request("r1", "a long command", Some(&heredoc), None).into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let output = frame(&ctx, &mut app, vec![]);
    let panel = app.panel_rects[app.desk.panels.focused()];
    let texts = shown_texts(&output, panel);
    assert!(texts.iter().any(|(t, _, shows)| t == "a long command" && shows.is_positive()), "shown with no item for it yet");
    assert!(texts.iter().any(|(t, _, shows)| t.contains("Type a prompt") && panel.contains_rect(*shows) && shows.height() > 4.0), "the typing box still in the panel");
    assert!(app.bound_actions().iter().any(|b| b.action.id == "request.r1.allow" && !b.keys.is_empty()), "and answerable");
}

/// While a turn runs, its line says what the agent's doing and how live it
/// is: receiving a moment after an update, the time since the last one
/// while waiting (not since the turn began), no word after a minute, the
/// connection down, waiting on you while a request of its waits (however
/// long); of the turn running, not one queued after it; asking for frames
/// as often as something on it moves (never faster than the fastest, and
/// for words alone, as they change: each second's).
#[test]
fn a_running_turn_says_what_its_doing_and_how_live_it_is() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    *app.home.ws_state.lock().unwrap() = crate::net::WsState::Open;
    let turn = |id: &str, ordinal, status| ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: harness_contracts::Turn { id: harness_contracts::TurnId::new(id), thread_id: ThreadId::new("t"), ordinal, status, failure: None, usage: None } } };
    let call = |status| {
        let mut call = item(200, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: "crates/cockpit-ui/src/render.rs".into(), line: None }] });
        if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut call {
            (item.turn_id, item.status, item.title) = (harness_contracts::TurnId::new("now"), harness_contracts::ItemStatus::InProgress, status);
        }
        call
    };
    let line = |ctx: &egui::Context, app: &mut CockpitApp, t: f64| {
        frame_at(ctx, app, vec![], t - 0.01);
        let output = frame_at(ctx, app, vec![], t);
        let words = texts(&output).lines().find(|l| l.contains("render.rs") && !l.contains("crates/") || l.contains("No word") || l.contains("connection") || l.contains("Waiting on you")).unwrap_or_default().to_owned();
        (words, output.viewport_output.values().map(|v| v.repaint_delay.as_secs_f64()).fold(f64::INFINITY, f64::min))
    };
    // The turn starts at 10 s.
    told(&app, turn("now", 2, harness_contracts::TurnStatus::Running));
    told(&app, call(None));
    frame_at(&ctx, &mut app, vec![], 10.0);
    let (words, again) = line(&ctx, &mut app, 10.5);
    assert!(words == "Reading render.rs…" && (crate::indicator::FASTEST_S..1.0).contains(&again), "receiving, moving: {words:?}, again in {again}");
    // An update at 30 s, a prompt queued after it at 31 s.
    told(&app, call(Some("read".into())));
    frame_at(&ctx, &mut app, vec![], 30.0);
    told(&app, turn("next", 3, harness_contracts::TurnStatus::Queued));
    frame_at(&ctx, &mut app, vec![], 31.0);
    let (words, again) = line(&ctx, &mut app, 45.0);
    assert!(words == "Reading render.rs… 15 s since anything" && again <= 1.0, "waiting, since the last update, of the turn running: {words:?}");
    let (words, again) = line(&ctx, &mut app, 95.0);
    assert!(words.starts_with("No word for 1 min") && again < 60.0, "quiet: {words:?}");
    *app.home.ws_state.lock().unwrap() = crate::net::WsState::Closed;
    let (words, _) = line(&ctx, &mut app, 96.0);
    assert_eq!(words, "The connection to the server is down");
    *app.home.ws_state.lock().unwrap() = crate::net::WsState::Open;
    // Shown as nothing: the words stay, nothing beside them; quiet, they're
    // drawn again only when the minutes change; waiting, each second.
    let marks = |output: &egui::FullOutput| output.shapes.iter().filter(|c| matches!(c.shape, egui::Shape::Path(_) | egui::Shape::Circle(_) | egui::Shape::LineSegment { .. })).count();
    frame_at(&ctx, &mut app, vec![], 99.0);
    let with = marks(&frame_at(&ctx, &mut app, vec![], 99.5));
    let none = app.bound_actions().into_iter().find(|b| b.action.id == "look.at_work").expect("the setting");
    app.invoke(Invocation { action: none.action, origin: none.origin, value: Some("none".into()) }, &ctx);
    let (words, again) = line(&ctx, &mut app, 100.0);
    assert!(words.starts_with("No word for 1 min") && again > 10.0, "{words:?}, again in {again}");
    assert!(marks(&frame_at(&ctx, &mut app, vec![], 100.5)) < with, "nothing drawn beside the words");
    told(&app, call(Some("read again".into())));
    frame_at(&ctx, &mut app, vec![], 101.0);
    let (words, again) = line(&ctx, &mut app, 105.4);
    assert!(words == "Reading render.rs… 4 s since anything" && (again - 0.6).abs() < 0.05, "the words, each second: {words:?}, again in {again}");
    assert_eq!(line(&ctx, &mut app, 106.0).0, "Reading render.rs… 5 s since anything");
    // A request of its waiting: it waits on you, however long.
    request("r1", "cargo test", None, None).into_iter().for_each(|e| told(&app, e));
    frame_at(&ctx, &mut app, vec![], 107.0);
    let (words, _) = line(&ctx, &mut app, 400.0);
    assert_eq!(words, "Waiting on you", "not the agent gone quiet");
}

/// How much the chat shows is a setting of each chat: picked so a run
/// folds round the cursor, the cursor is on the run's row; picking the one
/// it is keeps what was set by hand. What's picked stays through a change
/// of another setting, and through a reload.
#[test]
fn how_much_the_chat_shows_is_kept_with_the_chat() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.shared.agents.get_mut().apply(item(40, ItemBody::ToolCall { kind: ToolKind::Search, name: "grep".into(), input: None, output: vec![ContentBlock::Text { text: "found".into() }], locations: vec![] }));
    frame(&ctx, &mut app, vec![]);
    let set = |ctx: &egui::Context, app: &mut CockpitApp, to: &str| {
        let shows = app.bound_actions().into_iter().find(|b| b.action.id == "chat.verbosity").expect("the setting");
        app.invoke(Invocation { action: shows.action, origin: shows.origin, value: Some(to.into()) }, ctx);
        frame(ctx, app, vec![]);
    };
    let picked = |app: &CockpitApp| {
        let shows = app.bound_actions().into_iter().find(|b| b.action.id == "chat.verbosity").expect("the setting");
        let crate::actions::Param::Choice(choices) = shows.action.param else { panic!("a choice") };
        choices.into_iter().find(|c| c.current).map(|c| c.value)
    };
    let rows = |app: &CockpitApp| app.desk.chat().shape().rows.len();
    set(&ctx, &mut app, "everything");
    app.desk.chat_mut().move_to("3");
    set(&ctx, &mut app, "quiet");
    let spots = app.desk.chat().spots();
    assert!(app.desk.chat().cursor_key().as_deref() == Some("2") && spots.iter().any(|s| s.key == "2:run"), "the cursor on the run's row, drawn");
    assert_eq!(app.desk.chat_mut().enter(), crate::nav::Step::Done, "Enter opens the run");
    let opened = rows(&app);
    set(&ctx, &mut app, "quiet");
    assert_eq!(rows(&app), opened, "the same one: what was set stays");
    // Another setting changed: what's picked, and opened, stays.
    let mut settings = app.desk.shared.settings.get().clone();
    settings.changes.refresh_s += 1.0;
    app.desk.shared.settings.set(settings);
    frame(&ctx, &mut app, vec![]);
    assert_eq!((picked(&app).as_deref(), rows(&app)), (Some("quiet"), opened), "another setting changed");
    // A reload: kept with the chat.
    let kept = crate::buffer::Buffer::keep(app.desk.chat()).expect("what's picked, kept");
    let (ctx, mut fresh) = full_app();
    focus(&ctx, &mut fresh, BufferKind::Chat);
    crate::buffer::Buffer::restore_kept(fresh.desk.chat_mut(), &kept);
    frame(&ctx, &mut fresh, vec![]);
    assert_eq!(picked(&fresh).as_deref(), Some("quiet"), "through a reload");
}

/// A jump onto a call inside a folded run opens the run and shows that
/// call where it's laid out now (not where the folded rows had its row).
#[test]
fn a_jump_into_a_folded_run_shows_the_call_it_went_to() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let long = "a line of the answer\n\n".repeat(6);
    {
        let mut agents = app.desk.shared.agents.get_mut();
        for n in 40..60 {
            agents.apply(item(n, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: format!("src/f{n}.rs"), line: None }] }));
        }
        for n in 60..90 {
            agents.apply(item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: long.clone() }], streaming: false }));
        }
    }
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    app.desk.chat_mut().move_to("50");
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    let keys: Vec<String> = app.desk.chat().spots().into_iter().map(|s| s.key).collect();
    assert!(keys.contains(&"50".to_owned()), "the call it went to, in sight: {keys:?}");
}

/// Through a long turn, its line scrolled past while you follow the
/// latest, the live line is pinned at the stream's top and moves; with
/// the turn out of sight, nothing is asked for it.
#[test]
fn the_live_line_stays_in_sight_through_a_long_turn() {
    let (ctx, mut app) = full_app();
    *app.home.ws_state.lock().unwrap() = crate::net::WsState::Open;
    focus(&ctx, &mut app, BufferKind::Chat);
    let long = "a line of the answer\n\n".repeat(6);
    let said: Vec<ClientEvent> = (100..120).map(|n| item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: long.clone() }], streaming: false })).collect();
    running_turn(said).into_iter().for_each(|e| told(&app, e));
    let shown = |ctx: &egui::Context, app: &mut CockpitApp, t: f64| {
        frame_at(ctx, app, vec![], t - 0.05);
        let output = frame_at(ctx, app, vec![], t);
        let panel = app.panel_rects[app.desk.panels.focused()];
        let live = shown_texts(&output, panel).into_iter().find(|(text, _, shows)| text == "Writing…" && shows.is_positive()).map(|(_, rect, _)| rect);
        (live, output.viewport_output.values().map(|v| v.repaint_delay.as_secs_f64()).fold(f64::INFINITY, f64::min))
    };
    frame_at(&ctx, &mut app, vec![], 10.0);
    let (live, again) = shown(&ctx, &mut app, 10.5);
    let stream = app.panel_rects[app.desk.panels.focused()];
    assert!(live.is_some_and(|r| r.top() < stream.center().y) && again <= 0.3, "pinned at the top, moving: {live:?}, again in {again}");
    app.desk.chat_mut().move_to("0");
    for t in [11.0, 11.5, 12.0] {
        frame_at(&ctx, &mut app, vec![], t);
    }
    let (live, again) = shown(&ctx, &mut app, 13.0);
    assert!(live.is_none() && again > 1.0, "out of sight, nothing asked: {live:?}, again in {again}");
}

/// A sub-agent's work is under the call that started it, folded ("2
/// steps by its sub-agent"); Enter on the call unfolds it, its items a
/// step in; folded again, a jump to one of them opens it.
#[test]
fn a_sub_agents_work_is_folded_under_its_call() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let under = |mut event: ClientEvent| {
        if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut event {
            item.parent_id = Some(harness_contracts::ItemId::new("i40"));
        }
        event
    };
    let read = |n: u32, path: &str| item(n, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: path.into(), line: None }] });
    {
        let mut agents = app.desk.shared.agents.get_mut();
        agents.apply(titled(40, "Explore the parser", ItemBody::ToolCall { kind: ToolKind::Other, name: "Task".into(), input: None, output: vec![], locations: vec![] }));
        agents.apply(under(item(41, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "the helper's words".into() }], streaming: false })));
        agents.apply(item(42, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "mine, meanwhile".into() }], streaming: false }));
        agents.apply(under(read(43, "src/helper_read.rs")));
    }
    let drawn = |ctx: &egui::Context, app: &mut CockpitApp| {
        frame(ctx, app, vec![]);
        let output = frame(ctx, app, vec![]);
        shown_texts(&output, app.panel_rects[app.desk.panels.focused()]).into_iter().filter(|(_, _, shows)| shows.is_positive()).map(|(text, rect, _)| (text, rect)).collect::<Vec<_>>()
    };
    let at = |texts: &[(String, egui::Rect)], said: &str| texts.iter().find(|(t, _)| t.contains(said)).map(|(_, r)| *r);
    let texts = drawn(&ctx, &mut app);
    assert!(at(&texts, "2 steps by its sub-agent").is_some() && at(&texts, "the helper's words").is_none() && at(&texts, "mine, meanwhile").is_some(), "folded under its call: {texts:?}");
    app.desk.chat_mut().move_to("40");
    assert_eq!(app.desk.chat_mut().enter(), crate::nav::Step::Done);
    let texts = drawn(&ctx, &mut app);
    let (call, words) = (at(&texts, "Explore the parser").expect("the call"), at(&texts, "the helper's words").expect("its words, open"));
    assert!(words.left() > call.left() && words.top() > call.top(), "a step in, under it: {call:?} {words:?}");
    app.desk.chat_mut().fold(crate::nav::Fold::Close);
    assert!(at(&drawn(&ctx, &mut app), "the helper's words").is_none(), "folded again");
    let read = crate::nav::Navigable::items(app.desk.chat()).into_iter().find(|i| i.text.contains("src/helper_read.rs")).expect("its read, an item");
    app.desk.chat_mut().move_to(&read.key);
    assert!(at(&drawn(&ctx, &mut app), "src/helper_read.rs").is_some(), "a jump to its call opens it");
}

/// The chat's settings row wraps in its panel at every width, whatever
/// its pickers say (a harness starting says so in its name): no panel
/// reaches past the next, or the screen. (Widths across where the row
/// starts to wrap, and the widest.)
#[test]
fn the_settings_row_keeps_to_its_panel() {
    let mut broke = Vec::new();
    for status in [harness_contracts::ProviderStatus::Ready, harness_contracts::ProviderStatus::Starting] {
        for width in [520, 560, 600, 640, 680, 720, 800, 1000, 1400] {
            // (No session yet: the harness is picked on the row.)
            let (ctx, mut app) = app_with_graph();
            app.desk.shared.agents.get_mut().apply(ClientEvent::SlotUpdated { slot: crate::chat::tests::lead(None) });
            focus(&ctx, &mut app, BufferKind::Chat);
            let mut snapshot = crate::agents::tests::snapshot(status.clone());
            snapshot.instance_id = harness_contracts::InstanceId::new("i");
            let mut other = crate::agents::tests::snapshot(harness_contracts::ProviderStatus::Ready);
            (other.instance_id, other.display_name) = (harness_contracts::InstanceId::new("claude-code"), "Claude Code".into());
            app.desk.shared.agents.get_mut().apply(ClientEvent::InstanceUpdated { snapshot });
            app.desk.shared.agents.get_mut().apply(ClientEvent::InstanceUpdated { snapshot: other });
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width as f32, 900.0));
            let mut shown = String::new();
            for _ in 0..4 {
                shown = texts(&run(&ctx, &mut app, egui::RawInput { screen_rect: Some(screen), ..Default::default() }));
            }
            assert!(app.bound_actions().iter().any(|b| b.action.id == "instance.select"), "the harness picked on the row: {shown}");
            let panels = &app.panel_rects;
            let past = panels.windows(2).any(|p| p[0].right() > p[1].left() + 0.5) || panels.last().is_some_and(|p| p.right() > width as f32 + 0.5);
            if past {
                broke.push(format!("{status:?} at {width}: {panels:?}"));
            }
        }
    }
    assert!(broke.is_empty(), "{broke:#?}");
}

/// A message's code blocks are coloured as the language each fence names
/// (asked of the server once each, in one call; a call that fails, plain,
/// asked again only after a while), are places a hint picks, and `c`
/// copies the one picked (else the message).
#[test]
fn a_messages_code_blocks_are_coloured_picked_and_copied() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let text = "Two ways:\n```rust\nfn main() {}\n```\nor\n```py\nprint(1)\nprint(2)\n```\n";
    app.desk.shared.agents.get_mut().apply(item(40, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: text.into() }], streaming: false }));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let asked: Vec<serde_json::Value> = bodies(&app, "POST /api/code/highlight");
    assert_eq!(asked, [serde_json::json!({ "blocks": [{ "language": "rust", "text": "fn main() {}" }, { "language": "py", "text": "print(1)\nprint(2)" }] })], "each once, in one call");
    assert!(app.desk.net.answer("POST /api/code/highlight", 500, &serde_json::Value::Null), "the call, answered: it failed");
    for t in [1.0, 2.0, 3.0] {
        frame_at(&ctx, &mut app, vec![], t);
    }
    assert!(bodies(&app, "POST /api/code/highlight").is_empty(), "failed just now: not asked again yet");
    frame_at(&ctx, &mut app, vec![], 3.0 + crate::asked::RETRY_S);
    frame_at(&ctx, &mut app, vec![], 3.1 + crate::asked::RETRY_S);
    assert_eq!(bodies(&app, "POST /api/code/highlight").len(), 1, "asked again after a while");
    let rust = serde_json::json!({ "names": ["keyword"], "runs": [[0, 2, 0]] });
    assert!(app.desk.net.answer("POST /api/code/highlight", 200, &serde_json::json!([rust, null])));
    let keyword = app.desk.shared.highlighter.get_mut().colour("keyword").expect("the keyword's colour");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    fn coloured(shape: &egui::Shape, out: &mut Vec<(String, egui::Color32)>) {
        match shape {
            egui::Shape::Text(t) => out.extend(t.galley.job.sections.iter().map(|s| (t.galley.job.text[s.byte_range.clone()].to_owned(), s.format.color))),
            egui::Shape::Vec(list) => list.iter().for_each(|s| coloured(s, out)),
            _ => {}
        }
    }
    let mut parts = Vec::new();
    output.shapes.iter().for_each(|c| coloured(&c.shape, &mut parts));
    assert!(parts.iter().any(|(t, c)| t == "fn" && *c == keyword) && parts.iter().any(|(t, _)| t == " main() {}"), "coloured as the code view colours rust");
    let keys: Vec<String> = app.desk.chat().spots().into_iter().map(|s| s.key).collect();
    assert!(keys.contains(&"40:c0".to_owned()) && keys.contains(&"40:c1".to_owned()), "places of their own: {keys:?}");
    let copied = |output: &egui::FullOutput| output.platform_output.commands.iter().find_map(|c| match c { egui::OutputCommand::CopyText(t) => Some(t.clone()), _ => None });
    app.desk.chat_mut().move_to("40:c0");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, key("c"));
    assert_eq!(copied(&output).as_deref(), Some("fn main() {}"));
    app.desk.chat_mut().move_to("40");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, key("c"));
    assert_eq!(copied(&output).as_deref(), Some(text), "none picked: the message");
    app.desk.chat_mut().move_to("40:c1");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, key("c"));
    assert_eq!(copied(&output).as_deref(), Some("print(1)\nprint(2)"));
}

/// The files an agent's words name are links under its message, as a
/// call's places are: places of their own (`f`), and Enter opens the one
/// picked (else the first) in the code view, at its line.
#[test]
fn the_files_a_message_names_open_in_the_code_view() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let said = "Fixed in `src/chat/render.rs:42`, and lib.rs:7 too.";
    app.desk.shared.agents.get_mut().apply(item(40, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: said.into() }], streaming: false }));
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    let shown = texts(&output);
    assert!(shown.contains("src/chat/render.rs:42") && shown.contains("lib.rs:7"), "{shown}");
    let keys: Vec<String> = app.desk.chat().spots().into_iter().map(|s| s.key).collect();
    assert!(keys.contains(&"40:0".to_owned()) && keys.contains(&"40:1".to_owned()), "{keys:?}");
    app.desk.chat_mut().move_to("40:1");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.code().current_file.as_str(), app.desk.code().target_line), (BufferKind::Code, "lib.rs", Some(7)));
}

/// A file a call changed is "+2 −1 src/a.rs" under it, folded, as the
/// server compares its two texts (asked for once, when it's drawn);
/// opened, its hunks as the Changes view draws them, unnumbered (the texts
/// may be a part of the file). One written whole (no old text) says so.
#[test]
fn a_change_is_folded_as_what_it_adds_and_removes_and_opens_as_its_hunks() {
    use harness_contracts::changes::{DiffLine, Hunk, LineKind};
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let diff = harness_contracts::Diff { path: "src/a.rs".into(), old_text: Some("a\nb\nc\n".into()), new_text: "a\nB\nc\nd\n".into() };
    let edit = ItemBody::ToolCall { kind: ToolKind::Edit, name: "edit".into(), input: None, output: vec![ContentBlock::Diff(diff.clone())], locations: vec![] };
    {
        let mut agents = app.desk.shared.agents.get_mut();
        agents.apply(titled(40, "Edit a.rs", edit));
        agents.apply(item(41, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "between".into() }], streaming: false }));
        agents.apply(item(42, ItemBody::FileChange { path: "src/b.rs".into(), diff: Some(harness_contracts::Diff { path: "src/b.rs".into(), old_text: None, new_text: "x\ny\n".into() }) }));
    }
    sent(&app);
    let shown = |ctx: &egui::Context, app: &mut CockpitApp| {
        frame(ctx, app, vec![]);
        texts(&frame(ctx, app, vec![]))
    };
    let text = shown(&ctx, &mut app);
    assert_eq!(sent(&app).iter().filter(|s| s.contains("/api/changes/texts")).count(), 1, "asked for once");
    assert!(text.lines().any(|l| l == "src/a.rs changed") && text.contains("src/b.rs written whole (2 lines)"), "not compared yet; written whole: {text}");
    // As git compares them, said by the server.
    let line = |kind, text: &str| DiffLine { kind, old: None, new: None, text: text.into() };
    let hunk = Hunk { old_start: 1, old_lines: 3, new_start: 1, new_lines: 4, lines: vec![line(LineKind::Context, "a"), line(LineKind::Removed, "b"), line(LineKind::Added, "B"), line(LineKind::Context, "c"), line(LineKind::Added, "d")] };
    assert!(app.desk.net.answer("POST /api/changes/texts", 200, &serde_json::json!([[hunk]])));
    frame(&ctx, &mut app, vec![]);
    let text = shown(&ctx, &mut app);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.windows(3).any(|w| w == ["+2", "−1", "src/a.rs"]), "folded, as the Changes view counts: {text}");
    app.desk.chat_mut().move_to("40");
    app.desk.chat_mut().fold(crate::nav::Fold::Open);
    let text = shown(&ctx, &mut app);
    assert!(text.lines().any(|l| l == "B") && !text.contains("@@"), "its hunks, unnumbered: {text}");
    app.desk.chat_mut().move_to("42");
    app.desk.chat_mut().fold(crate::nav::Fold::Open);
    // (Opened at the stream's foot: shown whole once moved to again.)
    frame(&ctx, &mut app, vec![]);
    app.desk.chat_mut().move_to("42");
    let text = shown(&ctx, &mut app);
    assert!(text.contains("x\ny"), "written whole, opened: its text: {text}");
}

/// Your message shows its text, what it links as chips (places of their
/// own; Enter goes to what one links), and what they handed on folded
/// under it. One with no links shows as it came.
#[test]
fn your_message_shows_its_links_as_chips_and_what_went_with_it_folded() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    told(&app, ClientEvent::TagUpdated { tag: harness_contracts::Tag { id: harness_contracts::TagId::new("x1"), name: "flaky".into(), colour: 2, note: String::new(), noted_by: String::new(), on: Vec::new() } });
    let handed_on = "#flaky\n- terminal t1 line 3: the attached words";
    let piece = harness_contracts::Piece { links: vec![harness_contracts::Link { start: 8, end: 14, to: harness_contracts::Linked::Tag { tag: harness_contracts::TagId::new("x1") }, include: harness_contracts::Include::Auto }], text: handed_on.into() };
    {
        let mut agents = app.desk.shared.agents.get_mut();
        agents.apply(item(40, ItemBody::UserMessage { text: "look at #flaky.".into(), pieces: vec![piece] }));
        agents.apply(item(41, ItemBody::UserMessage { text: "older one, as sent".into(), pieces: Vec::new() }));
    }
    frame(&ctx, &mut app, vec![]);
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("look at #flaky.") && text.contains("what went with it") && !text.contains("attached words"), "its text, what went with it folded: {text}");
    assert!(text.lines().any(|l| l == "older one, as sent"), "no links: as it came: {text}");
    let keys: Vec<String> = app.desk.chat().spots().into_iter().map(|s| s.key).collect();
    assert!(keys.contains(&"40:l0".to_owned()), "the chip, a place: {keys:?}");
    app.desk.chat_mut().move_to("40:l0");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.view(BufferKind::Tags).cursor_key().as_deref()), (BufferKind::Tags, Some("x1")), "the tag it links, the cursor on it");
    // What went with it, opened.
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.chat_mut().move_to("40");
    app.desk.chat_mut().fold(crate::nav::Fold::Open);
    frame(&ctx, &mut app, vec![]);
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("the attached words"));
}

/// `] ]` and `[ [` go to the next and previous message (yours or the
/// agent's), past the calls between; `/` finds what's said, in a folded
/// run too (its first call's too: the run opened), `n` and `N` going on
/// to the next found and back, in order.
#[test]
fn messages_are_moved_through_and_searched() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    {
        let mut agents = app.desk.shared.agents.get_mut();
        for n in 40..43 {
            agents.apply(item(n, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: format!("src/needle{n}.rs"), line: None }] }));
        }
        agents.apply(item(43, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "the needle is found".into() }], streaming: false }));
    }
    frame(&ctx, &mut app, vec![]);
    let key_of = |app: &CockpitApp, text: &str| crate::nav::Navigable::items(app.desk.chat()).into_iter().find(|i| i.text.contains(text)).map(|i| i.key).expect("an item");
    let (before, run, message) = (key_of(&app, "message 39"), key_of(&app, "src/needle40.rs"), key_of(&app, "the needle is found"));
    app.desk.chat_mut().move_to(&before);
    keys(&ctx, &mut app, "]]");
    assert_eq!(app.desk.chat().cursor_key(), Some(message.clone()), "past the run of calls: the next message");
    keys(&ctx, &mut app, "[[");
    assert_eq!(app.desk.chat().cursor_key(), Some(before.clone()), "back: the one before");
    // The run's first call found: the run opened, the cursor on the call.
    keys(&ctx, &mut app, "/needle40");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat().cursor_key(), Some(run.clone()));
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("src/needle40.rs"), "its call shows, the run open");
    // Several found: n to the next after the cursor, N back.
    keys(&ctx, &mut app, "/needle4");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    let mut seen = vec![app.desk.chat().cursor_key()];
    for step in ["n", "n", "N"] {
        frame(&ctx, &mut app, key(step));
        seen.push(app.desk.chat().cursor_key());
    }
    let (n41, n42) = (key_of(&app, "src/needle41.rs"), key_of(&app, "src/needle42.rs"));
    assert_eq!(seen, [Some(run), Some(n41.clone()), Some(n42), Some(n41)], "in order, on and back");
    assert_eq!(app.search_status(&ctx).map(|(t, _)| t).as_deref(), Some("/needle4 2/3"));
    // A sub-agent's step put in before them: still where it says.
    let mut sub = item(44, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "a step".into() }], streaming: false });
    if let ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } = &mut sub {
        item.parent_id = Some(harness_contracts::ItemId::new("i39"));
    }
    told(&app, sub);
    frame(&ctx, &mut app, vec![]);
    // (Found again at most so often while the items change.)
    std::thread::sleep(std::time::Duration::from_millis(600));
    assert_eq!(app.search_status(&ctx).map(|(t, _)| t).as_deref(), Some("/needle4 2/3"), "found again once the items changed");
}

/// From a file a call changed, `g x` shows the turn's changes in the
/// Changes view, the cursor on that file once its list is in.
#[test]
fn a_change_goes_to_its_turns_changes_on_its_file() {
    use harness_contracts::changes::{ChangeList, ChangedFile, Comparison, FileStatus, Group, Point, Timeline, TurnChanges};
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let turn = Comparison { from: Point::Snapshot("1-a".into()), to: Point::Snapshot("2-b".into()) };
    app.desk.shared.changes.take_timeline_for_test(Timeline { windows: vec![], turns: vec![TurnChanges { thread: "t".into(), turn: "turn".into(), comparison: turn.clone(), files: 2, alongside: vec![] }] });
    app.desk.shared.agents.get_mut().apply(item(40, ItemBody::FileChange { path: "src/b.rs".into(), diff: None }));
    frame(&ctx, &mut app, vec![]);
    app.desk.chat_mut().move_to("40");
    keys(&ctx, &mut app, "gx");
    assert_eq!((app.focused_kind(), app.desk.shared.changes.comparison()), (BufferKind::Changes, &turn), "its turn's changes");
    let file = |path: &str| ChangedFile { path: path.into(), old_path: None, status: FileStatus::Modified, group: Group::Unstaged, added: 1, removed: 0, binary: false };
    app.desk.shared.changes.take_list_for_test(ChangeList { comparison: turn, files: vec![file("src/a.rs"), file("src/b.rs")], offset: 0, total: 2 });
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.view(BufferKind::Changes).target().and_then(|t| t.code()).map(|(path, _)| path).as_deref(), Some("src/b.rs"), "on its file");
}

/// A long chat draws only what's in sight or near it, from the first
/// frame it's shown: the rows far above take their room undrawn.
#[test]
fn a_long_chat_draws_only_what_is_near_sight() {
    let (ctx, mut app) = full_app();
    {
        let mut agents = app.desk.shared.agents.get_mut();
        for n in 40..240 {
            agents.apply(item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: format!("said {n}\n\nand more of it") }], streaming: false }));
        }
    }
    focus(&ctx, &mut app, BufferKind::Chat);
    // (What's laid out anew asks for the next frame till it settles.)
    let mut drawn = Vec::new();
    for _ in 0..3 {
        let output = frame(&ctx, &mut app, vec![]);
        drawn = texts(&output).lines().filter_map(|l| l.strip_prefix("said ")).filter_map(|n| n.parse::<u32>().ok()).collect();
        if !output.viewport_output.values().any(|v| v.repaint_delay.is_zero()) {
            break;
        }
    }
    assert!(drawn.contains(&239) && drawn.len() < 40, "the latest, and only what's near it: {drawn:?}");
}

/// What waits on you shows its answers whether you're typing or not: a
/// request's Allow and Deny stay under it while you type, and one tap
/// answers it.
#[test]
fn a_request_waiting_on_you_shows_its_answers_while_you_type() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    // Two at once (two sub-agents ask), the cursor on neither.
    request("r1", "cargo test", None, Some(100)).into_iter().chain(request("r2", "cargo fmt", None, Some(101))).for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    for _ in 0..4 {
        frame(&ctx, &mut app, vec![]);
    }
    // A button: its caption, then its key.
    let buttons = |output: &egui::FullOutput, caption: &str| -> Vec<egui::Pos2> { drawn_texts(output).into_iter().filter(|(t, _, _)| t.split_whitespace().next() == Some(caption)).map(|(_, r, _)| r.center()).collect() };
    let under_each = |output: &egui::FullOutput| {
        let (allow, deny) = (buttons(output, "Allow"), buttons(output, "Deny"));
        let cards = [drawn_containing(output, "cargo test"), drawn_containing(output, "cargo fmt")].map(|c| c.last().copied());
        allow.len() == 2 && deny.len() == 2 && cards.iter().all(Option::is_some) && {
            // Each card's answers under it, before the next card.
            let (first, second) = (cards[0].unwrap(), cards[1].unwrap());
            allow.iter().any(|a| a.y > first.bottom() && a.y < second.top()) && allow.iter().any(|a| a.y > second.bottom())
        }
    };
    let output = frame(&ctx, &mut app, vec![]);
    assert!(under_each(&output), "not typing: under each: {}", texts(&output));
    frame(&ctx, &mut app, key("i"));
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, vec![]);
    assert!(matches!(app.input_mode, InputMode::Insert { .. }));
    assert!(under_each(&output), "typing: each its answers still: {}", texts(&output));
    let second = *buttons(&output, "Allow").iter().max_by(|a, b| a.y.total_cmp(&b.y)).unwrap();
    sent(&app);
    click(&ctx, &mut app, second);
    assert!(sent(&app).iter().any(|c| c.starts_with("POST /api/threads/t/requests/r2")), "tapped while typing: the second answered");
}

/// A long chat costs a frame what a short one does, as the agent streams
/// into its last message: only the rows near the screen are drawn, and
/// where the others are is kept, not worked out again; `gg` gets to the
/// top all the same.
#[test]
fn a_long_chat_costs_a_frame_what_a_short_one_does() {
    let said = |n: u32, text: String| crate::chat::tests::item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text }], streaming: true });
    // The slowest frame, over many, each with a chunk more of the last message.
    let slowest = |count: u32| {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Chat);
        (100..100 + count).for_each(|n| app.desk.shared.agents.get_mut().apply(said(n, format!("message {n}"))));
        for _ in 0..4 {
            frame(&ctx, &mut app, vec![]);
        }
        let last = 99 + count;
        let slowest = (0..30)
            .map(|k| {
                told(&app, said(last, format!("message {last}{}", " more".repeat(k))));
                let start = std::time::Instant::now();
                frame(&ctx, &mut app, vec![]);
                start.elapsed()
            })
            .max()
            .unwrap();
        (slowest, ctx, app)
    };
    let (short, ..) = slowest(20);
    let (long, ctx, mut app) = slowest(2_000);
    eprintln!("slowest frame streaming: {short:?} with 20 messages, {long:?} with 2,000");
    assert!(long < short * 3 + std::time::Duration::from_millis(10), "{long:?} against {short:?}");
    keys(&ctx, &mut app, "gg");
    // (It glides there.)
    for n in 0..8 {
        frame_at(&ctx, &mut app, vec![], 10.0 + f64::from(n) * 0.1);
    }
    let text = texts(&frame_at(&ctx, &mut app, vec![], 11.0));
    assert!(text.contains("fix the parser") && !text.contains("message 2099"), "{text}");
}

/// With the buttons in a bar, what waits on you shows its answers there
/// too while you're on something else and typing.
#[test]
fn in_a_bar_what_waits_shows_its_answers_while_youre_elsewhere() {
    let (ctx, mut app) = full_app();
    app.item_buttons = Some(crate::hosted::ItemButtons::Bar);
    focus(&ctx, &mut app, BufferKind::Chat);
    crate::chat::tests::conversation_events().into_iter().chain(request("r1", "cargo test", None, Some(100))).for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    frame(&ctx, &mut app, vec![]);
    keys(&ctx, &mut app, "gg");
    frame(&ctx, &mut app, key("i"));
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    assert!(matches!(app.input_mode, InputMode::Insert { .. }) && app.desk.chat().ui.cursor == Some(0));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Allow") && text.contains("Deny"), "{text}");
}

/// The key bar cuts only a chip too wide for its share of the bar (a
/// request's title can be a whole command), by width; the others whole.
#[test]
fn the_key_bar_cuts_only_a_chip_too_wide_by_width() {
    // Its "Allow: …" chip, as the bar draws it, for a request asking `title`.
    let chip = |title: &str| {
        let (ctx, mut app) = full_app();
        focus(&ctx, &mut app, BufferKind::Chat);
        request("r1", title, None, Some(100)).into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        let output = frame(&ctx, &mut app, vec![]);
        let bar = drawn_texts(&output).into_iter().filter(|(_, rect, _)| rect.top() > SCREEN.y - 40.0).collect::<Vec<_>>();
        let allow = bar.iter().find(|(t, ..)| t.starts_with("y Allow:")).cloned().unwrap_or_else(|| panic!("{bar:?}"));
        (allow, bar.iter().any(|(t, ..)| t == "i Type a prompt"))
    };
    // Longer than 40 characters, narrower than a third of the bar: whole.
    let fits = "cargo test --workspace --all-targets -- --nocapture now";
    let ((text, rect, _), rest) = chip(fits);
    assert!(text.contains(fits) && rect.width() <= SCREEN.x / 3.0 && rest, "whole: {text:?}");
    // Wider than its share: cut to it.
    let ((text, rect, _), rest) = chip(&format!("cat {}", "crates/a_long_folder/".repeat(12)));
    assert!(rect.width() <= SCREEN.x / 3.0 + 1.0 && rest, "cut to its share: {text:?} {rect:?}");
}
