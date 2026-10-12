//! The repository's ast-grep rules (sgconfig.yml) hold. Machine-dependent:
//! it needs `ast-grep` (AST_GREP, cargo's user bin folder, or on PATH) and
//! skips, saying so, without it.

use std::process::Command;

#[test]
fn ast_grep_rules_hold_needs_ast_grep() {
    let ast_grep = std::env::var("AST_GREP").unwrap_or_else(|_| {
        let installed = format!("{}/.local/share/cargo/bin/ast-grep", std::env::var("HOME").unwrap_or_default());
        if std::path::Path::new(&installed).exists() { installed } else { "ast-grep".to_owned() }
    });
    let output = match Command::new(&ast_grep).arg("scan").current_dir("..").output() {
        Ok(output) => output,
        Err(e) => return eprintln!("skipped: {ast_grep} can't be run ({e})"),
    };
    assert!(
        output.status.success(),
        "Structural lint violation detected by ast-grep:\nSTDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
