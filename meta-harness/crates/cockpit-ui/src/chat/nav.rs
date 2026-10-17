//! Moving through the chat: places, targets, links.

use super::*;

/// The files an item points at, in the order they're drawn.
pub(super) fn links_of(item: &TurnItem) -> Vec<(String, Option<usize>)> {
    match &item.body {
        ItemBody::ToolCall { locations, .. } => locations.iter().map(|l| (l.path.clone(), l.line.map(|l| l as usize))).collect(),
        ItemBody::FileChange { path, .. } => vec![(path.clone(), None)],
        _ => Vec::new(),
    }
}

/// What search looks through in an item.
pub(super) fn item_text(item: &TurnItem) -> String {
    match &item.body {
        ItemBody::UserMessage { text } => text.clone(),
        ItemBody::AssistantMessage { content, .. } => content.iter().map(block_summary).collect::<Vec<_>>().join(" "),
        ItemBody::Reasoning { text, .. } => text.clone(),
        ItemBody::ToolCall { name, locations, .. } => {
            let paths: Vec<&str> = locations.iter().map(|l| l.path.as_str()).collect();
            format!("{} {name} {}", item.title.clone().unwrap_or_default(), paths.join(" "))
        }
        ItemBody::FileChange { path, .. } => path.clone(),
        ItemBody::CommandExecution { command, .. } => command.clone(),
        ItemBody::ProposedPlan { markdown, .. } => markdown.clone(),
        ItemBody::Notice { text, .. } => text.clone(),
        ItemBody::Error { message } => message.clone(),
        ItemBody::Unknown { display, .. } => display.clone(),
        _ => String::new(),
    }
}

impl ChatStreamState {
    /// The shared moves where the cursor is, in the chat's words: through
    /// the messages, opening the file one points to, following the latest
    /// again.
    pub(super) fn moves(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        out.push(match self.target() {
            Some(Target::Message { link: Some((path, _)), .. }) => Move::Enter.offer(format!("Open {path}")),
            _ => Move::Enter.offer_if(false, "Open the file it points to"),
        });
        out.push(Move::Leave.offer_if(self.ui.cursor.is_some(), "Follow the latest again"));
        out
    }
}

impl crate::buffer::Buffer for ChatStreamState {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut crate::buffer::Drawing<'_>) {
        self.draw(ui, drawing.feedback, drawing.focused, cx);
    }

    /// While it's on screen, its slot's updates are seen.
    fn tick(&mut self, _net: &crate::net::NetClient, ctx: &egui::Context) {
        self.seen_now(ctx.input(|i| i.time));
    }

    /// What's typed and not sent.
    fn keep(&self) -> Option<serde_json::Value> {
        (!self.ui.input_buffer.is_empty()).then(|| serde_json::Value::String(self.ui.input_buffer.clone()))
    }

    fn restore_kept(&mut self, kept: &serde_json::Value) {
        if let Some(draft) = kept.as_str().filter(|_| self.ui.input_buffer.is_empty()) {
            self.ui.input_buffer = draft.to_owned();
        }
    }

    fn describe(&self) -> String {
        self.slot().map_or_else(|| "the agent's session".to_owned(), |s| format!("agent slot {}", s.name))
    }
}

impl Navigable for ChatStreamState {
    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The desk's agents (read from now on), and how many files each turn
    /// changed (the desk's timeline).
    fn share(&mut self, shared: &crate::shared::Shared) {
        if !self.agents.reads(&shared.agents) {
            self.agents = shared.agents.view();
        }
        self.took();
        let (timeline, read) = shared.changes.timeline();
        if read != self.turn_files_read {
            self.turn_files = timeline.map(|t| t.turns.iter().map(|x| (x.turn.clone(), x.files)).collect()).unwrap_or_default();
            self.turn_files_read = read;
        }
    }

    fn place(&self) -> Place {
        self.ui.place()
    }

    /// An agent's chat: its slot, or the one running the session.
    fn can_show(&self, target: &Target) -> bool {
        matches!(target, Target::Agent { .. })
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let Target::Agent { slot, thread } = target else { return Err("Not an agent's chat.".into()) };
        let found = {
            let agents = self.agents();
            let running = |t: &ThreadId| agents.store().slots.values().find(|s| s.current_thread() == Some(t)).map(|s| s.id.clone());
            slot.clone().filter(|s| agents.slot(s).is_some()).or_else(|| thread.as_ref().and_then(running))
        };
        let Some(slot) = found else {
            return Err(match (slot, thread) {
                (_, Some(thread)) => format!("No agent slot runs session {thread} now."),
                _ => "That agent slot is closed.".into(),
            });
        };
        self.ui.slot = slot;
        self.ui.cursor = None;
        Ok(())
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<ChatPlace>().map(|p| p.message.map_or("latest".to_owned(), |m| format!("message {}", m + 1))).unwrap_or_default()
    }

    fn restore(&mut self, place: &Place) {
        if let Some(ChatPlace { message, slot }) = place.read() {
            if let Some(slot) = slot {
                self.ui.slot = slot;
            }
            self.ui.cursor = message;
            self.ui.link_pick = None;
            if let Some(at) = message {
                self.ui.scrolling.show(at, crate::viewport::Show::Middle);
            }
        }
    }

    fn target(&self) -> Option<Target> {
        let index = self.ui.cursor?;
        let item = self.stream().into_iter().nth(index)?;
        let links = links_of(&item);
        let link = self.ui.link_pick.and_then(|n| links.get(n).cloned()).or_else(|| links.first().cloned());
        Some(Target::Message { thread: self.active_thread().map(|t| t.to_string()), turn: Some(item.turn_id.to_string()), index, link })
    }

    /// From the latest message (none under the cursor yet), moves start at
    /// the bottom.
    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.stream().len();
        if n == 0 {
            return;
        }
        use crate::viewport::{Go, Show};
        let fresh = self.ui.cursor.is_none();
        match motion {
            // Following the latest: the first move lands on the newest.
            Motion::Down | Motion::Up if fresh => self.put_cursor(n - 1, Show::InView),
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1), Show::Middle),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// A message's file opens in the code view.
    fn enter(&mut self) -> Step {
        match self.target() {
            Some(Target::Message { link: Some(_), .. }) => Step::GoTo(GoTo::Code),
            Some(_) => Step::Nothing("No file in this message.".into()),
            None => Step::Nothing("Move to a message first (j / k).".into()),
        }
    }

    /// Lets go of the cursor: the stream goes to the latest and follows it
    /// again.
    fn leave(&mut self) -> Step {
        let Some(at) = self.ui.cursor.take() else {
            return Step::Nothing("Already following the latest.".into());
        };
        let n = self.stream().len();
        self.ui.scrolling.go(n, Some(at), crate::viewport::Go::Last);
        Step::Done
    }

    fn fold(&mut self, _fold: Fold) {}

    fn items(&self) -> Vec<Item> {
        self.stream().iter().enumerate().map(|(i, item)| Item::new(i.to_string(), "", item_text(item))).collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.ui.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.ui.cursor.map(|c| c.to_string())
    }

    /// A message ("3") or one of its links ("3:1").
    fn move_to(&mut self, key: &str) {
        let mut parts = key.split(':');
        let Some(index) = parts.next().and_then(|i| i.parse::<usize>().ok()) else { return };
        self.put_cursor(index, crate::viewport::Show::Middle);
        self.ui.link_pick = parts.next().and_then(|n| n.parse().ok());
    }
}
