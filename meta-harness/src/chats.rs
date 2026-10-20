//! The chats as owners of things notes are on: a turn of a session, a
//! message of a turn (`Subject::ChatTurn`, `Subject::ChatMessage`), by the
//! harness's id for the session (it outlives our threads) and the turn's
//! number. Named by the agent slot whose session it is (`chat lead turn 3
//! message 2`; a session that isn't its slot's current one, with the start
//! of its id: `chat lead@4f2a91c0 turn 3`). What one says is read from the
//! session while it runs (its turns and items, as the event hub keeps
//! them); a session is the harness's, not ours to keep, so a noted one is
//! written to the logs when it's noted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use harness_contracts::{AdapterEvent, ClientEvent, ContentBlock, DETAIL, ItemBody, NativeId, Slot, Subject, ThreadId, Turn, TurnId, TurnItem, message_number};
use harness_core::{EventHub, SlotManager};

pub struct Chats {
    slots: Arc<SlotManager>,
    hub: Arc<EventHub>,
    /// The sessions named, by the start of their id: one no slot holds now
    /// is named back by it.
    named: Mutex<HashMap<String, NativeId>>,
    /// Told when turns were numbered anew (a message the agent took while
    /// a turn ran), or a slot closed: the notes keep their turns where
    /// they are now, named as they are now.
    moved: OnceLock<Box<dyn Fn() + Send + Sync>>,
}

/// A noted turn (or a message's): its session, number, and own id (empty:
/// not known, found by its number).
type TurnAt<'a> = (&'a NativeId, u32, &'a str);

fn turn_at(subject: &Subject) -> Option<TurnAt<'_>> {
    match subject {
        Subject::ChatTurn { session, turn, turn_id } | Subject::ChatMessage { session, turn, turn_id, .. } => Some((session, *turn, turn_id)),
        _ => None,
    }
}

impl Chats {
    pub fn new(slots: Arc<SlotManager>, hub: Arc<EventHub>) -> Arc<Self> {
        Arc::new(Self { slots, hub, named: Mutex::default(), moved: OnceLock::new() })
    }

    /// Tells `moved` when turns are numbered anew, or a slot closes (its
    /// sessions are named as no slot's then), from now on (watching the
    /// event stream for it, on a thread of its own).
    pub fn when_moved(self: &Arc<Self>, moved: impl Fn() + Send + Sync + 'static) {
        if self.moved.set(Box::new(moved)).is_err() {
            return;
        }
        let (_, mut events) = self.hub.subscribe();
        let chats = Arc::downgrade(self);
        std::thread::spawn(move || {
            let mut numbers: HashMap<TurnId, u32> = HashMap::new();
            loop {
                match events.blocking_recv() {
                    Ok(ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } }) => {
                        let was = numbers.insert(turn.id.clone(), turn.ordinal);
                        if was.is_some_and(|n| n != turn.ordinal) {
                            let Some(chats) = chats.upgrade() else { return };
                            if let Some(moved) = chats.moved.get() {
                                moved();
                            }
                        }
                    }
                    Ok(ClientEvent::SlotClosed { .. }) => {
                        let Some(chats) = chats.upgrade() else { return };
                        if let Some(moved) = chats.moved.get() {
                            moved();
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                }
            }
        });
    }

    /// The slot session `native` is, when it's one of a slot's.
    fn slot_of(&self, native: &NativeId) -> Option<Slot> {
        self.slots.list().into_iter().find(|s| s.sessions.iter().any(|x| x.native_id.as_ref() == Some(native)))
    }

    /// The thread session `native` runs in, while a slot holds it.
    fn thread_of(&self, native: &NativeId) -> Option<ThreadId> {
        self.slot_of(native)?.sessions.iter().find(|s| s.native_id.as_ref() == Some(native))?.thread.clone()
    }

    /// Session `native`'s turns and items while it runs.
    fn running(&self, native: &NativeId) -> Option<(Vec<Turn>, Vec<TurnItem>)> {
        let thread = self.thread_of(native)?;
        let (mut turns, mut items) = (Vec::new(), Vec::new());
        for event in self.hub.thread_state(&thread) {
            match event {
                ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn } } => turns.push(turn),
                ClientEvent::Session { event: AdapterEvent::ItemUpdated { item } } => items.push(item),
                _ => {}
            }
        }
        Some((turns, items))
    }

    /// A noted turn as the session has it now (by its id when known, else
    /// its number), and its items, in order.
    fn turn(&self, (native, n, id): TurnAt<'_>) -> Option<(Turn, Vec<TurnItem>)> {
        let (turns, items) = self.running(native)?;
        let turn = turns.into_iter().find(|t| if id.is_empty() { t.ordinal == n } else { t.id.as_str() == id })?;
        let mut of: Vec<TurnItem> = items.into_iter().filter(|i| i.turn_id == turn.id).collect();
        of.sort_by_key(|i| i.ordinal);
        Some((turn, of))
    }

    /// Whose chat session `native` is, in a name (`chat_who`); one no
    /// slot holds, kept to be named back.
    fn who(&self, native: &NativeId) -> String {
        let who = harness_contracts::chat_who(self.slot_of(native).as_ref().map(|s| s.name.as_str()), native);
        if let Some(start) = who.strip_prefix("session ") {
            self.named.lock().unwrap_or_else(PoisonError::into_inner).insert(start.to_owned(), native.clone());
        }
        who
    }
}

/// `text` with each line that starts as a line of detail does (`DETAIL`)
/// made not to: only detail is folded.
fn plain(text: &str) -> String {
    let mark = DETAIL.chars().next().unwrap_or(' ');
    text.split('\n').map(|l| if l.starts_with(DETAIL) { l.replacen(mark, " ", 1) } else { l.to_owned() }).collect::<Vec<_>>().join("\n")
}

/// What a message says.
fn message_text(body: &ItemBody) -> Option<String> {
    let text = match body {
        ItemBody::UserMessage { text } => text.clone(),
        ItemBody::AssistantMessage { content, .. } => content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    Some(plain(&text))
}

/// `text` as detail under a line (a tool call's output): reads leave it
/// out unless asked for in full.
fn detail(text: &str) -> String {
    text.lines().map(|l| format!("{DETAIL}{l}\n")).collect()
}

/// A turn in words: your message, the answer, a line per tool call, what
/// was thought, planned, changed (what each gave, and what a tool was
/// asked, as detail under its line).
fn turn_text(items: &[TurnItem]) -> String {
    let mut out = String::new();
    for item in items {
        match &item.body {
            ItemBody::UserMessage { .. } => out.push_str(&format!("you: {}\n", message_text(&item.body).unwrap_or_default())),
            ItemBody::AssistantMessage { .. } => out.push_str(&format!("{}\n", message_text(&item.body).unwrap_or_default())),
            ItemBody::ToolCall { name, input, output, .. } => {
                out.push_str(&format!("• {}\n", plain(name)));
                if let Some(input) = input {
                    out.push_str(&detail(&input.to_string()));
                }
                let text: Vec<&str> = output.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                out.push_str(&detail(&text.join("\n")));
            }
            ItemBody::Reasoning { text, .. } => {
                out.push_str("• thought\n");
                out.push_str(&detail(text));
            }
            ItemBody::ProposedPlan { markdown, .. } => {
                out.push_str("• a plan\n");
                out.push_str(&detail(markdown));
            }
            ItemBody::TodoList { steps, .. } => {
                out.push_str("• to do\n");
                out.push_str(&detail(&steps.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n")));
            }
            ItemBody::Unknown { display, .. } => out.push_str(&detail(display)),
            ItemBody::CommandExecution { command, output, exit_code } => {
                let ended = exit_code.map(|c| format!(" (exit code {c})")).unwrap_or_default();
                out.push_str(&format!("• $ {}{ended}\n", plain(command)));
                out.push_str(&detail(output.as_deref().unwrap_or_default()));
            }
            ItemBody::FileChange { path, diff } => {
                out.push_str(&format!("• changed {}\n", plain(path)));
                if let Some(diff) = diff {
                    out.push_str(&detail(&diff.new_text));
                }
            }
            ItemBody::Notice { text, .. } => out.push_str(&format!("! {}\n", plain(text))),
            ItemBody::Error { message } => out.push_str(&format!("! {}\n", plain(message))),
            _ => {}
        }
    }
    out
}

impl nah_things::Resolve for Chats {
    fn owns(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::ChatTurn { .. } | Subject::ChatMessage { .. })
    }

    fn name(&self, subject: &Subject) -> String {
        let (Subject::ChatTurn { session, .. } | Subject::ChatMessage { session, .. }) = subject else { return String::new() };
        subject.name(&harness_contracts::Known { chat: self.who(session), ..harness_contracts::Known::default() })
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let (_, items) = self.turn(turn_at(subject)?)?;
        match subject {
            Subject::ChatTurn { .. } => Some(turn_text(&items)),
            Subject::ChatMessage { message, .. } => {
                let refs: Vec<&TurnItem> = items.iter().collect();
                let item = items.iter().find(|i| message_number(&refs, &i.id) == Some(*message))?;
                message_text(&item.body)
            }
            _ => None,
        }
    }

    /// `chat <slot>[@<id's start>] turn <n>[ message <m>]` (no id: the
    /// slot's session open now), or `chat session <id's start> turn …`.
    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let rest = words.trim().strip_prefix("chat ")?;
        let (who, what) = rest.split_once(" turn ")?;
        Some((|| {
            let sessions = |slots: Vec<Slot>| slots.into_iter().flat_map(|s| s.sessions.into_iter().filter_map(|x| x.native_id)).collect::<Vec<_>>();
            let session = if let Some(start) = who.strip_prefix("session ") {
                let start = start.trim();
                let held = self.named.lock().unwrap_or_else(PoisonError::into_inner).get(start).cloned();
                held.or_else(|| sessions(self.slots.list()).into_iter().find(|n| n.as_str().starts_with(start))).ok_or_else(|| format!("no session's id starts {start}"))?
            } else {
                let (slot_name, start) = match who.split_once('@') {
                    Some((slot, start)) => (slot, Some(start)),
                    None => (who, None),
                };
                let slot = self.slots.list().into_iter().find(|s| s.name == slot_name).ok_or_else(|| format!("no agent slot is named {slot_name:?}"))?;
                match start {
                    Some(start) if !start.is_empty() => slot.sessions.iter().filter_map(|s| s.native_id.clone()).find(|n| n.as_str().starts_with(start)),
                    Some(_) => None,
                    None => slot.current_session().and_then(|s| s.native_id.clone()),
                }
                .ok_or_else(|| format!("{slot_name} has no session {}", start.unwrap_or("open now")))?
            };
            let number = |n: &str| n.trim().parse::<u32>().map_err(|_| format!("{n:?} isn't a number"));
            Ok(match what.split_once(" message ") {
                Some((turn, message)) => Subject::ChatMessage { session, turn: number(turn)?, message: number(message)?, turn_id: String::new() },
                None => Subject::ChatTurn { session, turn: number(what)?, turn_id: String::new() },
            })
        })())
    }

    /// The turn's still in the session (its text isn't built to say so).
    fn there(&self, subject: &Subject) -> bool {
        turn_at(subject).is_some_and(|at| self.turn(at).is_some())
    }

    /// A turn noted: with its own id (the session may number it anew).
    fn settle(&self, subject: &Subject) -> Result<Subject, String> {
        let Some(at @ (_, _, id)) = turn_at(subject).filter(|(_, _, id)| id.is_empty()) else { return Ok(subject.clone()) };
        let turn_id = self.turn(at).map(|(t, _)| t.id.to_string()).unwrap_or_default();
        let _ = id;
        Ok(match subject.clone() {
            Subject::ChatTurn { session, turn, .. } => Subject::ChatTurn { session, turn, turn_id },
            Subject::ChatMessage { session, turn, message, .. } => Subject::ChatMessage { session, turn, message, turn_id },
            other => other,
        })
    }

    /// Turns numbered anew (a message the agent took while a turn ran):
    /// a noted one where its turn is now.
    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let at @ (_, n, id) = turn_at(subject)?;
        if id.is_empty() {
            return None;
        }
        let (turn, _) = self.turn(at)?;
        (turn.ordinal != n).then(|| match subject.clone() {
            Subject::ChatTurn { session, turn_id, .. } => Subject::ChatTurn { session, turn: turn.ordinal, turn_id },
            Subject::ChatMessage { session, message, turn_id, .. } => Subject::ChatMessage { session, turn: turn.ordinal, message, turn_id },
            other => other,
        })
    }

    /// A turn still running (or a message of one): what it says so far,
    /// then, once the turn ends, the rest of what it says then (all of it
    /// again, said so, when it changed rather than grew). Watched from
    /// before it's looked at: an end in between is seen.
    fn follow(&self, subject: &Subject, mut more: nah_things::More) -> bool {
        let Some(at @ (native, ..)) = turn_at(subject) else { return false };
        let Some(thread) = self.thread_of(native) else { return false };
        let (_, mut events) = self.hub.subscribe();
        let Some((running, _)) = self.turn(at).filter(|(t, _)| !t.status.is_terminal()) else { return false };
        let so_far = self.now(subject).unwrap_or_default();
        if !more(&so_far) {
            return false;
        }
        // Read by the turn's own id (numbered anew meanwhile, it's still the one).
        let subject = match subject.clone() {
            Subject::ChatTurn { session, turn, .. } => Subject::ChatTurn { session, turn, turn_id: running.id.to_string() },
            Subject::ChatMessage { session, turn, message, .. } => Subject::ChatMessage { session, turn, message, turn_id: running.id.to_string() },
            other => other,
        };
        let chats = Chats { slots: self.slots.clone(), hub: self.hub.clone(), named: Mutex::default(), moved: OnceLock::new() };
        std::thread::spawn(move || {
            loop {
                match events.blocking_recv() {
                    Ok(ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: t } }) if t.id == running.id && t.status.is_terminal() => {
                        let end = chats.now(&subject).unwrap_or_default();
                        let rest = match end.strip_prefix(&so_far) {
                            Some(rest) => rest.to_owned(),
                            None => format!("\n(as it ended)\n{end}"),
                        };
                        more(&rest);
                        return;
                    }
                    Ok(ClientEvent::ThreadClosed { thread_id }) if thread_id == thread => return,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                    _ => {}
                }
            }
        });
        true
    }

    /// A session is the harness's: what's noted in it is kept in the logs.
    fn lasts(&self, subject: &Subject) -> bool {
        !matches!(subject, Subject::ChatTurn { .. } | Subject::ChatMessage { .. })
    }

    /// A turn's tool calls say what they gave as detail.
    fn detailed(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::ChatTurn { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_contracts::{ItemId, ItemStatus, ThreadId, ToolKind, TurnId};

    fn item(ordinal: u32, body: ItemBody) -> TurnItem {
        TurnItem { id: ItemId::new(format!("i{ordinal}")), thread_id: ThreadId::new("t"), turn_id: TurnId::new("u"), native_id: None, parent_id: None, ordinal, status: ItemStatus::Completed, title: None, body }
    }

    #[test]
    fn a_turn_says_your_message_the_answer_and_a_line_per_tool_call_with_what_it_gave_under_it() {
        let items = [
            item(0, ItemBody::UserMessage { text: "fix the build".into() }),
            item(1, ItemBody::ToolCall { kind: ToolKind::Execute, name: "Bash".into(), input: None, output: vec![ContentBlock::Text { text: "error: 1\nwarning: 2".into() }], locations: Vec::new() }),
            item(2, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Fixed.".into() }], streaming: false }),
        ];
        assert_eq!(turn_text(&items), format!("you: fix the build\n• Bash\n{DETAIL}error: 1\n{DETAIL}warning: 2\nFixed.\n"));
        // What a message says never passes for detail (only detail is folded).
        let says = [item(0, ItemBody::UserMessage { text: format!("see:\n{DETAIL}not detail") })];
        assert!(!turn_text(&says).lines().any(|l| l.starts_with(DETAIL)), "{}", turn_text(&says));
        let refs: Vec<&TurnItem> = items.iter().collect();
        assert_eq!([0, 1, 2].map(|n| message_number(&refs, &items[n].id)), [Some(1), None, Some(2)], "messages numbered in their turn");
    }
}
