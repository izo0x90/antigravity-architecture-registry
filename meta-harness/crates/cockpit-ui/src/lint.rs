//! What code may do where, read from the sources (the code before each
//! file's test module). Two rules:
//!
//! - **Keys**: nothing reads or names keys behind the keymap's back.
//!   Outside it, no code names a key or a modifier, asks whether one is
//!   down, or writes one into text the user reads. Every key is a binding
//!   (or a list, typing or pointer key) the config sets, and is said by its
//!   label.
//! - **Controls**: views draw content; hosts draw every control. Outside
//!   the hosts (the top bar, columns, the quick look, the key bar and
//!   menus: `app/draw/`), the one control (`actions.rs`, `panel.rs`) and
//!   the palette, no code draws a button, link, switch, picker or anything
//!   clicked. A view's actions are placed (`Control`) and the host draws
//!   them.

/// Every source file but tests, its name (from `src/`) and its code.
fn code() -> Vec<(String, String)> {
    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "tests") {
                    sources(&path, out);
                }
            } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "tests.rs" && n != "lint.rs") {
                out.push(path);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    files
        .into_iter()
        .map(|file| {
            let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            let text = std::fs::read_to_string(&file).unwrap();
            // Its own tests make input as a user would: the code is what
            // comes before its test module.
            let code = text.match_indices("#[cfg(test)]\n").find(|(at, cut)| text[at + cut.len()..].trim_start_matches("pub(crate) ").starts_with("mod ")).map_or(text.as_str(), |(at, _)| &text[..at]).to_owned();
            (name, code)
        })
        .collect()
}

/// Each line of code that isn't a comment, numbered from 1.
fn lines(code: &str) -> impl Iterator<Item = (usize, &str)> {
    code.lines().enumerate().map(|(n, l)| (n + 1, l)).filter(|(_, l)| !l.trim_start().starts_with("//"))
}

/// Allowed by name, and why.
const EXCEPTIONS: &[(&str, &str, &str)] = &[
    // The agents' "press ctrl+enter" turned into input: it names keys to make them.
    ("remote.rs", "", "the agents' key names, parsed into input"),
    // A typed prompt (search, hint labels, marks) erases as any text box does.
    ("app/keys.rs", "egui::Key::Backspace", "a typed prompt's own erasing"),
];

const FORBIDDEN: &[&str] = &["egui::Key::", "Key::from_name", "key_pressed(egui", "key_released(egui", "key_down(egui", "modifiers.ctrl", "modifiers.command", "modifiers.shift", "modifiers.alt", "modifiers.mac_cmd"];

/// Key names, as words in text (not those that are also plain words:
/// Return, Delete, Home, End).
const KEY_WORDS: &[&str] = &["Enter", "Esc", "Escape", "Backspace", "Tab", "Space", "SPC", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "PageUp", "PageDown", "Up/Down"];

/// The string literals on a line (their text, escapes left as written).
fn literals(line: &str) -> Vec<String> {
    let (mut out, mut current, mut inside, mut escaped) = (Vec::new(), String::new(), false, false);
    let line = line.replace("'\"'", "");
    for c in line.chars() {
        match (inside, escaped, c) {
            (true, true, _) => escaped = false,
            (true, false, '\\') => escaped = true,
            (true, false, '"') => {
                out.push(std::mem::take(&mut current));
                inside = false;
            }
            (true, false, c) => current.push(c),
            (false, _, '"') => inside = true,
            _ => {}
        }
    }
    out
}

/// What in `text` names a key: a key's name as a word, a modifier with a
/// key ("ctrl+"), or the whole text a key sequence ("g d", "J / K", "a").
fn names_a_key(text: &str) -> Option<String> {
    let words = text.split(|c: char| !(c.is_alphanumeric() || c == '/' || c == '+'));
    if let Some(word) = words.into_iter().find(|w| KEY_WORDS.contains(w)) {
        return Some(word.to_owned());
    }
    let lower = text.to_ascii_lowercase();
    if let Some(m) = ["ctrl+", "shift+", "alt+", "cmd+"].into_iter().find(|m| lower.contains(m)) {
        return Some(m.to_owned());
    }
    let parts: Vec<&str> = text.split_whitespace().filter(|p| *p != "/").collect();
    let sequence = !parts.is_empty() && parts.iter().all(|p| p.chars().count() == 1 && p.chars().all(|c| c.is_ascii_alphabetic()));
    sequence.then(|| text.to_owned())
}

#[test]
fn no_code_reads_or_names_a_key_the_keymap_doesnt_give_it() {
    let mut found = Vec::new();
    for (name, code) in code().iter().filter(|(name, _)| !name.starts_with("keymap/")) {
        for (n, line) in lines(code) {
            let excused = |what: &str| EXCEPTIONS.iter().any(|(f, allowed, _)| f == name && (allowed.is_empty() || what.contains(allowed)));
            for token in FORBIDDEN.iter().filter(|t| line.contains(*t)) {
                if !excused(line) {
                    found.push(format!("{name}:{n}: {token}"));
                }
            }
            for text in literals(line) {
                if let Some(key) = names_a_key(&text).filter(|_| !excused(&text)) {
                    found.push(format!("{name}:{n}: \"{text}\" names {key} (say a bound key by its label)"));
                }
            }
        }
    }
    assert!(found.is_empty(), "keys read or named outside the keymap (bind them in [keymap], and say them by their labels):\n{}", found.join("\n"));
}

/// Where controls are drawn: the hosts, the one control, the palette; and
/// the developer tools (their own subsystem).
const HOSTS: &[&str] = &["app/draw/", "actions.rs", "panel.rs", "palette.rs", "feedback.rs"];

/// What draws a control, or reads a click.
const CONTROLS: &[&str] = &[".button(", "small_button(", "selectable_label(", ".link(", ".hyperlink(", "hyperlink_to(", ".checkbox(", "ComboBox::", "egui::Button::", "Sense::click", "radio", "toggle_value("];

/// Outside the hosts, allowed by name, and why.
const CONTROLS_ALLOWED: &[(&str, &str, &str)] = &[
    ("graph/draw.rs", "Sense::click_and_drag", "its canvas reads the pointer itself (cards, dragging, two fingers)"),
    ("code.rs", "Sense::click", "its text reads the pointer itself (the line, the name under it, the go-to modifier)"),
    ("request_card/draw.rs", "selectable_label(", "a form's fields are inputs, like a text box"),
];

#[test]
fn views_draw_no_controls_the_hosts_draw_them() {
    let mut found = Vec::new();
    for (name, code) in code().iter().filter(|(name, _)| !HOSTS.iter().any(|h| name.starts_with(h))) {
        for (n, line) in lines(code) {
            for token in CONTROLS.iter().filter(|t| line.contains(*t)) {
                if !CONTROLS_ALLOWED.iter().any(|(f, allowed, _)| f == name && line.contains(allowed)) {
                    found.push(format!("{name}:{n}: {token}"));
                }
            }
        }
    }
    assert!(found.is_empty(), "controls drawn outside the hosts (give the view an action with a Control, the host draws it):\n{}", found.join("\n"));
}

#[test]
fn text_naming_a_key_is_caught_and_words_are_not() {
    assert_eq!(literals(r#"ui.label("a" ); f(x, "Esc closes", '"')"#), ["a", "Esc closes"]);
    for named in ["Esc closes", "Enter sends", "ctrl+Enter", "Shift+Enter new line", "g d", "J / K", "a", "SPC I f", "(Space S a)"] {
        assert!(names_a_key(named).is_some(), "{named}");
    }
    for words in ["Open in a column", "entered", "Put away", "Re-read it", "", "—", "On", "It said last"] {
        assert_eq!(names_a_key(words), None, "{words}");
    }
}
