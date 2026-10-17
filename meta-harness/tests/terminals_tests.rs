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
                FromTerminal::Found { .. } | FromTerminal::CommandOutput { .. } => {}
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
    // it has the whole of it.
    // (One write finishes it and scrolls it off: one frame.)
    reader.run("printf 'half'; sleep 0.5; printf ' and whole\\n%s\\n' $(seq 400 430)").await;
    reader.until_shows("430").await;
    let halves: Vec<&String> = reader.lines.values().filter(|l| l.starts_with("half")).collect();
    assert_eq!(halves, ["half and whole"], "{:?}", reader.lines);
    assert!(reader.sent_again.iter().any(|l| l == "half and whole"), "sent again as it went: {:?}", reader.sent_again);
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

    // The user tags the line: a new tag, told to everyone.
    let subject = json!({ "kind": "terminal_lines", "terminal": terminal.id, "shell": screen.shell, "first": line, "last": line });
    let (status, body) = http(addr, "POST", "/api/tags", Some(&json!({ "tag": "seen-it", "subject": subject }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let tag: harness_contracts::Tag = serde_json::from_str(&body).unwrap();
    assert_eq!((tag.name.as_str(), tag.on[0].said.as_str(), tag.on[0].by.as_str()), ("seen-it", "tagged-line", "user"), "kept with what it said, and who tagged it");
    event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id)).await;
    let (status, body) = http(addr, "POST", "/api/tags", Some(&json!({ "tag": "two words", "subject": subject }).to_string())).await;
    assert!(status == 400 && body.contains("can't be a tag's name"), "{status} {body}");
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains(&format!("\"what\":\"terminal {} lines {line}-{line}\"", terminal.id)) && read.contains("\"now\":\"tagged-line\""), "what it says now, named: {read}");
    // A narrower reader lays the lines out anew: the tag follows its line,
    // told to everyone as it moves (not only when it's next read).
    reader.say(ToTerminal::Size { size: TermSize { cols: 40, rows: 20 } }).await;
    while !matches!(reader.next().await, FromTerminal::Screen { update } if update.size.cols == 40) {}
    let moved = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id && t.on[0].subject != tag.on[0].subject)).await;
    let harness_contracts::ClientEvent::TagUpdated { tag: moved } = moved else { unreachable!() };
    let harness_contracts::Subject::TerminalLines { first, .. } = moved.on[0].subject else { panic!("{moved:?}") };
    assert_eq!(moved.on[0].what, format!("terminal {} lines {first}-{first}", terminal.id), "named where it is now");
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains("\"now\":\"tagged-line\""), "still found, numbered anew: {read}");
    // Many new widths in a row (more than the terminal remembers): still followed.
    for cols in 41..48 {
        reader.say(ToTerminal::Size { size: TermSize { cols, rows: 20 } }).await;
    }
    while !matches!(reader.next().await, FromTerminal::Screen { update } if update.size.cols == 47) {}
    let (_, read) = http(addr, "GET", "/api/tags/seen-it", None).await;
    assert!(read.contains("\"now\":\"tagged-line\""), "still found after seven: {read}");

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
    assert!(read.contains(&format!("- {command} (tagged by lead)")) && read.contains("$ echo tagged-line (exit code 0"), "{read}");
    assert!(read.contains("(tagged by the user)"), "the user's, to an agent: {read}");
    // Taken off by the name tag_read gives; again: it isn't on it, and says so.
    let (error, said) = lead.tool("untag", json!({ "tag": "seen-it", "subject": command })).await;
    assert!(!error && said.starts_with("seen-it (on 1 thing)"), "{said}");
    let (error, said) = lead.tool("untag", json!({ "tag": "seen-it", "subject": command })).await;
    assert!(error && said.contains("isn't on that"), "{said}");
    let (error, said) = lead.tool("tag_edit", json!({ "tag": "seen-it", "name": "checked", "note": "the echo works" })).await;
    assert!(!error && said.contains("checked") && said.contains("the echo works"), "{said}");
    // A prompt naming the tag hands the agent what it's on, as it is now.
    let (status, body) = http(addr, "POST", "/api/slots/lead/prompt", Some(&json!({ "text": "look at #checked." }).to_string())).await;
    assert_eq!(status, 200, "{body}");
    let echoed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("What the tags named above"))).await;
    let echoed = format!("{echoed:?}");
    assert!(echoed.contains("#checked (the echo works; noted by lead)") && echoed.contains("tagged-line"), "an agent's note says whose: {echoed}");
    // So does a sub-slot's report to the lead: every prompt goes the one way.
    let (err, text) = lead.tool("slot_start", json!({ "harness": "fake", "name": "helper" })).await;
    assert!(!err, "{text}");
    let sub = state.harness.slots.list().into_iter().find(|s| s.name == "helper").unwrap();
    let thread = state.harness.slots.get(&sub.id).and_then(|s| s.current_thread().cloned()).expect("its session runs");
    let mut helper = Mcp::as_session(addr, &thread).await;
    let (err, text) = helper.tool("tell_lead", json!({ "text": "fixed #checked" })).await;
    assert!(!err, "{text}");
    let echoed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::Session { event: harness_contracts::AdapterEvent::ItemUpdated { item } } if format!("{:?}", item.body).contains("fixed #checked"))).await;
    assert!(format!("{echoed:?}").contains("What the tags named above"), "{echoed:?}");
    // One that may only read can't.
    let mut external = Mcp::connect(addr).await;
    let (_, listed) = external.tool("tags", json!({})).await;
    assert!(listed.starts_with("checked ("), "{listed}");
    let (error, said) = external.tool("tag_edit", json!({ "tag": "checked", "delete": true })).await;
    assert!(error && said.contains("tags.edit"), "refused: {said}");
    // Its shell started again: what it's on is an earlier start's, named so
    // (as it's taken off by), and gone (what it said is kept).
    let (status, body) = http(addr, "POST", &format!("/api/terminals/{}/restart", terminal.id), None).await;
    assert_eq!(status, 200, "{body}");
    let renamed = event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagUpdated { tag: t } if t.id == tag.id && t.on[0].what.contains('@'))).await;
    let (_, read) = lead.tool("tag_read", json!({ "tag": "checked" })).await;
    assert!(read.contains(&format!("- terminal {}@", terminal.id)) && read.contains("gone; when tagged:\ntagged-line"), "{read}");
    let harness_contracts::ClientEvent::TagUpdated { tag: renamed } = renamed else { unreachable!() };
    let (error, said) = lead.tool("untag", json!({ "tag": "checked", "subject": renamed.on[0].what })).await;
    assert!(!error && said.starts_with("checked (on 0 things)"), "taken off by that name: {said}");
    // The user deletes it: gone, told.
    let (status, _) = http(addr, "DELETE", "/api/tags/checked", None).await;
    assert_eq!(status, 204);
    event_until(&mut events, |e| matches!(e, harness_contracts::ClientEvent::TagRemoved { tag_id } if *tag_id == tag.id)).await;
}
