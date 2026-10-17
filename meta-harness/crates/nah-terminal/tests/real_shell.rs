//! Real shells in real pseudo-terminals: what's typed runs, what it prints
//! is on the screen as the program asked for it, and what happens in it
//! is told.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_contracts::{Colour, Mods, TermKey, TermSize, TerminalInput};
use nah_terminal::{Happening, Options, Terminal};

struct Shell {
    terminal: Terminal,
    happened: Arc<Mutex<Vec<Happening>>>,
    _dir: tempfile::TempDir,
}

fn open(shell: &str, cols: u16, rows: u16) -> Shell {
    let dir = tempfile::tempdir().unwrap();
    let happened = Arc::new(Mutex::new(Vec::new()));
    let seen = happened.clone();
    let options = Options { shell: shell.into(), folder: dir.path().into(), size: TermSize { cols, rows }, hooks: dir.path().join("hooks"), history: 1000 };
    let terminal = Terminal::open(options, move |h| seen.lock().unwrap().push(h)).unwrap();
    Shell { terminal, happened, _dir: dir }
}

impl Shell {
    fn typed(&self, text: &str) {
        self.terminal.input(&TerminalInput::Text { text: text.into() }).unwrap();
    }

    fn run(&self, line: &str) {
        self.typed(line);
        self.terminal.input(&TerminalInput::Key { key: TermKey::Enter, mods: Mods::default() }).unwrap();
    }

    /// Waits (up to 10 s) until `ok` holds of its text (with 1000 lines back).
    fn until(&self, what: &str, ok: impl Fn(&str) -> bool) -> String {
        let start = Instant::now();
        loop {
            let text = self.terminal.text(1000);
            if ok(&text) {
                return text;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "waited for {what}; the screen:\n{text}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn happened(&self, what: &str, ok: impl Fn(&Happening) -> bool) -> Happening {
        let start = Instant::now();
        loop {
            if let Some(h) = self.happened.lock().unwrap().iter().find(|h| ok(h)) {
                return h.clone();
            }
            assert!(start.elapsed() < Duration::from_secs(10), "waited for {what}; happened: {:?}", self.happened.lock().unwrap());
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A line of `text` shows `wanted` (after a prompt, when one's drawn there:
/// keys typed before the prompt put it in front of the output).
fn shows(text: &str, wanted: &str) -> bool {
    text.lines().any(|l| l.trim_start_matches(['#', '$', ' ']) == wanted)
}

#[test]
fn what_is_typed_runs_and_its_output_is_on_the_screen() {
    let sh = open("/bin/sh", 80, 24);
    sh.run("echo hel''lo");
    sh.until("hello", |t| shows(t, "hello"));
    let screen = sh.terminal.screen(None);
    assert!(screen.full && screen.size == TermSize { cols: 80, rows: 24 });
    assert!(screen.rows.len() == 24, "the whole screen, every row");
    sh.happened("a change told", |h| *h == Happening::Changed);
}

#[test]
fn a_burst_of_keys_arrives_in_the_order_typed() {
    let sh = open("/bin/sh", 200, 24);
    let word: String = (0..150).map(|i| char::from(b'a' + (i % 26) as u8)).collect();
    sh.typed("echo ");
    for c in word.chars() {
        sh.typed(&c.to_string());
    }
    sh.run("");
    sh.until("the word echoed whole", |t| shows(t, &word));
}

#[test]
fn a_reader_gets_only_the_rows_changed_since_it_last_looked() {
    let sh = open("/bin/sh", 80, 24);
    sh.run("echo first");
    sh.until("first", |t| shows(t, "first"));
    std::thread::sleep(Duration::from_millis(200));
    let seen = sh.terminal.screen(None).seq;
    sh.run("echo second");
    sh.until("second", |t| shows(t, "second"));
    let update = sh.terminal.screen(Some(seen));
    assert!(!update.full && update.seq > seen);
    assert!(!update.rows.is_empty() && update.rows.len() < 24, "only the rows that changed: {:?}", update.rows.iter().map(|r| r.0).collect::<Vec<_>>());
    assert!(sh.terminal.screen(Some(update.seq)).rows.is_empty(), "nothing new since");
}

#[test]
fn resized_the_program_sees_the_new_size_and_readers_get_the_whole_screen() {
    let sh = open("/bin/sh", 80, 24);
    let seen = sh.terminal.screen(None).seq;
    sh.terminal.resize(TermSize { cols: 40, rows: 10 }).unwrap();
    assert!(sh.terminal.screen(Some(seen)).full, "a resize sends it all again");
    sh.run("stty size");
    sh.until("the new size", |t| shows(t, "10 40"));
}

#[test]
fn ctrl_c_interrupts_what_runs() {
    let sh = open("/bin/sh", 80, 24);
    sh.run("sleep 30; echo slept");
    std::thread::sleep(Duration::from_millis(300));
    sh.terminal.input(&TerminalInput::Key { key: TermKey::Char('c'), mods: Mods { ctrl: true, ..Mods::default() } }).unwrap();
    sh.run("echo after");
    let text = sh.until("the prompt back", |t| shows(t, "after"));
    assert!(!text.contains("\nslept"), "sleep was interrupted");
}

#[test]
fn colours_come_as_the_program_asked_for_them() {
    let sh = open("/bin/sh", 80, 24);
    sh.run(r"printf '\033[31mR\033[38;5;200mI\033[38;2;1;2;3mT\033[1;44mB\033[0m\n'");
    sh.until("the coloured line", |t| shows(t, "RITB"));
    let screen = sh.terminal.screen(None);
    // Its last four runs (a prompt is in front of it when the keys came before the prompt).
    let runs = screen.rows.iter().map(|(_, r)| r).find(|r| shows(&r.iter().map(|x| x.text.as_str()).collect::<String>(), "RITB")).unwrap();
    let styles: Vec<_> = runs[runs.len().saturating_sub(4)..].iter().map(|r| (r.text.as_str(), r.style.fg, r.style.bg, r.style.has(harness_contracts::Style::BOLD))).collect();
    assert_eq!(
        styles,
        [("R", Colour::Named(1), Colour::Default, false), ("I", Colour::Indexed(200), Colour::Default, false), ("T", Colour::Rgb(1, 2, 3), Colour::Default, false), ("B", Colour::Rgb(1, 2, 3), Colour::Named(4), true)]
    );
}

#[test]
fn a_line_keeps_its_number_in_the_scrollback_past_whats_kept_and_through_a_clear() {
    // 1000 lines kept: `seq` prints three times as many.
    let sh = open("/bin/bash", 80, 10);
    sh.run("seq 1 3000");
    sh.until("the last number", |t| shows(t, "3000"));
    let found = sh.terminal.search("2950");
    assert_eq!(found.len(), 1, "{found:?}");
    let number = found[0].line;
    let screen = sh.terminal.screen(None);
    assert!(number < screen.top && number >= screen.oldest, "it's in the scrollback: {number} of {}..{}", screen.oldest, screen.top);
    assert!(screen.top - screen.oldest >= 1000, "what's kept is kept");
    assert!(sh.terminal.scrollback(0, 1).0 == screen.oldest, "older than what's kept: from the oldest kept");
    let line = |n: u64| Some(sh.terminal.scrollback(n, 1)).filter(|(from, _)| *from == n).and_then(|(_, rows)| rows.first().map(|r| r.iter().map(|run| run.text.as_str()).collect::<String>()));
    assert_eq!(line(number).as_deref(), Some("2950"));
    // More output: it keeps its number.
    sh.run("seq 1 100");
    sh.until("the second run", |t| t.contains("\n100\n"));
    assert_eq!(line(number).as_deref(), Some("2950"));
    // `clear` takes the scrollback: no number is given again.
    let top = sh.terminal.screen(None).top;
    sh.run("clear");
    sh.until("the screen cleared", |t| !t.contains("100"));
    let screen = sh.terminal.screen(None);
    assert_eq!(screen.oldest, screen.top, "nothing above the screen");
    assert!(screen.top >= top, "{} after {top}", screen.top);
    assert_eq!(line(number), None, "gone with the clear");
}

#[test]
fn the_shell_ending_is_told_with_its_code() {
    let sh = open("/bin/sh", 80, 24);
    sh.run("exit 3");
    assert_eq!(sh.happened("the end", |h| matches!(h, Happening::Ended(_))), Happening::Ended(Some(3)));
    assert_eq!(sh.terminal.ended(), Some(Some(3)));
}

#[test]
fn bash_marks_its_commands_and_a_finished_one_is_told_with_its_code_and_time() {
    let sh = open("bash", 80, 24);
    sh.run("sleep 0.3; false");
    let finished = sh.happened("the command's end", |h| matches!(h, Happening::CommandFinished { .. }));
    let Happening::CommandFinished { code, ran } = finished else { unreachable!() };
    assert_eq!(code, Some(1));
    assert!(ran >= Duration::from_millis(300), "it ran {ran:?}");
}

#[test]
fn the_title_the_program_sets_is_told() {
    let sh = open("/bin/sh", 80, 24);
    sh.run(r"printf '\033]0;building\007'");
    sh.happened("the title", |h| *h == Happening::Title("building".into()));
    assert_eq!(sh.terminal.title(), "building");
}

/// Closing it hangs up, as closing a terminal window does: the shell and
/// what runs in the foreground end, and bash ends its background jobs (as
/// it does in any terminal).
#[test]
fn closing_it_ends_what_the_shell_runs() {
    let pid_in = |t: &str| t.lines().find_map(|l| l.split("pid=").nth(1)?.trim().parse::<u32>().ok());
    // Running (not gone, not a zombie waiting to be reaped).
    let alive = |pid: u32| {
        let out = std::process::Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().unwrap();
        let stat = String::from_utf8_lossy(&out.stdout);
        !stat.trim().is_empty() && !stat.trim().starts_with('Z')
    };
    for (shell, line) in [("bash", "sleep 100 & echo pid=$!"), ("/bin/sh", "sh -c 'echo pid=$$; exec sleep 100'")] {
        let sh = open(shell, 80, 24);
        sh.run(line);
        let pid = pid_in(&sh.until("the pid", |t| pid_in(t).is_some())).unwrap();
        assert!(alive(pid));
        drop(sh);
        let start = Instant::now();
        while alive(pid) {
            assert!(start.elapsed() < Duration::from_secs(5), "{shell}: `{line}` ({pid}) outlived its terminal");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// The crate knows nothing of the server or the cockpit: the only crate of
/// this workspace it uses is the shared types'.
#[test]
fn it_depends_on_no_workspace_crate_but_the_shared_types() {
    let manifest = include_str!("../Cargo.toml");
    let local: Vec<&str> = manifest.lines().filter(|l| l.contains("path = ")).map(|l| l.split('=').next().unwrap().trim()).collect();
    assert_eq!(local, ["harness-contracts"]);
}

#[test]
fn a_synchronized_update_never_ended_shows_and_its_answers_reach_the_program() {
    let sh = open("/bin/bash", 80, 24);
    // Asks where the cursor is inside an update it never ends, and waits for the answer.
    sh.run(r"printf '\e[?2026h\e[6n'; read -rs -d R pos; echo; echo answered ${pos:2}");
    // "answered 24;1": the row and column it said.
    let said = |l: &str| l.strip_prefix("answered ").and_then(|p| p.split_once(';')).is_some_and(|(r, c)| r.parse::<u16>().is_ok() && c.parse::<u16>().is_ok());
    sh.until("the answer, once the update was let go", |t| t.lines().any(said));
    // A program that ends inside one: what it drew shows.
    sh.run(r"printf '\e[?2026hbye\x20inside'; exit");
    sh.happened("the end", |h| matches!(h, Happening::Ended(_)));
    sh.until("what it drew (not the command typed)", |t| t.contains("bye inside"));
}
