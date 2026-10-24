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

/// A request waiting, titled `title`, said further in `detail`.
fn request(title: &str, detail: &str) -> Vec<ClientEvent> {
    let option = |id: &str, decision, label: &str| ApprovalOption { id: id.into(), decision, label: label.into(), warning: None };
    let request = RuntimeRequest {
        id: RequestId::new("r1"),
        thread_id: ThreadId::new("t"),
        turn_id: None,
        item_id: None,
        kind: RequestKind::Approval { action: harness_contracts::ToolAction::Tool },
        status: RequestStatus::Pending,
        title: title.to_owned(),
        detail: Some(detail.to_owned()),
        options: vec![option("allow", ApprovalDecision::Accept, "Allow"), option("deny", ApprovalDecision::Decline, "Deny")],
        questions: Vec::new(),
        response: None,
    };
    vec![ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }, item(100, ItemBody::Request { request_id: RequestId::new("r1") })]
}

/// Every kind of item, each holding a long line with nowhere to break it
/// (a file path), then an ordinary one after it, in a narrow panel: the
/// panel keeps its width, and nothing of the item or the one after it
/// shows outside it (a table's long row scrolls sideways in its item's
/// box).
#[test]
fn long_unbreakable_lines_stay_inside_a_narrow_panel() {
    let long = format!("crates/{}/src/lib.rs", "a_very_long_folder_name_with_no_spaces".repeat(4));
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
        ("a message's words", vec![item(100, said(format!("It's in {long}, and `{long}` too.")))]),
        ("a message's code block", vec![item(100, said(format!("```\n{long} {long}\n```")))]),
        ("a message's table", vec![item(100, said(format!("| file | lines |\n|---|---|\n| {long} | 3 |")))]),
        ("reasoning", vec![item(100, ItemBody::Reasoning { text: long.clone(), streaming: false })]),
        ("a tool call's title", vec![tool(Some(&format!("Read {long}")), vec![], vec![])]),
        ("a tool call's places", vec![tool(None, vec![FileLocation { path: long.clone(), line: Some(3) }], vec![])]),
        ("a tool call's output", vec![tool(None, vec![], vec![ContentBlock::Text { text: long.clone() }])]),
        ("a command", vec![item(100, ItemBody::CommandExecution { command: format!("cat {long}"), output: Some(long.clone()), exit_code: Some(0) })]),
        ("a file change", vec![item(100, ItemBody::FileChange { path: long.clone(), diff: None })]),
        ("a notice", vec![item(100, ItemBody::Notice { level: NoticeLevel::Info, text: long.clone() })]),
        ("an error", vec![item(100, ItemBody::Error { message: long.clone() })]),
        ("a request", request(&format!("cat {long}"), &long)),
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
        // Its text, found by what it says (the long name, and the one
        // after), as much of it as shows (cut by what it's clipped to),
        // among what's clipped within the panel (not the key bar's).
        let mut drawn = Vec::new();
        fn collect(shape: &egui::Shape, clip: egui::Rect, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((t.galley.text().to_owned(), egui::Rect::from_min_size(t.pos, t.galley.size()).intersect(clip))),
                egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, clip, out)),
                _ => {}
            }
        }
        for clipped in output.shapes.iter().filter(|c| c.clip_rect.top() >= panel.top() - 0.5 && c.clip_rect.bottom() <= panel.bottom() + 0.5) {
            collect(&clipped.shape, clipped.clip_rect, &mut drawn);
        }
        drawn.retain(|(text, _)| text.contains("a_very_long_folder") || text == "after it");
        if !drawn.iter().any(|(t, _)| t == "after it") {
            broke.push(format!("{case}: the item after it isn't drawn"));
        }
        for (text, r) in drawn.iter().filter(|(_, r)| r.left() < panel.left() - 0.5 || r.right() > panel.right() + 0.5) {
            broke.push(format!("{case}: {:?} drawn at {r:?}, outside {panel:?}", text.chars().take(30).collect::<String>()));
        }
    }
    assert!(broke.is_empty(), "\n{}", broke.join("\n"));
}

/// Two requests waiting at once (sub-agents asking in parallel): each is
/// answerable, its answers' keys the one's you're on (the first while you
/// follow the latest); once later messages push them out of the stream's
/// sight, both are kept in sight above the typing box, in the panel.
#[test]
fn two_requests_at_once_are_each_answerable_and_kept_in_sight() {
    let (ctx, mut app) = full_app();
    focus(&ctx, &mut app, BufferKind::Chat);
    let ask = |id: &str, title: &str, n: u32| {
        let option = |o: &str, decision, label: &str| ApprovalOption { id: o.into(), decision, label: label.into(), warning: None };
        let request = RuntimeRequest {
            id: RequestId::new(id),
            thread_id: ThreadId::new("t"),
            turn_id: None,
            item_id: None,
            kind: RequestKind::Approval { action: harness_contracts::ToolAction::Command },
            status: RequestStatus::Pending,
            title: title.into(),
            detail: None,
            options: vec![option("allow", ApprovalDecision::Accept, "Allow"), option("deny", ApprovalDecision::Decline, "Deny")],
            questions: Vec::new(),
            response: None,
        };
        [ClientEvent::Session { event: AdapterEvent::RequestUpdated { request } }, item(n, ItemBody::Request { request_id: RequestId::new(id) })]
    };
    ask("r1", "cargo test", 100).into_iter().chain(ask("r2", "cargo build", 101)).for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    frame(&ctx, &mut app, vec![]);
    let keyed = |app: &CockpitApp, id: &str| app.bound_actions().iter().find(|b| b.action.id == id).is_some_and(|b| !b.keys.is_empty());
    let offered = |app: &CockpitApp, id: &str| app.bound_actions().iter().any(|b| b.action.id == id);
    assert!(offered(&app, "request.r1.allow") && offered(&app, "request.r2.allow"), "both answerable");
    assert!(keyed(&app, "request.r1.allow") && !keyed(&app, "request.r2.allow"), "the keys: the first's, following the latest");
    let second = app.desk.chat().pending_items()[1].0;
    app.desk.view_mut(BufferKind::Chat).move_to(&second.to_string());
    frame(&ctx, &mut app, vec![]);
    assert!(!keyed(&app, "request.r1.allow") && keyed(&app, "request.r2.allow"), "on the second: its keys");
    app.desk.view_mut(BufferKind::Chat).leave();

    // Later messages push them out of the stream's sight: kept in sight.
    for n in 102..140 {
        app.desk.shared.agents.get_mut().apply(item(n, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: format!("later {n}") }], streaming: false }));
    }
    for _ in 0..3 {
        frame(&ctx, &mut app, vec![]);
    }
    let output = frame(&ctx, &mut app, vec![]);
    let panel = app.panel_rects[app.desk.panels.focused()];
    let typing = drawn_containing(&output, "Type a prompt").into_iter().find(|r| panel.contains_rect(*r)).unwrap_or_else(|| panic!("the typing box, in the panel {panel:?}: {}", texts(&output)));
    for title in ["cargo test", "cargo build"] {
        let card = drawn_containing(&output, title).into_iter().find(|r| panel.contains_rect(*r)).unwrap_or_else(|| panic!("{title} in sight: {}", texts(&output)));
        assert!(card.bottom() <= typing.top(), "{title} above the typing box: {card:?}, {typing:?}");
    }
}
