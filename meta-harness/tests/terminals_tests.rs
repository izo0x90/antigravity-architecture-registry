//! Terminals through the server: opened by the user, listed on the event
//! stream (never their screens), each one's screen and typing on its own
//! socket (also through home), shown at the smallest size it's shown at;
//! agents read them, and type only when their role allows.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{COCKPIT_TOKEN, Mcp, http, spawn_app};
use futures::{SinkExt, StreamExt};
use harness_contracts::{FromTerminal, Mods, ReaderScreen, ScreenUpdate, TermKey, TermSize, Terminal, TerminalInput, ToTerminal};
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: SocketAddr) -> Terminal {
    let (status, body) = http(addr, "POST", "/api/terminals", Some("{}")).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

async fn connect(addr: SocketAddr, path: &str) -> Socket {
    let ticket = common::ticket(addr).await;
    tokio_tungstenite::connect_async(format!("ws://{addr}{path}?ticket={ticket}")).await.unwrap().0
}

/// A reader of a terminal: its screen as it has it, kept from what it's sent.
struct Reader {
    socket: Socket,
    screen: ReaderScreen,
    /// Its screen's rows' text.
    rows: Vec<String>,
    last: Option<ScreenUpdate>,
    /// Lines of the scrollback it has, by number: those that scrolled off
    /// its screen (as it had them, or as sent again when they'd changed),
    /// and those asked for.
    lines: std::collections::BTreeMap<u64, String>,
    /// The commands it was told of, by number.
    commands: std::collections::BTreeMap<u64, harness_contracts::TerminalCommand>,
    /// Rows the server sent again (they changed before they scrolled off).
    sent_again: Vec<String>,
}

fn text(row: &harness_contracts::Row) -> String {
    row.iter().map(|run| run.text.as_str()).collect()
}

impl Reader {
    async fn at(addr: SocketAddr, path: &str) -> Self {
        Self { socket: connect(addr, path).await, screen: ReaderScreen::default(), rows: Vec::new(), last: None, lines: Default::default(), commands: Default::default(), sent_again: Vec::new() }
    }

    async fn say(&mut self, said: ToTerminal) {
        self.socket.send(Message::Text(serde_json::to_string(&said).unwrap().into())).await.unwrap();
    }

    async fn run(&mut self, line: &str) {
        self.say(ToTerminal::Input { input: TerminalInput::Text { text: line.into() } }).await;
        self.say(ToTerminal::Input { input: TerminalInput::Key { key: TermKey::Enter, mods: Mods::default() } }).await;
    }

    /// The next thing it's sent (a screen update is applied to its rows).
    async fn next(&mut self) -> FromTerminal {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(10), self.socket.next()).await.expect("sent something within 10 s").unwrap().unwrap();
            let Message::Text(json) = message else { continue };
            let said: FromTerminal = serde_json::from_str(&json).unwrap();
            match &said {
                FromTerminal::Screen { update } => {
                    let (from, gone) = self.screen.take(update);
                    for (n, row) in (from..).zip(&gone) {
                        self.lines.entry(n).or_insert_with(|| text(row));
                    }
                    self.rows = self.screen.rows().iter().map(text).collect();
                    self.commands.extend(update.commands.iter().map(|c| (c.id, c.clone())));
                    self.last = Some(update.clone());
                }
                FromTerminal::Lines { from, rows } => self.lines.extend((*from..).zip(rows.iter().map(text))),
                FromTerminal::ScrolledOff { lines } => {
                    self.sent_again.extend(lines.iter().map(|(_, row)| text(row)));
                    self.lines.extend(lines.iter().map(|(n, row)| (*n, text(row))));
                }
                FromTerminal::Found { .. } | FromTerminal::CommandOutput { .. } | FromTerminal::LineNow { .. } => {}
            }
            return said;
        }
    }

    /// Reads what it was sent until nothing more comes for a while.
    async fn drain(&mut self) {
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(300), self.socket.next()).await {}
    }

    /// Reads until a row shows `wanted` (after a prompt, if one's there).
    async fn until_shows(&mut self, wanted: &str) {
        let start = std::time::Instant::now();
        while !self.rows.iter().any(|r| r.trim_start_matches(['#', '$', ' ']) == wanted) {
            assert!(start.elapsed() < Duration::from_secs(10), "never showed {wanted:?}: {:?}", self.rows);
            self.next().await;
        }
    }
}

#[tokio::test]
async fn the_user_opens_one_types_in_it_and_every_reader_sees_the_same() {
    let (addr, _) = spawn_app().await;
    let mut events = common::events_socket(addr).await;
    let terminal = open(addr).await;
    let (status, body) = http(addr, "GET", "/api/terminals", None).await;
    assert!(status == 200 && body.contains(&format!("\"id\":\"{}\"", terminal.id)), "{body}");

    let path = format!("/ws/terminals/{}", terminal.id);
    let mut first = Reader::at(addr, &path).await;
    let FromTerminal::Screen { update } = first.next().await else { panic!("the screen comes first") };
    assert!(update.full, "the whole screen first");
    first.run("echo typed-here").await;
    first.until_shows("typed-here").await;

    // Another reader gets the whole screen as it is now.
    let mut second = Reader::at(addr, &path).await;
    second.next().await;
    assert!(second.rows.iter().any(|r| r.ends_with("typed-here")), "{:?}", second.rows);
    // And what changes after reaches both.
    second.run("echo from-the-second").await;
    second.until_shows("from-the-second").await;
    first.until_shows("from-the-second").await;

    // The event stream said it exists (and never carried its screen).
    let mut said = String::new();
    while let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(Duration::from_millis(300), events.next()).await {
        said.push_str(&text);
    }
    assert!(said.contains("terminal_updated"), "{said}");
    assert!(!said.contains("typed-here"), "screens aren't on the event stream");
}

#[tokio::test]
async fn shown_at_two_sizes_the_smallest_wins_until_that_reader_goes() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let path = format!("/ws/terminals/{}", terminal.id);
    let size = |addr| async move {
        let (_, body) = http(addr, "GET", "/api/terminals", None).await;
        serde_json::from_str::<Vec<Terminal>>(&body).unwrap()[0].size
    };
    let mut big = Reader::at(addr, &path).await;
    big.next().await;
    big.say(ToTerminal::Size { size: TermSize { cols: 120, rows: 40 } }).await;
    let mut small = Reader::at(addr, &path).await;
    small.next().await;
    small.say(ToTerminal::Size { size: TermSize { cols: 70, rows: 20 } }).await;
    small.run("stty size").await;
    small.until_shows("20 70").await;
    assert_eq!(size(addr).await, TermSize { cols: 70, rows: 20 });
    drop(small);
    let start = std::time::Instant::now();
    while size(addr).await != (TermSize { cols: 120, rows: 40 }) {
        assert!(start.elapsed() < Duration::from_secs(10), "back to the one reader's size");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_reader_is_sent_lines_as_they_scroll_off_and_asks_for_scrollback_and_searches_it() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.say(ToTerminal::Size { size: TermSize { cols: 80, rows: 10 } }).await;
    reader.run("seq 1 300").await;
    reader.until_shows("300").await;
    // What scrolled off its screen it has, unasked, as the server has it.
    let had = reader.lines.clone();
    assert!(!had.is_empty());
    let (first, last) = (*had.keys().next().unwrap(), *had.keys().last().unwrap());
    reader.say(ToTerminal::Lines { from: first, count: (last - first + 1) as u32 }).await;
    while !matches!(reader.next().await, FromTerminal::Lines { .. }) {}
    assert!(had.iter().all(|(n, line)| reader.lines.get(n) == Some(line)), "{had:?}\n{:?}", reader.lines);
    reader.say(ToTerminal::Search { query: "150".into() }).await;
    let found = loop {
        if let FromTerminal::Found { found, .. } = reader.next().await {
            break found;
        }
    };
    assert_eq!(found.len(), 1, "{found:?}");
    reader.say(ToTerminal::Lines { from: found[0].line, count: 2 }).await;
    let rows = loop {
        if let FromTerminal::Lines { from, rows } = reader.next().await
            && from == found[0].line
        {
            break rows;
        }
    };
    let text: Vec<String> = rows.iter().map(|r| r.iter().map(|run| run.text.as_str()).collect()).collect();
    assert_eq!(text, ["150", "151"]);
    // A line it saw half printed, finished and scrolled off in one frame:
    // it has the whole of it. (The half is seen, not assumed: the command
    // waits for a key, silently, before one write finishes the line and
    // scrolls it off.)
    reader.run("printf 'half'; read -s x; printf ' and whole\\n%s\\n' $(seq 400 430)").await;
    reader.until_shows("half").await;
    reader.say(ToTerminal::Input { input: TerminalInput::Key { key: TermKey::Enter, mods: Mods::default() } }).await;
    reader.until_shows("430").await;
    let halves: Vec<&String> = reader.lines.values().filter(|l| l.starts_with("half")).collect();
    assert_eq!(halves, ["half and whole"], "{:?}", reader.lines);
    assert!(reader.sent_again.iter().any(|l| l == "half and whole"), "sent again as it went: {:?}", reader.sent_again);
}

/// A line is named by the start of the shell and its number then; after a
/// new width lays the lines out anew, the server says where it is now
/// (exactly, as it moves a tag's), and the reader doesn't work it out.
#[tokio::test]
async fn a_line_is_found_where_it_is_after_a_new_width() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.say(ToTerminal::Size { size: TermSize { cols: 80, rows: 10 } }).await;
    reader.run("for i in $(seq 1 40); do printf '%s %s\\n' $i 0123456789012345678901234567890123456789012345678901234567890123456789; done").await;
    reader.until_shows("40 0123456789012345678901234567890123456789012345678901234567890123456789").await;
    let shell = reader.last.as_ref().unwrap().shell;
    reader.say(ToTerminal::Search { query: "20 0123".into() }).await;
    let line = loop {
        if let FromTerminal::Found { found, .. } = reader.next().await {
            break found[0].line;
        }
    };
    // Narrower: each 72-character line wraps to two; line 20 is numbered anew.
    reader.say(ToTerminal::Size { size: TermSize { cols: 40, rows: 10 } }).await;
    loop {
        if let FromTerminal::Screen { update } = reader.next().await
            && update.size.cols == 40
        {
            break;
        }
    }
    reader.say(ToTerminal::LineNow { shell, first: line, last: line }).await;
    let now = loop {
        if let FromTerminal::LineNow { shell: s, first, now, .. } = reader.next().await
            && (s, first) == (shell, line)
        {
            break now;
        }
    };
    let (now_first, _) = now.expect("followed");
    assert_ne!(now_first, line, "numbered anew");
    reader.say(ToTerminal::Lines { from: now_first, count: 1 }).await;
    let rows = loop {
        if let FromTerminal::Lines { from, rows } = reader.next().await
            && from == now_first
        {
            break rows;
        }
    };
    assert!(text(&rows[0]).starts_with("20 0123"), "{:?}", text(&rows[0]));
    // Another start of the shell: not followed.
    reader.say(ToTerminal::LineNow { shell: shell + 1, first: line, last: line }).await;
    let other = loop {
        if let FromTerminal::LineNow { shell: s, now, .. } = reader.next().await
            && s == shell + 1
        {
            break now;
        }
    };
    assert_eq!(other, None);
}

#[tokio::test]
async fn closing_one_ends_it_and_says_so() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut events = common::events_socket(addr).await;
    let (status, _) = http(addr, "DELETE", &format!("/api/terminals/{}", terminal.id), None).await;
    assert_eq!(status, 204);
    let (_, body) = http(addr, "GET", "/api/terminals", None).await;
    assert_eq!(body, "[]");
    let mut said = String::new();
    while let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout(Duration::from_millis(500), events.next()).await {
        said.push_str(&text);
    }
    assert!(said.contains("terminal_closed"), "{said}");
}

#[tokio::test]
async fn an_ended_shell_is_started_again_in_the_same_terminal() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("exit 4").await;
    let ended = |addr| async move {
        let (_, body) = http(addr, "GET", "/api/terminals", None).await;
        body.contains(r#""state":"ended","code":4"#)
    };
    let start = std::time::Instant::now();
    while !ended(addr).await {
        assert!(start.elapsed() < Duration::from_secs(10), "its end is said, with its code");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    reader.drain().await;
    let (status, body) = http(addr, "POST", &format!("/api/terminals/{}/restart", terminal.id), Some("{}")).await;
    assert!(status == 200 && body.contains(r#""state":"running""#), "{body}");
    // The reader still there gets the new shell's whole screen.
    while !matches!(reader.next().await, FromTerminal::Screen { update } if update.full) {}
    reader.run("echo again").await;
    reader.until_shows("again").await;
    let mut again = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    again.next().await;
    again.until_shows("again").await;
}

#[tokio::test]
async fn through_home_the_socket_reaches_the_servers_terminal() {
    let (worker, _) = spawn_app().await;
    let (home, _) = spawn_app().await;
    let (status, body) = http(home, "POST", "/api/home/servers", Some(&json!({ "name": "w1", "url": format!("http://{worker}"), "secret": COCKPIT_TOKEN }).to_string())).await;
    assert_eq!(status, 201, "{body}");
    let (status, body) = http(home, "POST", "/s/w1/api/terminals", Some("{}")).await;
    assert_eq!(status, 200, "{body}");
    let terminal: Terminal = serde_json::from_str(&body).unwrap();
    let mut reader = Reader::at(home, &format!("/s/w1/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("echo via-home").await;
    reader.until_shows("via-home").await;
}

#[tokio::test]
async fn agents_read_terminals_and_type_only_with_shell_drive() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("echo for-the-agent").await;
    reader.until_shows("for-the-agent").await;

    let mut lead = Mcp::lead(addr).await;
    let (error, listed) = lead.tool("terminals", json!({})).await;
    assert!(!error && listed.contains(terminal.id.as_str()), "{listed}");
    let (error, text) = lead.tool("terminal_read", json!({ "terminal": terminal.id })).await;
    assert!(!error && text.contains("for-the-agent"), "{text}");
    // The shell's commands, when it says them (bash and zsh do).
    assert!(["bash", "zsh"].iter().any(|s| terminal.shell.ends_with(s)), "commands are marked by bash and zsh: run these tests with SHELL one of them, not {}", terminal.shell);
    let start = std::time::Instant::now();
    while !reader.commands.values().any(|c| c.text == "echo for-the-agent" && c.end.is_some()) {
        assert!(start.elapsed() < Duration::from_secs(10), "the reader is told of it: {:?}", reader.commands);
        reader.next().await;
    }
    let (_, text) = lead.tool("terminal_read", json!({ "terminal": terminal.id })).await;
    let line = text.lines().find(|l| l.contains("$ echo for-the-agent (exit code 0")).unwrap_or_else(|| panic!("named with its end: {text}"));
    let number: u64 = line.split('.').next().unwrap().parse().unwrap();
    let (error, output) = lead.tool("terminal_read", json!({ "terminal": terminal.id, "command": number })).await;
    assert!(!error && output.lines().nth(1) == Some("for-the-agent"), "{output}");
    let tools = lead.rpc("tools/list", json!({})).await;
    assert!(!tools.to_string().contains("terminal_type"), "not offered to a role without shell.drive");
    let (error, said) = lead.tool("terminal_type", json!({ "terminal": terminal.id, "text": "echo nope", "keys": "enter" })).await;
    assert!(error, "refused: {said}");
}

#[tokio::test]
async fn a_bell_and_a_shells_end_are_notifications_put_away_once_the_terminal_is_seen() {
    let (addr, _) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run(r"printf 'ding\a\n'").await;
    reader.until_shows("ding").await;
    let open_ones = |addr| async move {
        let (_, body) = http(addr, "GET", "/api/notifications", None).await;
        serde_json::from_str::<Vec<harness_contracts::Notification>>(&body).unwrap()
    };
    let start = std::time::Instant::now();
    let bell = loop {
        if let Some(n) = open_ones(addr).await.into_iter().find(|n| n.kind == harness_contracts::NotificationKind::TerminalBell) {
            break n;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "the bell became a notification");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(bell.terminal.as_ref(), Some(&terminal.id), "about its terminal");
    let (status, body) = http(addr, "POST", "/api/notifications/seen", Some(&json!({ "terminal": terminal.id }).to_string())).await;
    assert!(status == 200 && body.contains(r#""dismissed":1"#), "{body}");
    assert!(open_ones(addr).await.iter().all(|n| n.terminal.is_none()), "seen: put away");
    reader.run("exit 3").await;
    let start = std::time::Instant::now();
    while !open_ones(addr).await.iter().any(|n| n.kind == harness_contracts::NotificationKind::ShellEnded && n.detail.as_deref() == Some("exit code 3")) {
        assert!(start.elapsed() < Duration::from_secs(10), "its end is told, with its code");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The events sent on `events` until one `matches` (within 10 s).
async fn event_until(events: &mut Socket, matches: impl Fn(&harness_contracts::ClientEvent) -> bool) -> harness_contracts::ClientEvent {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), events.next()).await.expect("an event within 10 s").unwrap().unwrap();
        let Message::Text(json) = message else { continue };
        if let Ok(event) = serde_json::from_str::<harness_contracts::ClientEvent>(&json)
            && matches(&event)
        {
            return event;
        }
    }
}

#[tokio::test]
async fn a_terminals_lines_and_commands_are_tagged_by_the_user_and_by_agents_whose_role_may() {
    let (addr, state) = spawn_app().await;
    let mut events = common::events_socket(addr).await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("echo tagged-line").await;
    reader.until_shows("tagged-line").await;
    let screen = reader.last.clone().unwrap();
    let line = screen.top + reader.rows.iter().position(|r| r.trim_start_matches(['#', '$', ' ']) == "tagged-line").unwrap() as u64;

    // Commands ran, nothing tagged: nothing logged.
    let tagged_dir = state.project.arch.root().join(".nah/state/tagged");
    assert!(std::fs::read_dir(&tagged_dir).map_or(true, |d| d.count() == 0), "nothing logged untagged");
    // The user tags the line: a new tag, told to everyone.
    let subject = json!({ "kind": "terminal_lines", "terminal": terminal.id, "shell": screen.shell, "first": line, "last": line });
    let (status, body) = http(addr, "POST", "/api/tags", Some(&json!({ "tag": "seen-it", "subject": subject }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let tag: harness_contracts::Tag = serde_json::from_str(&body).unwrap();
    assert_eq!((tag.name.as_str(), tag.on[0].by.as_str()), ("seen-it", "user"), "who tagged it");
    assert!(tag.on[0].kept.is_some() && !body.contains("tagged-line"), "a tag only points (what it said is in the tag logs): {body}");
    let logs = state.project.arch.root().join(".nah/state/tagged");
    let logged = std::fs::read_dir(&logs).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap()).collect::<String>();
    assert!(logged.contains("tagged by user") && logged.contains("tagged-line"), "{logged}");
    event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id)).await;
    let (status, body) = http(addr, "POST", "/api/tags", Some(&json!({ "tag": "two words", "subject": subject }).to_string())).await;
    assert!(status == 400 && body.contains("can't be a tag's name"), "{status} {body}");
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains(&format!("\"what\":\"terminal {} line {line} (of command ", terminal.id)) && read.contains("\"held\":\"source\"") && read.contains("\"text\":\"tagged-line"), "what it says now, named: {read}");
    // A narrower reader lays the lines out anew: the tag follows its line,
    // told to everyone as it moves (not only when it's next read).
    reader.say(ToTerminal::Size { size: TermSize { cols: 40, rows: 20 } }).await;
    while !matches!(reader.next().await, FromTerminal::Screen { update } if update.size.cols == 40) {}
    let moved = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id && t.on[0].subject != tag.on[0].subject)).await;
    let harness_contracts::ClientEvent::TagUpdated { tag: moved } = moved else { unreachable!() };
    let harness_contracts::Subject::TerminalLines { first, .. } = moved.on[0].subject else { panic!("{moved:?}") };
    assert!(moved.on[0].what.starts_with(&format!("terminal {} line {first} (of command ", terminal.id)), "named where it is now, with its command: {}", moved.on[0].what);
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains("\"held\":\"source\"") && read.contains("\"text\":\"tagged-line"), "still found, numbered anew: {read}");
    // Many new widths in a row (more than the terminal remembers): still followed.
    for cols in 41..48 {
        reader.say(ToTerminal::Size { size: TermSize { cols, rows: 20 } }).await;
    }
    while !matches!(reader.next().await, FromTerminal::Screen { update } if update.size.cols == 47) {}
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains("\"held\":\"source\"") && read.contains("\"text\":\"tagged-line"), "still found after seven: {read}");

    // An agent whose role may (the lead's) tags a command, by its number.
    let mut lead = Mcp::lead(addr).await;
    assert!(["bash", "zsh"].iter().any(|s| terminal.shell.ends_with(s)), "commands are marked by bash and zsh: run these tests with SHELL one of them, not {}", terminal.shell);
    while !reader.commands.values().any(|c| c.text == "echo tagged-line" && c.end.is_some()) {
        reader.next().await;
    }
    let number = reader.commands.values().find(|c| c.text == "echo tagged-line").unwrap().id;
    let command = format!("terminal {} command {number}", terminal.id);
    let (error, said) = lead.tool("tag", json!({ "tag": "seen-it", "subject": command })).await;
    assert!(!error && said.starts_with("seen-it (on 2 things)"), "{said}");
    let (_, read) = lead.tool("tag_read", json!({ "tag": "seen-it" })).await;
    assert!(read.contains(&format!("2. {command} (echo tagged-line) (tagged by lead; as it is now):")) && read.contains("$ echo tagged-line (exit code 0"), "named with what was typed: {read}");
    assert!(read.contains("(tagged by the user; as it is now)"), "the user's, to an agent: {read}");
    // Taken off by the name tag_read gives; again: it isn't on it, and says so.
    let (error, said) = lead.tool("untag", json!({ "tag": "seen-it", "subject": command })).await;
    assert!(!error && said.starts_with("seen-it (on 1 thing)"), "{said}");
    let (error, said) = lead.tool("untag", json!({ "tag": "seen-it", "subject": command })).await;
    assert!(error && said.contains("isn't on that"), "{said}");
    let (error, said) = lead.tool("tag_edit", json!({ "tag": "seen-it", "name": "checked", "note": "the echo works" })).await;
    assert!(!error && said.contains("checked") && said.contains("the echo works"), "{said}");
    // A prompt linking the tag hands the agent what it's on, as it is now;
    // the same words not linked are text.
    let (_, tags) = http(addr, "GET", "/api/tags", None).await;
    let checked = serde_json::from_str::<serde_json::Value>(&tags).unwrap().as_array().unwrap().iter().find(|t| t["name"] == "checked").unwrap()["id"].as_str().unwrap().to_owned();
    let link = json!({ "start": 8, "end": 16, "to": { "kind": "tag", "tag": checked } });
    let (status, body) = http(addr, "POST", "/api/slots/lead/prompt", Some(&json!({ "text": "look at #checked.", "links": [link] }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let echoed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("What's linked above"))).await;
    let echoed = format!("{echoed:?}");
    assert!(echoed.contains("#checked (the echo works; noted by lead)") && echoed.contains("tagged-line"), "an agent's note says whose: {echoed}");
    let (status, _) = http(addr, "POST", "/api/slots/lead/prompt", Some(&json!({ "text": "plain #checked, not linked" }).to_string())).await;
    assert_eq!(status, 200);
    let plain = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("plain #checked"))).await;
    assert!(!format!("{plain:?}").contains("tagged-line"), "not linked: nothing handed on: {plain:?}");
    // One thing linked as a pointer: its start, and the tool that reads the rest.
    let subject = json!({ "kind": "terminal_command", "terminal": terminal.id, "shell": reader.last.as_ref().unwrap().shell, "command": number });
    let link = json!({ "start": 0, "end": 4, "to": { "kind": "thing", "subject": subject }, "include": "pointer" });
    let (status, body) = http(addr, "POST", "/api/slots/lead/prompt", Some(&json!({ "text": "this one", "links": [link] }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let pointed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("this one"))).await;
    assert!(format!("{pointed:?}").contains("its start only: `thing_read`"), "{pointed:?}");
    // Checked before it's sent: what sending would say (small: nothing).
    let whole = json!({ "start": 0, "end": 4, "to": { "kind": "thing", "subject": subject }, "include": "whole" });
    let (status, body) = http(addr, "POST", "/api/links/check", Some(&json!({ "text": "this one", "links": [whole] }).to_string())).await;
    assert_eq!((status, body.as_str()), (200, r#"{"notes":[]}"#));
    // A link to a tag deleted since: said before it's sent.
    let gone = json!({ "start": 0, "end": 5, "to": { "kind": "tag", "tag": "no-such-tag" } });
    let (_, body) = http(addr, "POST", "/api/links/check", Some(&json!({ "text": "#gone it", "links": [gone] }).to_string())).await;
    assert!(body.contains("#gone: gone since it was linked"), "{body}");
    let (error, read) = lead.tool("thing_read", json!({ "thing": command })).await;
    assert!(!error && read.contains("tagged-line"), "{read}");
    // An agent's report to the lead is plain text: nothing in it is linked.
    let (err, text) = lead.tool("slot_start", json!({ "harness": "fake", "name": "helper" })).await;
    assert!(!err, "{text}");
    let sub = state.harness.slots.list().into_iter().find(|s| s.name == "helper").unwrap();
    let thread = state.harness.slots.get(&sub.id).and_then(|s| s.current_thread().cloned()).expect("its session runs");
    let mut helper = Mcp::as_session(addr, &thread).await;
    let (err, text) = helper.tool("tell_lead", json!({ "text": "fixed #checked" })).await;
    assert!(!err, "{text}");
    let echoed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("fixed #checked"))).await;
    assert!(!format!("{echoed:?}").contains("tagged-line"), "an agent's text links nothing: {echoed:?}");
    // An agent with no key reads nothing, and changes nothing.
    let mut external = Mcp::connect(addr).await;
    let (error, said) = external.tool("tag_read", json!({ "tag": "checked" })).await;
    assert!(error && said.contains("view.read"), "refused: {said}");
    let (error, said) = external.tool("tag_edit", json!({ "tag": "checked", "delete": true })).await;
    assert!(error && said.contains("tags.edit"), "refused: {said}");
    // Its shell started again: what it's on is an earlier start's, named so
    // (as it's taken off by), and gone from there (as it was, from the logs).
    let (status, body) = http(addr, "POST", &format!("/api/terminals/{}/restart", terminal.id), None).await;
    assert_eq!(status, 200, "{body}");
    let renamed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id && t.on[0].what.contains('@'))).await;
    let (_, read) = lead.tool("tag_read", json!({ "tag": "checked" })).await;
    assert!(read.contains(&format!("1. terminal {}@", terminal.id)) && read.contains("as it was when noted: changed or gone since):\ntagged-line"), "{read}");
    let harness_contracts::ClientEvent::TagUpdated { tag: renamed } = renamed else { unreachable!() };
    let (error, said) = lead.tool("untag", json!({ "tag": "checked", "subject": renamed.on[0].what })).await;
    assert!(!error && said.starts_with("checked (on 0 things)"), "taken off by that name: {said}");
    // The user deletes it: gone, told.
    let (status, _) = http(addr, "DELETE", "/api/tags/checked", None).await;
    assert_eq!(status, 204);
    event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagRemoved { tag_id } if *tag_id == tag.id)).await;
    assert_eq!(std::fs::read_dir(&logs).unwrap().count(), 0, "nothing tagged, nothing logged");
}

#[tokio::test]
async fn a_command_tagged_while_it_runs_comes_back_whole_past_the_scrollback_and_its_shell() {
    let (addr, _state) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    // 50,000 lines, five times what the terminal keeps; tagged early on.
    reader.run("seq 1 5000; sleep 6; seq 5001 50000").await;
    reader.until_shows("5000").await;
    while !reader.commands.values().any(|c| c.text.starts_with("seq 1 5000") && c.output.is_some()) {
        reader.next().await;
    }
    let number = reader.commands.values().find(|c| c.text.starts_with("seq 1 5000")).unwrap().id;
    let command = format!("terminal {} command {number}", terminal.id);
    let mut lead = Mcp::lead(addr).await;
    let (error, said) = lead.tool("tag", json!({ "tag": "big", "subject": command })).await;
    assert!(!error, "{said}");
    while !reader.commands.values().any(|c| c.id == number && c.end.is_some()) {
        reader.next().await;
    }
    // Ended, its start long gone from the scrollback: all of it, from the logs.
    let (_, read) = http(addr, "GET", "/api/tags/big", None).await;
    assert!(read.contains("\"lines\":50002"), "{read}");
    // Its shell started again: the command is gone from there, whole in the logs.
    let (status, body) = http(addr, "POST", &format!("/api/terminals/{}/restart", terminal.id), None).await;
    assert_eq!(status, 200, "{body}");
    let (_, read) = http(addr, "GET", "/api/tags/big", None).await;
    assert!(read.contains("\"held\":\"kept\"") && read.contains("\"lines\":50002"), "its line, 50,000 lines, how it ended: {read}");
    let (error, first) = lead.tool("tag_read", json!({ "tag": "big", "thing": 1, "lines": "1-2" })).await;
    assert!(!error && first.contains("$ seq 1 5000") && first.ends_with("\n1\n(lines 1-2 of 50002: `lines` gives others)"), "{first}");
    let (error, last) = lead.tool("tag_read", json!({ "tag": "big", "thing": 1, "lines": "50001-50002" })).await;
    assert!(!error && last.contains("\n50000\n(") && last.contains("exit code 0"), "{last}");
}

#[tokio::test]
async fn a_project_says_how_much_scrollback_its_terminals_keep() {
    let arch = common::fixture_architecture_with(|a| {
        std::fs::create_dir_all(a.root().join(".nah")).unwrap();
        std::fs::write(a.root().join(".nah/terminals.toml"), "scrollback_lines = 300\n").unwrap();
        a
    });
    let (addr, _state) = common::spawn_app_with(arch).await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("seq 1 3000").await;
    reader.until_shows("3000").await;
    let screen = reader.last.clone().unwrap();
    // (Trimmed once 512 more have come.)
    assert!(screen.top - screen.oldest <= 300 + 512 && screen.top > 2500, "300 kept above the screen, give or take (10,000 would keep them all): {} to {}", screen.oldest, screen.top);
}

#[tokio::test]
async fn a_running_command_tagged_again_after_its_tag_came_off_is_kept_once() {
    let (addr, state) = spawn_app().await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("for i in 1 2 3 4; do echo tick-$i; sleep 0.5; done").await;
    reader.until_shows("tick-1").await;
    let number = loop {
        if let Some(c) = reader.commands.values().find(|c| c.text.starts_with("for i in") && c.output.is_some()) {
            break c.id;
        }
        reader.next().await;
    };
    let command = format!("terminal {} command {number}", terminal.id);
    let mut lead = Mcp::lead(addr).await;
    for tool in ["tag", "untag", "tag"] {
        let (error, said) = lead.tool(tool, json!({ "tag": "ticks", "subject": command })).await;
        assert!(!error, "{said}");
    }
    let followed = state.tags.tags.find("ticks").unwrap().on[0].kept.clone().unwrap();
    assert_eq!(followed.len, None, "tagged again while it ran: followed (its file of its own, read to its end)");
    let mut said = String::new();
    for _ in 0..100 {
        said = lead.tool("tag_read", json!({ "tag": "ticks", "thing": 1 })).await.1;
        if said.contains("exit code 0") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for tick in 1..=4 {
        assert_eq!(said.matches(&format!("tick-{tick}\n")).count(), 1, "tick-{tick} once: {said}");
    }
    assert!(said.contains("exit code 0"), "{said}");
    // Searched: each line found, with its tag, thing and line.
    let (error, found) = lead.tool("tag_find", json!({ "query": "tick-3" })).await;
    assert!(!error && found.contains(&format!("ticks · 1. {command} (for i in 1 2 3 4; do echo tick-$i; sleep 0.5; done) · line ")) && found.ends_with(": tick-3"), "{found}");
}

/// The server itself, as the user starts it: on a project folder, with
/// the test's cockpit token. Killed when dropped.
struct Started {
    child: std::process::Child,
    addr: SocketAddr,
}

impl Started {
    fn on(place: &std::path::Path, home: &std::path::Path) -> Self {
        let page = home.join("static");
        let build = page.join(meta_harness::server::COCKPIT_BUILD);
        std::fs::create_dir_all(build.parent().unwrap()).unwrap();
        std::fs::write(&build, b"").unwrap();
        std::fs::write(page.join("index.html"), "<title>NAH</title>").unwrap();
        let addr: SocketAddr = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_meta-harness"))
            .args(["serve", "--listen", &addr.to_string()])
            .current_dir(place)
            .env("HOME", home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("NAH_REPO_ROOT", place)
            .env("NAH_COCKPIT_TOKEN", COCKPIT_TOKEN)
            .env("STATIC_DIR", &page)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut started = Self { child, addr };
        let since = std::time::Instant::now();
        while std::net::TcpStream::connect(addr).is_err() {
            assert!(started.child.try_wait().unwrap().is_none(), "the server exited on start");
            assert!(since.elapsed() < Duration::from_secs(60), "the server didn't listen");
            std::thread::sleep(Duration::from_millis(100));
        }
        started
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A command and a line of its output, tagged, and the server killed and
/// started again on the project: the command comes back whole from the
/// logs, and the line still says the command it was in (its shell gone
/// with the server).
#[tokio::test]
async fn what_was_tagged_comes_back_whole_after_the_server_is_started_again() {
    let home = tempfile::tempdir().unwrap();
    let place = home.path().join("place");
    std::fs::create_dir_all(place.join("src")).unwrap();
    std::fs::write(place.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
    std::fs::write(place.join("src/lib.rs"), "pub fn run() {}\n").unwrap();
    let first = Started::on(&place, home.path());
    let terminal = open(first.addr).await;
    let mut reader = Reader::at(first.addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("printf 'kept-1\\nkept-2\\n'").await;
    while !reader.commands.values().any(|c| c.text.starts_with("printf") && c.end.is_some()) {
        reader.next().await;
    }
    let ran = reader.commands.values().find(|c| c.text.starts_with("printf")).unwrap().clone();
    let shell = reader.last.as_ref().unwrap().shell;
    let command = json!({ "kind": "terminal_command", "terminal": terminal.id, "shell": shell, "command": ran.id });
    let line = json!({ "kind": "terminal_lines", "terminal": terminal.id, "shell": shell, "first": ran.output.unwrap(), "last": ran.output.unwrap() });
    for subject in [command, line] {
        let (status, body) = http(first.addr, "POST", "/api/tags", Some(&json!({ "tag": "kept", "subject": subject }).to_string())).await;
        assert_eq!(status, 200, "{body}");
    }
    drop(reader);
    drop(first);
    let again = Started::on(&place, home.path());
    let (status, read) = http(again.addr, "GET", "/api/tags/kept", None).await;
    assert_eq!(status, 200, "{read}");
    let read: harness_contracts::TagRead = serde_json::from_str(&read).unwrap();
    let said: Vec<(&str, harness_contracts::Held, &str)> = read.on.iter().map(|t| (t.tagged.what.as_str(), t.held, t.text.as_deref().unwrap_or_default())).collect();
    let output = ran.output.unwrap();
    assert_eq!(said.len(), 2);
    assert_eq!((said[0].0, said[0].1), (format!("terminal {}@{shell} command {} (printf 'kept-1\\nkept-2\\n')", terminal.id, ran.id).as_str(), harness_contracts::Held::Kept), "{said:?}");
    assert!(said[0].2.contains("kept-1\nkept-2") && said[0].2.contains("exit code 0"), "whole, with how it ended: {said:?}");
    assert_eq!((said[1].0, said[1].1, said[1].2), (format!("terminal {}@{shell} line {output} (of command {})", terminal.id, ran.id).as_str(), harness_contracts::Held::Kept, "kept-1"), "the command it was in, still named: {said:?}");
}

/// A project says how much of a running command a note keeps
/// (`running_log_mb`): past it, the log says so and stops.
#[tokio::test]
async fn a_project_says_how_much_of_a_running_command_is_kept() {
    let arch = common::fixture_architecture_with(|a| {
        std::fs::create_dir_all(a.root().join(".nah")).unwrap();
        std::fs::write(a.root().join(".nah/terminals.toml"), "running_log_mb = 1\n").unwrap();
        a
    });
    let (addr, _state) = common::spawn_app_with(arch).await;
    let terminal = open(addr).await;
    let mut reader = Reader::at(addr, &format!("/ws/terminals/{}", terminal.id)).await;
    reader.next().await;
    reader.run("sleep 2; yes 0123456789 | head -n 200000; echo done-here").await;
    while !reader.commands.values().any(|c| c.text.starts_with("sleep 2") && c.output.is_some()) {
        reader.next().await;
    }
    let number = reader.commands.values().find(|c| c.text.starts_with("sleep 2")).unwrap().id;
    let mut lead = Mcp::lead(addr).await;
    let (error, said) = lead.tool("tag", json!({ "tag": "capped", "subject": format!("terminal {} command {number}", terminal.id) })).await;
    assert!(!error, "{said}");
    while !reader.commands.values().any(|c| c.id == number && c.end.is_some()) {
        reader.next().await;
    }
    let (_, read) = http(addr, "GET", "/api/tags/capped", None).await;
    let read: harness_contracts::TagRead = serde_json::from_str(&read).unwrap();
    let lines = read.on[0].lines;
    assert!((90_000..100_000).contains(&lines), "about 1 MB of 11-byte lines, not 2.2 MB: {lines}");
    let (_, end) = lead.tool("tag_read", json!({ "tag": "capped", "thing": 1, "lines": format!("{}-{}", lines - 1, lines) })).await;
    assert!(end.contains("(the rest isn't kept: past the 1 MB a growing thing's log keeps here)") && !end.lines().skip(1).any(|l| l == "done-here"), "{end}");
}
