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

impl Navigable for ChatStreamState {
    fn place(&self) -> Place {
        Place::Chat { message: self.ui.cursor }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Chat { message } = place {
            self.ui.cursor = *message;
            self.ui.link_pick = None;
            self.ui.scroll = message.is_some();
        }
    }

    fn target(&self) -> Option<Target> {
        let index = self.ui.cursor?;
        let item = self.stream().into_iter().nth(index)?;
        let links = links_of(&item);
        let link = self.ui.link_pick.and_then(|n| links.get(n).cloned()).or_else(|| links.first().cloned());
        Some(Target::Message { thread: self.active_thread.as_ref().map(|t| t.to_string()), index, link })
    }

    /// From the latest message (none under the cursor yet), moves start at
    /// the bottom.
    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.stream().len();
        if n == 0 {
            return;
        }
        let at = self.ui.cursor.unwrap_or(n - 1);
        let fresh = self.ui.cursor.is_none();
        match motion {
            Motion::Down => self.put_cursor(if fresh { at } else { at + count }),
            Motion::Up => self.put_cursor(if fresh { at } else { at.saturating_sub(count) }),
            Motion::HalfDown => self.put_cursor(at + self.half_page_items() * count),
            Motion::HalfUp => self.put_cursor(at.saturating_sub(self.half_page_items() * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right => {}
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

    fn can_enter(&self) -> bool {
        matches!(self.target(), Some(Target::Message { link: Some(_), .. }))
    }

    fn can_leave(&self) -> bool {
        self.ui.cursor.is_some()
    }

    fn enter_label(&self) -> Option<String> {
        match self.target()? {
            Target::Message { link: Some((path, _)), .. } => Some(format!("Open {path}")),
            _ => None,
        }
    }

    /// Lets go of the cursor: the stream follows the latest again.
    fn leave(&mut self) -> Step {
        if self.ui.cursor.take().is_none() {
            return Step::Nothing("Already following the latest.".into());
        }
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
        self.put_cursor(index);
        self.ui.link_pick = parts.next().and_then(|n| n.parse().ok());
    }
}
