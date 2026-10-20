//! The chats as owners of tags' subjects: a turn of a session, a message of
//! a turn (`Subject::ChatTurn`, `Subject::ChatMessage`), by the harness's
//! id for the session (it outlives our threads) and the turn's number.
//! Named by the agent slot whose session it is (`chat lead turn 3
//! message 2`; a session that isn't its slot's current one, with the start
//! of its id: `chat lead@4f2a91c0 turn 3`). What one says is read from the
//! session while it runs (its turns and items, as the event hub keeps
//! them); a session is the harness's, not ours to keep, so a tagged one is
//! written to the tags' logs when it's tagged.

use std::sync::Arc;

use harness_contracts::{AdapterEvent, ClientEvent, ContentBlock, DETAIL, ItemBody, NativeId, Slot, Subject, Turn, TurnItem, message_number};
use harness_core::{EventHub, SlotManager};

/// How much of a session's id names it (beside its slot's name).
const ID_SHOWN: usize = 8;

pub struct Chats {
    slots: Arc<SlotManager>,
    hub: Arc<EventHub>,
}

impl Chats {
    pub fn new(slots: Arc<SlotManager>, hub: Arc<EventHub>) -> Arc<Self> {
        Arc::new(Self { slots, hub })
    }

    /// The slot session `native` is, when it's one of a slot's.
    fn slot_of(&self, native: &NativeId) -> Option<Slot> {
        self.slots.list().into_iter().find(|s| s.sessions.iter().any(|x| x.native_id.as_ref() == Some(native)))
    }

    /// Session `native`'s turns and items while it runs.
    fn running(&self, native: &NativeId) -> Option<(Vec<Turn>, Vec<TurnItem>)> {
        let slot = self.slot_of(native)?;
        let thread = slot.sessions.iter().find(|s| s.native_id.as_ref() == Some(native))?.thread.clone()?;
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

    /// Turn `n` of session `native` (while it runs) and its items, in order.
    fn turn(&self, native: &NativeId, n: u32) -> Option<Vec<TurnItem>> {
        let (turns, items) = self.running(native)?;
        let turn = turns.into_iter().find(|t| t.ordinal == n)?;
        let mut of: Vec<TurnItem> = items.into_iter().filter(|i| i.turn_id == turn.id).collect();
        of.sort_by_key(|i| i.ordinal);
        Some(of)
    }

    /// A session in words: its slot's chat and the start of its id (a
    /// slot has several sessions over time), else the session.
    fn words(&self, native: &NativeId) -> String {
        let short: String = native.as_str().chars().take(ID_SHOWN).collect();
        match self.slot_of(native) {
            Some(slot) => format!("chat {}@{short}", slot.name),
            None => format!("chat session {short}"),
        }
    }
}

/// What a message says.
fn message_text(body: &ItemBody) -> Option<String> {
    match body {
        ItemBody::UserMessage { text } => Some(text.clone()),
        ItemBody::AssistantMessage { content, .. } => Some(content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n")),
        _ => None,
    }
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
            ItemBody::UserMessage { text } => out.push_str(&format!("you: {text}\n")),
            ItemBody::AssistantMessage { .. } => out.push_str(&format!("{}\n", message_text(&item.body).unwrap_or_default())),
            ItemBody::ToolCall { name, input, output, .. } => {
                out.push_str(&format!("• {name}\n"));
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
                out.push_str(&format!("• $ {command}{ended}\n"));
                out.push_str(&detail(output.as_deref().unwrap_or_default()));
            }
            ItemBody::FileChange { path, diff } => {
                out.push_str(&format!("• changed {path}\n"));
                if let Some(diff) = diff {
                    out.push_str(&detail(&diff.new_text));
                }
            }
            ItemBody::Notice { text, .. } => out.push_str(&format!("! {text}\n")),
            ItemBody::Error { message } => out.push_str(&format!("! {message}\n")),
            _ => {}
        }
    }
    out
}

impl nah_tags::Resolve for Chats {
    fn name(&self, subject: &Subject) -> Option<String> {
        match subject {
            Subject::ChatTurn { session, turn } => Some(format!("{} turn {turn}", self.words(session))),
            Subject::ChatMessage { session, turn, message } => Some(format!("{} turn {turn} message {message}", self.words(session))),
            _ => None,
        }
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        match subject {
            Subject::ChatTurn { session, turn } => Some(turn_text(&self.turn(session, *turn)?)),
            Subject::ChatMessage { session, turn, message } => {
                let items = self.turn(session, *turn)?;
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
                sessions(self.slots.list()).into_iter().find(|n| n.as_str().starts_with(start.trim())).ok_or_else(|| format!("no session's id starts {start}"))?
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
                Some((turn, message)) => Subject::ChatMessage { session, turn: number(turn)?, message: number(message)? },
                None => Subject::ChatTurn { session, turn: number(what)? },
            })
        })())
    }

    /// The turn's still in the session (its text isn't built to say so).
    fn there(&self, subject: &Subject) -> bool {
        let (Subject::ChatTurn { session, turn } | Subject::ChatMessage { session, turn, .. }) = subject else { return false };
        self.running(session).is_some_and(|(turns, _)| turns.iter().any(|t| t.ordinal == *turn))
    }

    /// A turn still running (or a message of one): what it says so far,
    /// then, once the turn ends, the rest of what it says then (all of it
    /// again, said so, when it changed rather than grew).
    fn follow(&self, subject: &Subject, mut more: nah_tags::More) -> bool {
        let (Subject::ChatTurn { session, turn } | Subject::ChatMessage { session, turn, .. }) = subject else { return false };
        let Some((turns, _)) = self.running(session) else { return false };
        let Some(running) = turns.into_iter().find(|t| t.ordinal == *turn && !t.status.is_terminal()) else { return false };
        let so_far = self.now(subject).unwrap_or_default();
        if !more(&so_far) {
            return false;
        }
        let (_, mut events) = self.hub.subscribe();
        let (chats, subject, thread, turn) = (Chats { slots: self.slots.clone(), hub: self.hub.clone() }, subject.clone(), running.thread_id, *turn);
        std::thread::spawn(move || {
            loop {
                match events.blocking_recv() {
                    Ok(ClientEvent::Session { event: AdapterEvent::TurnUpdated { turn: t } }) if t.thread_id == thread && t.ordinal == turn && t.status.is_terminal() => {
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

    /// A session is the harness's: what's tagged in it is kept in the logs.
    fn lasts(&self, subject: &Subject) -> bool {
        !matches!(subject, Subject::ChatTurn { .. } | Subject::ChatMessage { .. })
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
        let refs: Vec<&TurnItem> = items.iter().collect();
        assert_eq!([0, 1, 2].map(|n| message_number(&refs, &items[n].id)), [Some(1), None, Some(2)], "messages numbered in their turn");
    }
}
