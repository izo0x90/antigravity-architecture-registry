use std::process::Command;

#[test]
fn test_ast_grep_rules_conformance() {
    let ast_grep_bin = std::env::var("AST_GREP").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_default();
        let fallback = format!("{}/.local/share/cargo/bin/ast-grep", home);
        if std::path::Path::new(&fallback).exists() {
            fallback
        } else {
            "ast-grep".to_string()
        }
    });

    let output = Command::new(ast_grep_bin)
        .arg("scan")
        .current_dir("..")
        .output()
        .expect("Failed to execute ast-grep scan.");

    assert!(
        output.status.success(),
        "Structural lint violation detected by ast-grep:\nSTDOUT:\n{}\nSTDERR:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
