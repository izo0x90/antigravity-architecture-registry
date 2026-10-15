//! Nothing reads keys behind the keymap's back: outside it, no code names a
//! key or a modifier, or asks whether one is down. Every key is a binding
//! (or a list, typing or pointer key) the config sets.

/// Allowed by name, and why.
const EXCEPTIONS: &[(&str, &str, &str)] = &[
    // The agents' "press ctrl+enter" turned into input: it names keys to make them.
    ("remote.rs", "", "the agents' key names, parsed into input"),
    // A typed prompt (search, hint labels, marks) erases as any text box does.
    ("app/keys.rs", "egui::Key::Backspace", "a typed prompt's own erasing"),
];

const FORBIDDEN: &[&str] = &["egui::Key::", "Key::from_name", "key_pressed(egui", "key_released(egui", "key_down(egui", "modifiers.ctrl", "modifiers.command", "modifiers.shift", "modifiers.alt", "modifiers.mac_cmd"];

fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "tests") {
                sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") && path.file_name().is_some_and(|n| n != "tests.rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_code_reads_a_key_the_keymap_doesnt_give_it() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut found = Vec::new();
    for file in files {
        let name = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if name.starts_with("keymap/") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        // Its own tests make input as a user would.
        let code = text.split("#[cfg(test)]\nmod tests").next().unwrap_or(&text);
        for (n, line) in code.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for token in FORBIDDEN.iter().filter(|t| line.contains(*t)) {
                let excused = EXCEPTIONS.iter().any(|(f, what, _)| *f == name && (what.is_empty() || line.contains(what)));
                if !excused {
                    found.push(format!("{name}:{}: {token}", n + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "keys read outside the keymap (bind them in [keymap] instead):\n{}", found.join("\n"));
}
