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
        // Folding: the run or the item's own fold under the cursor; every one.
        let here = self.ui.cursor.and_then(|at| self.with_items(|items, folds| folds.at(items, at)).flatten());
        out.extend(crate::nav::fold_here(here.as_ref().map(|(f, open)| (f.words(), !open)).as_ref().map(|(w, folded)| (w.as_str(), *folded))));
        let (any_folded, any_open) = self.with_items(|items, folds| folds.any(items)).unwrap_or_default();
        out.extend(crate::nav::fold_all((any_folded, "Unfold every run of calls and output"), (any_open, "Fold every run of calls and output")));
        out
    }
}

impl ChatStreamState {
    /// Its slot's unsent text as the server saved it, taken when the box
    /// is empty (once per slot shown, once the drafts are read).
    pub(super) fn take_unsent(&mut self) {
        if self.unsent_for.as_ref() == Some(&self.ui.slot) || self.ui.sign_in_has_box {
            return;
        }
        let Some(saved) = self.unsent.get().current(&self.ui.slot).map(|s| s.cloned()) else { return };
        self.unsent_for = Some(self.ui.slot.clone());
        if self.ui.input.text.is_empty()
            && let Some(saved) = &saved
        {
            self.ui.input.restore(saved);
        }
        // What's already in the box (the view's own, or typed before the
        // drafts were read) differs from what's saved: reported.
        self.unsent_said = saved.unwrap_or_default();
    }

    /// What's in the box, when it changed since it was last reported
    /// (typed, put in, or cleared by sending): to be saved for its slot.
    pub(super) fn report_unsent(&mut self) {
        // The box is a sign-in's: what's saved stays the slot's draft.
        if self.ui.sign_in_has_box {
            return;
        }
        if self.unsent_for.as_ref() == Some(&self.ui.slot) && !self.ui.input.holds(&self.unsent_said) {
            self.unsent_said = self.ui.input.draft();
            self.asks.push(Ask::Draft { slot: self.ui.slot.clone(), draft: self.unsent_said.clone() });
        }
    }
}

impl crate::buffer::Buffer for ChatStreamState {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut crate::buffer::Drawing<'_>) {
        self.tend_sign_in_box(cx.note);
        self.take_unsent();
        self.draw(ui, drawing.feedback, drawing.typing, cx);
        self.report_unsent();
    }

    /// While it's on screen, its slot's updates are seen.
    fn tick(&mut self, _net: &crate::net::NetClient, ctx: &egui::Context) {
        self.seen_now(ctx.input(|i| i.time));
    }

    fn describe(&self) -> String {
        self.slot().map_or_else(|| "the agent's session".to_owned(), |s| format!("agent slot {}", s.name))
    }
}

impl crate::nav::DrawsThings for ChatStreamState {
    fn drawn_at(&self, subject: &harness_contracts::Subject) -> Option<egui::Rect> {
        self.tag_drawn_at(subject)
    }
}

impl Navigable for ChatStreamState {
    fn typing(&self) -> Option<&crate::conversation::TypingBox> {
        Some(&self.ui.input)
    }

    fn typing_box(&mut self) -> Option<&mut crate::conversation::TypingBox> {
        Some(&mut self.ui.input)
    }

    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    /// The desk's agents (read from now on), and how many files each turn
    /// changed (the desk's timeline).
    fn share(&mut self, shared: &crate::shared::Shared) {
        self.agents.follow(&shared.agents);
        self.unsent.follow(&shared.drafts);
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

    /// An agent's chat: its slot, or the one running the session; a turn
    /// or message of a session an agent slot has open.
    fn can_show(&self, target: &Target) -> bool {
        match target {
            Target::Agent { .. } => true,
            Target::Chat { session, .. } => self.slot_with(session).is_some(),
            _ => false,
        }
    }

    fn things(&self, key: &str) -> crate::nav::Things {
        self.tag_things(key)
    }

    fn draws_things(&self) -> Option<&dyn crate::nav::DrawsThings> {
        Some(self)
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        if let Target::Chat { session, turn, message } = target {
            let slot = self.slot_with(session).ok_or("No agent slot has that session open now (resume it to see it).")?;
            self.ui.slot = slot;
            let subject = match message {
                Some(message) => harness_contracts::Subject::ChatMessage { session: session.clone(), turn: *turn, message: *message, turn_id: String::new() },
                None => harness_contracts::Subject::ChatTurn { session: session.clone(), turn: *turn, turn_id: String::new() },
            };
            let at = self.tagged_indexes(&subject).first().copied().ok_or_else(|| format!("Turn {turn} isn't in the chat (its session holds it no more)."))?;
            self.put_cursor(at, crate::viewport::Show::Middle);
            return Ok(());
        }
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
            if let Some(at) = message
                && let Some(row) = self.rows().iter().position(|r| r.holds(at))
            {
                self.ui.scrolling.show(row, crate::viewport::Show::Middle);
            }
        }
    }

    fn target(&self) -> Option<Target> {
        let index = self.ui.cursor?;
        let item = self.item_at(index)?;
        let links = links_of(&item);
        let link = self.ui.link_pick.and_then(|n| links.get(n).cloned()).or_else(|| links.first().cloned());
        Some(Target::Message { thread: self.active_thread().map(|t| t.to_string()), turn: Some(item.turn_id.to_string()), index, link })
    }

    /// From the latest message (none under the cursor yet), moves start at
    /// the bottom.
    fn motion(&mut self, motion: Motion, count: usize) {
        let rows = self.rows();
        let Some(last) = rows.last() else { return };
        use crate::viewport::{Go, Show};
        let fresh = self.ui.cursor.is_none();
        match motion {
            // Following the latest: the first move lands on the newest.
            Motion::Down | Motion::Up if fresh => self.put_cursor(last.first, Show::InView),
            // The row of that number, as shown.
            Motion::Line(i) => self.put_cursor(rows[i.saturating_sub(1).min(rows.len() - 1)].first, Show::Middle),
            _ => {
                if let Some(go) = Go::of(motion, count) {
                    self.go(go);
                }
            }
        }
    }

    /// A folded run opens; a message's file opens in the code view.
    fn enter(&mut self) -> Step {
        if let Some(at) = self.ui.cursor
            && let Some((folding @ crate::chat::rows::Folding::Run(..), false)) = self.with_items(|items, folds| folds.at(items, at)).flatten()
        {
            self.ui.folds.toggle(&folding);
            return Step::Done;
        }
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
        let rows = self.rows();
        let row = rows.iter().position(|r| r.holds(at));
        self.ui.scrolling.go(rows.len(), row, crate::viewport::Go::Last);
        Step::Done
    }

    /// The run or the item's own fold under the cursor (its own first);
    /// or every one.
    fn fold(&mut self, fold: Fold) {
        match fold {
            Fold::OpenAll | Fold::CloseAll => {
                if let Some(all) = self.with_items(|items, folds| folds.all(items, fold == Fold::OpenAll)) {
                    self.ui.folds = all;
                }
            }
            Fold::Toggle | Fold::Open | Fold::Close => {
                let Some(at) = self.ui.cursor else { return };
                let Some((folding, open)) = self.with_items(|items, folds| folds.at(items, at)).flatten() else { return };
                if fold == Fold::Toggle || (fold == Fold::Open) != open {
                    self.ui.folds.toggle(&folding);
                }
            }
        }
        // The cursor in a run folded now: on the run.
        if let Some(at) = self.ui.cursor
            && let Some(row) = self.rows().into_iter().find(|r| r.holds(at))
        {
            self.ui.cursor = Some(row.first);
        }
    }

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
