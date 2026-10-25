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

/// What's drawn within `inside` (clipped to it, or to a part of it): each
/// text, where it's laid out and how much of it shows (cut by its clip).
fn drawn_texts(output: &egui::FullOutput, inside: egui::Rect) -> Vec<(String, egui::Rect, egui::Rect)> {
    fn collect(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<(String, egui::Rect, egui::Rect)>) {
        match shape {
            egui::Shape::Text(t) => {
                let rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                out.push((t.galley.text().to_owned(), rect, rect.intersect(clip)));
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, clip, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for clipped in output.shapes.iter().filter(|c| inside.expand(1.0).contains_rect(c.clip_rect)) {
        collect(&clipped.shape, clipped.clip_rect, &mut out);
    }
    out
}

/// Every kind of item, each holding a long line with nowhere to break it
/// (a file path), then an ordinary one after it, in a narrow panel: the
/// panel keeps its width; nothing of it shows outside the panel, and
/// nothing starts left of it (the drift); long words wrap in the column
/// (only a table's row scrolls sideways, in its message's box).
#[test]
fn long_unbreakable_lines_stay_inside_a_narrow_panel() {
    let long = format!("crates/{}/src/lib.rs", "a_very_long_folder_name_with_no_spaces".repeat(4));
    let words = "words that wrap in the column ".repeat(30);
    let said = |text: String| ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text }], streaming: false };
    let tool = |title: Option<&str>, locations, output| {
        let body = ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output, locations };
        match title {
            Some(title) => titled(100, title, body),
            None => item(100, body),
        }
    };
    let cases: Vec<(&str, Vec<ClientEvent>)> = vec![
        ("your message", vec![item(100, ItemBody::UserMessage { text: format!("read {long} please") })]),
        ("a message's words", vec![item(100, said(format!("It's in {long}, and `{long}` too. {words}")))]),
        ("a message's code block", vec![item(100, said(format!("```\n{long} {long}\n```")))]),
        ("a message's table", vec![item(100, said(format!("| file | lines |\n|---|---|\n| {long} | 3 |\n\n{words}")))]),
        ("reasoning", vec![item(100, ItemBody::Reasoning { text: long.clone(), streaming: false })]),
        ("a tool call's title", vec![tool(Some(&format!("Read {long}")), vec![], vec![])]),
        ("a tool call's places", vec![tool(None, vec![FileLocation { path: long.clone(), line: Some(3) }], vec![])]),
        ("a tool call's output", vec![tool(None, vec![], vec![ContentBlock::Text { text: long.clone() }])]),
        ("a command", vec![item(100, ItemBody::CommandExecution { command: format!("cat {long}"), output: Some(long.clone()), exit_code: Some(0) })]),
        ("a file change", vec![item(100, ItemBody::FileChange { path: long.clone(), diff: None })]),
        ("a notice", vec![item(100, ItemBody::Notice { level: NoticeLevel::Info, text: long.clone() })]),
        ("an error", vec![item(100, ItemBody::Error { message: long.clone() })]),
        ("a request waiting", request("r1", &format!("cat {long}"), Some(&long), Some(100))),
        // (Its words last: what it's doing, a file with a long name.)
        ("a running turn's line", running_turn(vec![item(100, said(words.clone())), item(102, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations: vec![FileLocation { path: format!("src/{}.rs", "a_very_long_folder_name_with_no_spaces".repeat(4)), line: None }] })])),
    ];
    let mut broke = Vec::new();
    for (case, events) in cases {
        let (ctx, mut app) = full_app();
        *app.desk.net.ws_state.lock().unwrap() = crate::net::WsState::Open;
        focus(&ctx, &mut app, BufferKind::Chat);
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
        // Its text: the long name, the words, and the one after it.
        let ours: Vec<_> = drawn_texts(&output, panel).into_iter().filter(|(t, _, _)| t.contains("a_very_long_folder") || t.contains("words that wrap") || t == "after it").collect();
        if !ours.iter().any(|(t, _, shows)| t == "after it" && shows.is_positive()) {
            broke.push(format!("{case}: the item after it doesn't show"));
        }
        for (text, rect, shows) in &ours {
            let what = text.chars().take(30).collect::<String>();
            if rect.left() < panel.left() - 0.5 {
                broke.push(format!("{case}: {what:?} starts left of the panel: {rect:?} in {panel:?}"));
            }
            if shows.is_positive() && shows.right() > panel.right() + 0.5 {
                broke.push(format!("{case}: {what:?} shows past the panel: {shows:?} in {panel:?}"));
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
/// you follow the latest), and `y` twice answers each in turn (one being
/// answered doesn't wait on you).
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
        let texts = drawn_texts(output, panel);
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
    assert!(keyed(&app, "request.r1.allow") && !keyed(&app, "request.r2.allow"), "the keys: the first's, following the latest");
    frame(&ctx, &mut app, key("y"));
    assert!(keyed(&app, "request.r2.allow"), "the first being answered: the keys move on");
    frame(&ctx, &mut app, key("y"));
    let answered: Vec<String> = sent(&app).into_iter().filter(|c| c.starts_with("POST /api/threads/t/requests/")).collect();
    assert_eq!(answered, ["POST /api/threads/t/requests/r1", "POST /api/threads/t/requests/r2"], "each answered in turn");
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
    let texts = drawn_texts(&output, panel);
    assert!(texts.iter().any(|(t, _, shows)| t == "a long command" && shows.is_positive()), "shown with no item for it yet");
    assert!(texts.iter().any(|(t, _, shows)| t.contains("Type a prompt") && panel.contains_rect(*shows) && shows.height() > 4.0), "the typing box still in the panel");
    assert!(app.bound_actions().iter().any(|b| b.action.id == "request.r1.allow" && !b.keys.is_empty()), "and answerable");
}

/// While a turn runs, its line says what the agent's doing and how live it
/// is, in place of "RUNNING": receiving a moment after an update, the time
/// since the last one while waiting (not since the turn began), no word
/// after a minute, the connection down; of the turn running, not one
/// queued after it; asking for frames only as often as something on it
/// moves (none for words that don't change).
#[test]
fn a_running_turn_says_what_its_doing_and_how_live_it_is() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    *app.desk.net.ws_state.lock().unwrap() = crate::net::WsState::Open;
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
        let words = texts(&output).lines().find(|l| l.contains("render.rs") && !l.contains("crates/") || l.contains("No word") || l.contains("connection")).unwrap_or_default().to_owned();
        (words, output.viewport_output.values().map(|v| v.repaint_delay.as_secs_f64()).fold(f64::INFINITY, f64::min))
    };
    // The turn starts at 10 s.
    told(&app, turn("now", 2, harness_contracts::TurnStatus::Running));
    told(&app, call(None));
    frame_at(&ctx, &mut app, vec![], 10.0);
    let (words, again) = line(&ctx, &mut app, 10.5);
    assert!(words == "Reading render.rs…" && again <= 0.3, "receiving: {words:?}, again in {again}");
    assert!(!texts(&frame_at(&ctx, &mut app, vec![], 10.55)).contains("RUNNING"));
    // An update at 30 s, a prompt queued after it at 31 s.
    told(&app, call(Some("read".into())));
    frame_at(&ctx, &mut app, vec![], 30.0);
    told(&app, turn("next", 3, harness_contracts::TurnStatus::Queued));
    frame_at(&ctx, &mut app, vec![], 31.0);
    let (words, again) = line(&ctx, &mut app, 45.0);
    assert!(words == "Reading render.rs… 15 s since anything" && again <= 1.0, "waiting, since the last update, of the turn running: {words:?}");
    let (words, again) = line(&ctx, &mut app, 95.0);
    assert!(words.starts_with("No word for 1 min") && again < 60.0, "quiet: {words:?}");
    *app.desk.net.ws_state.lock().unwrap() = crate::net::WsState::Closed;
    let (words, _) = line(&ctx, &mut app, 96.0);
    assert_eq!(words, "The connection to the server is down");
    *app.desk.net.ws_state.lock().unwrap() = crate::net::WsState::Open;
    // Shown as nothing: the words stay; quiet, they're drawn again only
    // when the minutes change.
    let none = app.bound_actions().into_iter().find(|b| b.action.id == "chat.indicator").expect("the setting");
    app.invoke(Invocation { action: none.action, origin: none.origin, value: Some("none".into()) }, &ctx);
    let (words, again) = line(&ctx, &mut app, 100.0);
    assert!(app.desk.chat().ui.indicator.name() == "none" && words.starts_with("No word for 1 min") && again > 10.0, "{words:?}, again in {again}");
}

/// How much the chat shows is a setting on its line ("Shows:", beside
/// "At work:"): quiet folds even one call into a line, everything opens
/// every call. The cursor inside a run folded by the switch is on the
/// run's row. Picking the one it is keeps what was set by hand; another
/// starts afresh.
#[test]
fn how_much_the_chat_shows_is_its_setting() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    app.desk.shared.agents.get_mut().apply(item(40, ItemBody::ToolCall { kind: ToolKind::Search, name: "grep".into(), input: None, output: vec![ContentBlock::Text { text: "found".into() }], locations: vec![] }));
    let text = texts(&frame(&ctx, &mut app, vec![]));
    assert!(text.contains("Shows:") && text.contains("At work:"), "on its settings row: {text}");
    let set = |ctx: &egui::Context, app: &mut CockpitApp, to: &str| {
        let shows = app.bound_actions().into_iter().find(|b| b.action.id == "chat.verbosity").expect("the setting");
        app.invoke(Invocation { action: shows.action, origin: shows.origin, value: Some(to.into()) }, ctx);
        frame(ctx, app, vec![]);
    };
    let rows = |app: &CockpitApp| {
        let shape = app.desk.chat().shape();
        (shape.rows.len(), shape.rows.iter().filter(|r| r.folded).count())
    };
    assert_eq!(rows(&app), (40, 1), "normal: the run of two folded, the lone call its row");
    set(&ctx, &mut app, "everything");
    assert_eq!(rows(&app), (41, 0), "everything: every call a row");
    app.desk.chat_mut().move_to("3");
    set(&ctx, &mut app, "quiet");
    assert_eq!(rows(&app), (40, 2), "quiet: the lone call folded too");
    let spots = app.desk.chat().spots();
    assert!(app.desk.chat().cursor_key().as_deref() == Some("2") && spots.iter().any(|s| s.key == "2"), "the cursor on the run's row, drawn");
    assert_eq!(app.desk.chat_mut().enter(), crate::nav::Step::Done, "Enter opens the run");
    assert_eq!(rows(&app), (41, 1));
    set(&ctx, &mut app, "quiet");
    assert_eq!(rows(&app), (41, 1), "the same one: what was set stays");
    set(&ctx, &mut app, "normal");
    assert_eq!(rows(&app), (40, 1), "another: afresh");
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
    *app.desk.net.ws_state.lock().unwrap() = crate::net::WsState::Open;
    focus(&ctx, &mut app, BufferKind::Chat);
    let long = "a line of the answer\n\n".repeat(6);
    let said: Vec<ClientEvent> = (100..120).map(|n| item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: long.clone() }], streaming: false })).collect();
    running_turn(said).into_iter().for_each(|e| told(&app, e));
    let shown = |ctx: &egui::Context, app: &mut CockpitApp, t: f64| {
        frame_at(ctx, app, vec![], t - 0.05);
        let output = frame_at(ctx, app, vec![], t);
        let panel = app.panel_rects[app.desk.panels.focused()];
        let live = drawn_texts(&output, panel).into_iter().find(|(text, _, shows)| text == "Writing…" && shows.is_positive()).map(|(_, rect, _)| rect);
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
        drawn_texts(&output, app.panel_rects[app.desk.panels.focused()]).into_iter().filter(|(_, _, shows)| shows.is_positive()).map(|(text, rect, _)| (text, rect)).collect::<Vec<_>>()
    };
    let at = |texts: &[(String, egui::Rect)], said: &str| texts.iter().find(|(t, _)| t.contains(said)).map(|(_, r)| *r);
    let texts = drawn(&ctx, &mut app);
    assert!(at(&texts, "2 steps by its sub-agent").is_some() && at(&texts, "the helper's words").is_none() && at(&texts, "mine, meanwhile").is_some(), "folded under its call: {texts:?}");
    app.desk.chat_mut().move_to("40");
    assert!(app.bound_actions().iter().any(|b| b.action.label == "Unfold its sub-agent's work"), "Enter says it");
    assert_eq!(app.desk.chat_mut().enter(), crate::nav::Step::Done);
    let texts = drawn(&ctx, &mut app);
    let (call, words) = (at(&texts, "Explore the parser").expect("the call"), at(&texts, "the helper's words").expect("its words, open"));
    assert!(words.left() > call.left() && words.top() > call.top(), "a step in, under it: {call:?} {words:?}");
    let ours = |text: &str| at(&texts, text).map(|r| r.top());
    let mine = ours("mine, meanwhile").unwrap_or(f32::INFINITY);
    assert!(ours("the helper's words") < ours("src/helper_read.rs") && ours("src/helper_read.rs").is_some_and(|read| read < mine), "its items follow it, before what came between");
    app.desk.chat_mut().fold(crate::nav::Fold::Close);
    assert!(at(&drawn(&ctx, &mut app), "the helper's words").is_none(), "folded again");
    // (In the order shown, its read is 42nd: before what came between.)
    app.desk.chat_mut().move_to("42");
    assert!(at(&drawn(&ctx, &mut app), "src/helper_read.rs").is_some(), "a jump to its call opens it");
}

/// The chat's settings row wraps in its panel at every width, whatever
/// its pickers say (a harness starting says so in its name): no panel
/// reaches past the next, or the screen.
#[test]
fn the_settings_row_keeps_to_its_panel() {
    let mut broke = Vec::new();
    for status in [harness_contracts::ProviderStatus::Ready, harness_contracts::ProviderStatus::Starting] {
        for width in (520..=1400).step_by(20) {
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
                shown = texts(&ctx.run(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ctx| app.frame(ctx)));
            }
            assert!(shown.contains("Harness:"), "{shown}");
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
/// (asked of the server once each), are places a hint picks, and `c`
/// copies the one picked (else the first), said so.
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
    assert_eq!(asked, [serde_json::json!({ "language": "rust", "text": "fn main() {}" }), serde_json::json!({ "language": "py", "text": "print(1)\nprint(2)" })], "each once");
    app.desk.shared.highlighter.get_mut().knows("rust", "fn main() {}", arch_graph::code::Highlights { names: vec!["keyword".into()], runs: vec![[0, 2, 0]] });
    let keyword = app.desk.shared.highlighter.get_mut().colour("keyword").expect("the keyword's colour");
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
    app.desk.chat_mut().move_to("40");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, key("c"));
    assert_eq!((copied(&output).as_deref(), app.note.as_deref()), (Some("fn main() {}"), Some("Copied the code block (1 line).")));
    app.desk.chat_mut().move_to("40:c1");
    frame(&ctx, &mut app, vec![]);
    let output = frame(&ctx, &mut app, key("c"));
    assert_eq!((copied(&output).as_deref(), app.note.as_deref()), (Some("print(1)\nprint(2)"), Some("Copied the code block (2 lines).")));
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
    assert!(app.bound_actions().iter().any(|b| b.action.label == "Open lib.rs"), "Enter says which");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!((app.focused_kind(), app.desk.code().current_file.as_str(), app.desk.code().target_line), (BufferKind::Code, "lib.rs", Some(7)));
}
