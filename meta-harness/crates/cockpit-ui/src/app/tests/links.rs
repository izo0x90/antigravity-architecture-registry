//! Links written into a prompt: `@` picks a file, lines of one or a
//! symbol from the code index, and the prompt sends it as a link.

use super::*;

/// `@` links a file (or a symbol, or lines) as `#` links a tag: the picker
/// asks the code index, the pick is a link in the box (the text says its
/// name), and the prompt sends it as a link; nothing typed is lost.
#[test]
fn an_at_typed_in_a_prompt_links_a_file_found_and_the_prompt_sends_the_link() {
    let (ctx, mut app) = full_app();
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    let typed = |ctx: &egui::Context, app: &mut CockpitApp, text: &str| {
        for c in text.chars() {
            frame(ctx, app, vec![egui::Event::Text(c.to_string())]);
        }
        frame(ctx, app, vec![]);
    };
    typed(&ctx, &mut app, "read @");
    assert_eq!(app.palette.picking(), Some("link.thing"), "the thing picker");
    typed(&ctx, &mut app, "a.rs");
    let asked: Vec<String> = sent(&app).into_iter().filter(|c| c.starts_with("GET /api/search?q=a.rs")).collect();
    assert_eq!(asked.len(), 2, "files and symbols: {asked:?}");
    let file = serde_json::json!({ "query": "a.rs", "kind": "files", "hits": [{ "kind": "files", "label": "src/a.rs", "detail": "", "file": "src/a.rs" }], "total": 1 });
    assert!(app.desk.net.answer(&asked[0], 200, &file));
    assert!(app.desk.net.answer(&asked[1], 200, &serde_json::json!({ "query": "a.rs", "kind": "symbols", "hits": [], "total": 0 })));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    typed(&ctx, &mut app, "now");
    assert_eq!(app.desk.chat().ui.input.text, "read @src/a.rs now");
    bodies(&app, "");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let thing = serde_json::json!({ "kind": "thing", "subject": { "kind": "code_file", "path": "src/a.rs" } });
    assert_eq!(bodies(&app, "POST /api/slots/lead/prompt"), [serde_json::json!({ "text": "read @src/a.rs now", "links": [{ "start": 5, "end": 14, "to": thing, "include": "auto" }], "addToRunning": false })]);
}

/// The `@` picker's rows stay as they are while what's typed next is
/// looked for (the list doesn't empty and fill at each key), files before
/// symbols whichever answer comes first; an Enter meanwhile waits for the
/// rows of what's typed, and an answer to a query gone is dropped.
#[test]
fn the_at_picker_keeps_its_rows_until_the_next_are_all_in_and_enter_waits_for_them() {
    let (ctx, mut app) = full_app();
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    let typed = |ctx: &egui::Context, app: &mut CockpitApp, text: &str| {
        for c in text.chars() {
            frame(ctx, app, vec![egui::Event::Text(c.to_string())]);
        }
        frame(ctx, app, vec![]);
    };
    let asked = |app: &CockpitApp, q: &str| -> Vec<String> { sent(app).into_iter().filter(|c| c.starts_with(&format!("GET /api/search?q={q}&"))).collect() };
    let files = |q: &str, paths: &[&str]| serde_json::json!({ "query": q, "kind": "files", "hits": paths.iter().map(|p| serde_json::json!({ "kind": "files", "label": p, "detail": "", "file": p })).collect::<Vec<_>>(), "total": paths.len() });
    let symbols = |q: &str, names: &[&str]| serde_json::json!({ "query": q, "kind": "symbols", "hits": names.iter().map(|n| serde_json::json!({ "kind": "symbols", "label": n, "detail": "a function", "key": format!("rust:{n}") })).collect::<Vec<_>>(), "total": names.len() });
    typed(&ctx, &mut app, "read @a");
    let first = asked(&app, "a");
    assert_eq!(first.len(), 2, "files and symbols: {:?}", sent(&app));
    // Symbols answer first: nothing shown until both are in, then files first.
    assert!(app.desk.net.answer(&first[1], 200, &symbols("a", &["alpha"])));
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("Looking…") && !shown.contains("alpha"), "half the answers: still looking: {shown}");
    assert!(app.desk.net.answer(&first[0], 200, &files("a", &["src/a.rs"])));
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.find("src/a.rs").zip(shown.find("alpha")).is_some_and(|(f, s)| f < s), "files, then symbols: {shown}");

    typed(&ctx, &mut app, "b");
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("src/a.rs") && !shown.contains("Looking…"), "the rows of `a` stay while `ab` is looked for");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("link.thing"), "Enter on the rows of `a` waits for those of `ab`");
    // An answer for `a` coming late changes nothing.
    assert!(!app.desk.net.answer(&first[0], 200, &files("a", &["src/late.rs"])), "already answered");
    let next = asked(&app, "ab");
    assert_eq!(next.len(), 2);
    assert!(app.desk.net.answer(&next[0], 200, &files("ab", &["src/ab.rs"])));
    assert!(app.desk.net.answer(&next[1], 200, &symbols("ab", &[])));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat().ui.input.text, "read @src/ab.rs ", "the Enter, taken on the rows of what was typed");
}
