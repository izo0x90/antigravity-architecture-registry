//! The cockpit's actions and keys, pinned (see the test).

use super::*;

/// Every action and its keys, with each view focused, pinned in
/// `tests/snapshots/keys.txt`: refactors leave them unchanged; a change
/// that means to alter them rewrites the file on purpose
/// (`UPDATE_SNAPSHOTS=1`).
#[test]
fn the_actions_and_their_keys_are_as_pinned() {
    let (ctx, mut app) = full_app();
    let mut out = String::new();
    for kind in BufferKind::ALL {
        focus(&ctx, &mut app, kind);
        out.push_str(&format!("## {} focused\n", kind.name()));
        for b in app.bound_actions() {
            let origin = match b.origin {
                Origin::Shell => "shell",
                Origin::View(k) => k.name(),
            };
            out.push_str(&format!("{origin} {} [{}]\n", b.action.id, b.key_hint()));
        }
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/keys.txt");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &out).unwrap();
        return;
    }
    let pinned =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e} (run with UPDATE_SNAPSHOTS=1 to create it)", path.display()));
    if let Some((line, (was, now))) = pinned.lines().zip(out.lines()).enumerate().find(|(_, (a, b))| a != b) {
        panic!("keys changed at line {}:\n  pinned: {was}\n  now:    {now}\n(if intended, rerun with UPDATE_SNAPSHOTS=1)", line + 1);
    }
    assert_eq!(pinned.lines().count(), out.lines().count(), "actions were added or removed (if intended, rerun with UPDATE_SNAPSHOTS=1)");
}
