//! The cockpit's frames in a real browser (scripts/frame_check.py).
//! Machine-dependent: a Chrome on the DevTools protocol, and a server it can
//! open (NAH_FRAME_CHECK_URL, the cockpit's URL with its token, serving the
//! big repo). Without them it says why it skipped, and passes.

/// scripts/frame_check.py's exit code when there's no Chrome to check with.
const SKIPPED: i32 = 77;

#[test]
fn no_frame_takes_longer_than_the_limit_while_panning_every_view_needs_chrome() {
    let Ok(url) = std::env::var("NAH_FRAME_CHECK_URL") else {
        return eprintln!("skipped: NAH_FRAME_CHECK_URL (the cockpit's URL) isn't set");
    };
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/frame_check.py");
    let out = match std::process::Command::new("uv").args(["run", "--with", "websockets", "python"]).arg(&script).args(["--url", &url]).output() {
        Ok(out) => out,
        Err(e) => return eprintln!("skipped: uv can't be run ({e})"),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.code() == Some(SKIPPED) {
        return eprintln!("{text}");
    }
    println!("{text}{}", String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.lines().count() == 3, "every view, each frame within the limit:\n{text}");
}
