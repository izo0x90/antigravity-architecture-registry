//! The shell stays split as agreed: no file in `app/` regrows past the size
//! that made it hard to work in.

#[test]
fn the_shell_is_split_as_agreed() {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, usize)>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap().lines().count()));
            }
        }
    }
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![("app.rs".to_owned(), std::fs::read_to_string(src.join("app.rs")).unwrap().lines().count())];
    walk(&src.join("app"), &mut files);
    // (1,500 lines: since the code is formatted, 2026-10-26.)
    let big: Vec<_> = files.iter().filter(|(_, n)| *n > 1500).collect();
    assert!(big.is_empty(), "split these further: {big:?}");
}
