//! Links written into a prompt: `@` picks a file, lines of one or a
//! symbol from the code index, and the prompt sends it as a link.

use super::*;

/// The cockpit with the chat focused and typed in.
fn typing_in_the_chat() -> (egui::Context, CockpitApp) {
    let (ctx, mut app) = full_app();
    crate::chat::tests::conversation_events().into_iter().for_each(|e| app.desk.shared.agents.get_mut().apply(e));
    focus(&ctx, &mut app, BufferKind::Chat);
    frame(&ctx, &mut app, key("i"));
    (ctx, app)
}

/// `text` typed a key at a time, then a frame.
fn typed(ctx: &egui::Context, app: &mut CockpitApp, text: &str) {
    for c in text.chars() {
        frame(ctx, app, vec![egui::Event::Text(c.to_string())]);
    }
    frame(ctx, app, vec![]);
}

/// The searches asked since last looked: (files, symbols) for `q`.
fn asked(app: &CockpitApp, q: &str) -> (Vec<String>, Vec<String>) {
    let calls = sent(app);
    let of = |kind: &str| calls.iter().filter(|c| c.starts_with(&format!("GET /api/search?q={q}&kind={kind}&"))).cloned().collect();
    (of("files"), of("symbols"))
}

fn files(q: &str, paths: &[&str]) -> serde_json::Value {
    serde_json::json!({ "query": q, "kind": "files", "hits": paths.iter().map(|p| serde_json::json!({ "kind": "files", "label": p, "detail": "", "file": p })).collect::<Vec<_>>(), "total": paths.len() })
}

fn symbols(q: &str, names: &[&str]) -> serde_json::Value {
    serde_json::json!({ "query": q, "kind": "symbols", "hits": names.iter().map(|n| serde_json::json!({ "kind": "symbols", "label": n, "detail": "a function", "key": format!("rust:{n}") })).collect::<Vec<_>>(), "total": names.len() })
}

/// "read @a" typed and its rows in (the symbol alpha, the file
/// src/apple.rs).
fn rows_of_a(ctx: &egui::Context, app: &mut CockpitApp) {
    typed(ctx, app, "read @a");
    let (files_a, symbols_a) = asked(app, "a");
    assert!(app.desk.net.answer(&files_a[0], 200, &files("a", &["src/apple.rs"])));
    assert!(app.desk.net.answer(&symbols_a[0], 200, &symbols("a", &["alpha"])));
    frame(ctx, app, vec![]);
}

/// On the rows of what was typed before, a row chosen (a key) is taken at
/// once by Enter; an Enter that waited is dropped by typing after it.
#[test]
fn enter_takes_a_row_chosen_on_the_rows_shown_and_typing_drops_one_waiting() {
    let (ctx, mut app) = typing_in_the_chat();
    rows_of_a(&ctx, &mut app);
    typed(&ctx, &mut app, "b");
    frame(&ctx, &mut app, press(egui::Key::ArrowDown, false));
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat().ui.input.text, "read @src/apple.rs ", "the row chosen, taken at once");

    typed(&ctx, &mut app, "@a");
    let (files_a, symbols_a) = asked(&app, "a");
    assert!(app.desk.net.answer(&files_a[0], 200, &files("a", &["src/apple.rs"])));
    assert!(app.desk.net.answer(&symbols_a[0], 200, &symbols("a", &["alpha"])));
    frame(&ctx, &mut app, vec![]);
    typed(&ctx, &mut app, "b");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    typed(&ctx, &mut app, "c");
    // The rows of `ab` come, then those of `abc`: nothing was picked.
    // (Each answered wherever it's still waiting: the first half's `ab`,
    // never answered, has the same address.)
    let answer_all = |app: &CockpitApp, call: &str, body: serde_json::Value| while app.desk.net.answer(call, 200, &body) {};
    let (files_ab, symbols_ab) = asked(&app, "ab");
    answer_all(&app, &files_ab[0], files("ab", &["src/ab.rs"]));
    answer_all(&app, &symbols_ab[0], symbols("ab", &[]));
    frame(&ctx, &mut app, vec![]);
    let (files_abc, symbols_abc) = asked(&app, "abc");
    answer_all(&app, &files_abc[0], files("abc", &["src/abc.rs"]));
    answer_all(&app, &symbols_abc[0], symbols("abc", &[]));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.palette.picking(), Some("link.thing"), "the Enter before `c` was typed is dropped");
    assert!(texts(&frame(&ctx, &mut app, vec![])).contains("src/abc.rs"), "and the rows of `abc` are shown, to pick from");
}

/// `@` links a file (or a symbol, or lines) as `#` links a tag: the picker
/// asks the code index, the pick is a link in the box (the text says its
/// name), and the prompt sends it as a link; nothing typed is lost.
#[test]
fn an_at_typed_in_a_prompt_links_a_file_found_and_the_prompt_sends_the_link() {
    let (ctx, mut app) = typing_in_the_chat();
    typed(&ctx, &mut app, "read @");
    assert_eq!(app.palette.picking(), Some("link.thing"), "the thing picker");
    // (Pasted at once: one query asked.)
    frame(&ctx, &mut app, vec![egui::Event::Text("a.rs".into())]);
    frame(&ctx, &mut app, vec![]);
    let calls = sent(&app);
    let asked = |kind: &str| calls.iter().find(|c| c.starts_with(&format!("GET /api/search?q=a.rs&kind={kind}&"))).cloned().unwrap_or_else(|| panic!("{kind} asked: {calls:?}"));
    let file = serde_json::json!({ "query": "a.rs", "kind": "files", "hits": [{ "kind": "files", "label": "src/a.rs", "detail": "", "file": "src/a.rs" }], "total": 1 });
    assert!(app.desk.net.answer(&asked("files"), 200, &file));
    assert!(app.desk.net.answer(&asked("symbols"), 200, &serde_json::json!({ "query": "a.rs", "kind": "symbols", "hits": [], "total": 0 })));
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
/// looked for (the list doesn't empty and fill at each key), symbols
/// before files whichever answer comes first; an Enter meanwhile waits for
/// the rows of what's typed.
#[test]
fn the_at_picker_keeps_its_rows_until_the_next_are_all_in_and_enter_waits_for_them() {
    let (ctx, mut app) = typing_in_the_chat();
    typed(&ctx, &mut app, "read @a");
    let (first_files, first_symbols) = asked(&app, "a");
    assert_eq!((first_files.len(), first_symbols.len()), (1, 1), "files and symbols");
    // Files answer first: nothing shown until both are in, then symbols first.
    assert!(app.desk.net.answer(&first_files[0], 200, &files("a", &["src/apple.rs"])));
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("Looking…") && !shown.contains("src/apple.rs"), "half the answers: still looking: {shown}");
    assert!(app.desk.net.answer(&first_symbols[0], 200, &symbols("a", &["alpha"])));
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.find("alpha").zip(shown.find("src/apple.rs")).is_some_and(|(s, f)| s < f), "symbols, then files: {shown}");

    typed(&ctx, &mut app, "b");
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert!(shown.contains("src/apple.rs") && !shown.contains("Looking…"), "the rows of `a` stay while `ab` is looked for");
    frame(&ctx, &mut app, press(egui::Key::Enter, false));
    let shown = texts(&frame(&ctx, &mut app, vec![]));
    assert_eq!(app.palette.picking(), Some("link.thing"), "Enter on the rows of `a` waits for those of `ab`");
    assert!(shown.contains("Looking… · Enter picks"), "and the key line says it's waiting: {shown}");
    let (next_files, next_symbols) = asked(&app, "ab");
    assert_eq!((next_files.len(), next_symbols.len()), (1, 1));
    assert!(app.desk.net.answer(&next_files[0], 200, &files("ab", &["src/ab.rs"])));
    assert!(app.desk.net.answer(&next_symbols[0], 200, &symbols("ab", &[])));
    frame(&ctx, &mut app, vec![]);
    frame(&ctx, &mut app, vec![]);
    assert_eq!(app.desk.chat().ui.input.text, "read @src/ab.rs ", "the Enter, taken on the rows of what was typed");
}
