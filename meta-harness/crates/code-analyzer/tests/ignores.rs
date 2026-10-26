//! One definition of "ignored": the parent
//! repository's .gitignore, .ignore files, .git/info/exclude, the user's
//! global excludes, the project's .nah/ignore, and hidden paths; the walk
//! and every other caller ask the same `Ignores`. Alone in its binary: it
//! points HOME at its own global excludes.

use std::path::Path;

mod common;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn one_definition_of_ignored_from_every_source_the_walk_included() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path().join("repo");
    let home = dir.path().join("home");
    // The user's global excludes.
    write(&home.join(".gitconfig"), &format!("[core]\n\texcludesFile = {}\n", home.join("global-ignore").display()));
    write(&home.join("global-ignore"), "globally_ignored/\n");
    // SAFETY: this test is alone in its binary; nothing else reads HOME.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    }
    assert!(std::process::Command::new("git").args(["init", "-q"]).arg(&top).status().unwrap().success());
    // The project sits below the repository's top, like meta-harness.
    let proj = top.join("proj");
    write(&top.join(".gitignore"), "target/\n*.log\n!keep.log\n");
    write(&top.join(".git/info/exclude"), "secret/\n");
    write(&proj.join(".ignore"), "scratch/\n");
    write(&proj.join("src/.gitignore"), "local_only.rs\n");
    write(&proj.join(".nah/ignore"), "static/wasm/\n");
    let kept = ["src/lib.rs", "src/keep.log.rs", "src/deep/mod.rs"];
    let ignored = [
        "target/debug/build.rs",
        "secret/key.rs",
        "scratch/try.rs",
        "src/local_only.rs",
        "static/wasm/cockpit.ts",
        "globally_ignored/x.rs",
        ".hidden/h.rs",
    ];
    for f in kept.iter().chain(ignored.iter()) {
        write(&proj.join(f), "pub fn f() {}\n");
    }
    let packs = common::shipped_packs();
    let ignores = code_analyzer::Ignores::new(&proj);
    for f in kept {
        assert!(!ignores.is_ignored(&proj.join(f)), "{f} is not ignored");
    }
    for f in ignored {
        assert!(ignores.is_ignored(&proj.join(f)), "{f} is ignored");
    }
    // A whole ignored directory, and a path that doesn't exist yet in one.
    assert!(ignores.is_ignored(&proj.join("target")));
    assert!(ignores.is_ignored(&proj.join("target/not/yet.rs")));
    assert!(ignores.is_ignored(&proj.join("x.log")) && !ignores.is_ignored(&proj.join("keep.log")));
    // Outside the project: ignored.
    assert!(ignores.is_ignored(&top.join("elsewhere.rs")));
    // The walk asks the same definition.
    let walked: Vec<String> = code_analyzer::walk(&proj, &proj, &packs).unwrap().into_iter().map(|f| f.path.as_str().to_owned()).collect();
    let mut want: Vec<String> = kept.iter().filter(|f| f.ends_with(".rs")).map(|f| (*f).to_owned()).collect();
    want.sort();
    assert_eq!(walked, want);
    // Every file the walk lists, for search (not only those a pack reads).
    let all: Vec<String> =
        ignores.files(&proj).into_iter().map(|p| p.strip_prefix(&proj).unwrap().to_string_lossy().into_owned()).collect();
    assert!(all.iter().all(|f| !ignored.contains(&f.as_str()) && !f.starts_with('.')), "{all:?}");
    assert!(kept.iter().all(|f| all.contains(&(*f).to_owned())), "{all:?}");
    // The hand-made matcher is gone: there is one definition.
    let walk_rs = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/walk.rs")).unwrap();
    assert!(!walk_rs.contains("pub fn ignored("), "the old ignored() is still there");
}

#[test]
fn on_this_repo_the_files_are_gits_less_hidden_ones_and_what_nah_ignore_skips() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let Ok(listed) = std::process::Command::new("git").arg("-C").arg(&root).args(["ls-files", "-co", "--exclude-standard"]).output() else {
        eprintln!("skipped: git isn't installed (machine-dependent)");
        return;
    };
    let skipped: Vec<String> = std::fs::read_to_string(root.join(".nah/ignore"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(str::to_owned)
        .collect();
    let git: std::collections::BTreeSet<String> = String::from_utf8(listed.stdout)
        .unwrap()
        .lines()
        .filter(|p| !p.split('/').any(|s| s.starts_with('.')))
        .filter(|p| !skipped.iter().any(|s| p.starts_with(s.as_str())))
        .filter(|p| root.join(p).is_file())
        .map(str::to_owned)
        .collect();
    let started = std::time::Instant::now();
    let ours: std::collections::BTreeSet<String> = code_analyzer::Ignores::new(&root)
        .files(&root)
        .iter()
        .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().into_owned())
        .collect();
    eprintln!("{} files in {:?}", ours.len(), started.elapsed());
    assert_eq!(ours.difference(&git).collect::<Vec<_>>(), Vec::<&String>::new(), "listed but git ignores them");
    assert_eq!(git.difference(&ours).collect::<Vec<_>>(), Vec::<&String>::new(), "git lists them but we skip them");
}

#[test]
fn in_a_git_worktree_the_repositorys_exclude_still_applies() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("main");
    let git = |args: &[&str], at: &Path| {
        std::process::Command::new("git").arg("-C").arg(at).args(args).output().map(|o| o.status.success()).unwrap_or(false)
    };
    std::fs::create_dir_all(&main).unwrap();
    if !git(&["init", "-q"], &main) {
        eprintln!("skipped: git isn't installed (machine-dependent)");
        return;
    }
    std::fs::write(main.join("a.rs"), "").unwrap();
    assert!(git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "a", "--allow-empty"], &main));
    let worktree = dir.path().join("wt");
    assert!(git(&["worktree", "add", "-q", worktree.to_str().unwrap()], &main));
    std::fs::write(main.join(".git/info/exclude"), "local_only.rs\n").unwrap();
    std::fs::write(worktree.join("local_only.rs"), "").unwrap();
    assert!(code_analyzer::Ignores::new(&worktree).is_ignored(&worktree.join("local_only.rs")), "git ignores it there too");
}

#[test]
fn links_are_never_followed_and_only_the_projects_own_rule_files_count() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj");
    write(&proj.join("src/real.rs"), "");
    write(&proj.join("target/gen.rs"), "");
    write(&proj.join(".gitignore"), "target/\n");
    std::os::unix::fs::symlink(proj.join("src/real.rs"), proj.join("link.rs")).unwrap();
    std::os::unix::fs::symlink(proj.join("src"), proj.join("linkdir")).unwrap();
    std::os::unix::fs::symlink(proj.join("target/gen.rs"), proj.join("src/gen_link.rs")).unwrap();
    let ignores = code_analyzer::Ignores::new(&proj);
    let listed: Vec<String> = ignores.files(&proj).iter().map(|p| p.strip_prefix(&proj).unwrap().to_string_lossy().into_owned()).collect();
    for path in ["link.rs", "linkdir/real.rs", "src/gen_link.rs"] {
        assert!(!listed.contains(&path.to_owned()), "{path} is listed");
        assert!(ignores.is_ignored(&proj.join(path)), "{path}: a link isn't followed, so it's as if it weren't there");
    }
    assert_eq!(listed, ["src/real.rs"]);
    // Rule files: the project's own, not one with the same name elsewhere.
    assert!(ignores.rules_scope(&proj.join(".nah/ignore")).is_some());
    assert!(ignores.rules_scope(&proj.join("sub/.nah/ignore")).is_none());
    assert!(ignores.rules_scope(&proj.join("vendored/.git/info/exclude")).is_none());
    // In an ignored folder, a rule file changes nothing that's read.
    assert!(ignores.rules_scope(&proj.join("target/.gitignore")).is_none());
}

#[test]
fn deciding_what_is_ignored_costs_little_more_than_listing_the_folders() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    fn list(dir: &Path, out: &mut usize) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            *out += 1;
            let kind = entry.file_type().unwrap();
            let name = entry.file_name();
            // The same folders the rules leave out (as a bare walk would
            // never know): hidden ones.
            if kind.is_dir() && !name.to_string_lossy().starts_with('.') {
                list(&entry.path(), out);
            }
        }
    }
    let mut best_bare = std::time::Duration::MAX;
    let mut best_ours = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let mut n = 0;
        list(&root, &mut n);
        best_bare = best_bare.min(t.elapsed());
        let ignores = code_analyzer::Ignores::new(&root);
        let t = std::time::Instant::now();
        ignores.files(&root);
        best_ours = best_ours.min(t.elapsed());
    }
    eprintln!("bare {best_bare:?}, with the rules {best_ours:?}");
    assert!(best_ours <= best_bare * 10 + std::time::Duration::from_millis(5), "deciding costs {best_ours:?}, listing {best_bare:?}");
}
