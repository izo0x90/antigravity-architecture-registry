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
    ];
    let mut broke = Vec::new();
    for (case, events) in cases {
        let (ctx, mut app) = full_app();
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
