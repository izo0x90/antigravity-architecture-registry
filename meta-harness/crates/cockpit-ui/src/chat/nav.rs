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
        // Enter: a folded run opens; else the file it points to does.
        let here = self.here().unwrap_or_default();
        out.push(match (here.unfolds(), self.target()) {
            (Some(folded), _) => Move::Enter.offer(format!("Unfold {}", folded.words())),
            (None, Some(Target::Message { link: Some((path, _)), .. })) => Move::Enter.offer(format!("Open {path}")),
            _ => Move::Enter.offer_if(false, "Open the file it points to"),
        });
        out.push(Move::Leave.offer_if(self.ui.cursor.is_some(), "Follow the latest again"));
        // Folding: the innermost fold under the cursor each key can act
        // on (its own, the run it's in); every one.
        let words = |f: &rows::Folding| f.words();
        let (toggle, open, close) = (here.toggles().map(|(f, open)| (words(f), !open)), here.opens().map(words), here.closes().map(words));
        out.extend(crate::nav::fold_here(crate::nav::FoldsHere { toggle: toggle.as_ref().map(|(w, folded)| (w.as_str(), *folded)), open: open.as_deref(), close: close.as_deref() }));
        let shape = self.shape();
        out.extend(crate::nav::fold_all((shape.any_folded, "Unfold every run of calls and output"), (shape.any_open, "Fold every run of calls and output")));
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

    /// Its settings, the desk's agents (read from now on), and how many
    /// files each turn changed (the desk's timeline).
    fn share(&mut self, shared: &crate::shared::Shared) {
        // The chat's settings changed: what it starts with, taken.
        if let Some(settings) = shared.settings.since(&mut self.seen_settings) {
            self.settings = settings.chat.clone();
            self.ui.folds.set_verbosity(self.settings.shows);
            if let Some(shown) = crate::indicator::Shown::named(&self.settings.indicator) {
                self.ui.indicator = shown;
            }
        }
        self.agents.follow(&shared.agents);
        self.unsent.follow(&shared.drafts);
        self.ui.messages.follow(shared);
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
            self.ui.pick = None;
            if let Some(row) = message.and_then(|at| self.shape().row_of(at)) {
                self.ui.scrolling.show(row, crate::viewport::Show::Middle);
            }
        }
    }

    /// What the cursor's on: its item (a folded run, as one: its first
    /// item's place, no file of its own), the file it points to.
    fn target(&self) -> Option<Target> {
        let index = self.ui.cursor?;
        let shape = self.shape();
        let folded = shape.row_of(index).is_some_and(|r| shape.rows[r].folded);
        let view = self.thread()?;
        let item = view.in_stream(index)?;
        let links = if folded { Vec::new() } else { links_of(item) };
        let picked = match self.ui.pick {
            Some(Pick::Link(n)) => links.get(n).cloned(),
            _ => None,
        };
        let link = picked.or_else(|| links.first().cloned());
        Some(Target::Message { thread: Some(view.thread.id.to_string()), turn: Some(item.turn_id.to_string()), index, link })
    }

    /// From the latest message (none under the cursor yet), moves start at
    /// the bottom.
    fn motion(&mut self, motion: Motion, count: usize) {
        let shape = self.shape();
        let Some(last) = shape.rows.last() else { return };
        use crate::viewport::{Go, Show};
        let fresh = self.ui.cursor.is_none();
        match motion {
            // Following the latest: the first move lands on the newest.
            Motion::Down | Motion::Up if fresh => self.put_cursor(last.first, Show::InView),
            // Message of that number (as a place says it).
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1), Show::Middle),
            _ => {
                if let Some(go) = Go::of(motion, count) {
                    self.go(go);
                }
            }
        }
    }

    /// A folded run, or a sub-agent's folded work, opens; a message's file
    /// opens in the code view.
    fn enter(&mut self) -> Step {
        if let Some(folded) = self.here().as_ref().and_then(rows::Here::unfolds) {
            self.ui.folds.set(folded, rows::Set::Open);
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
        let shape = self.shape();
        self.ui.scrolling.go(shape.rows.len(), shape.row_of(at), crate::viewport::Go::Last);
        Step::Done
    }

    /// The innermost fold under the cursor each key acts on (its own, then
    /// the run it's in), or every one.
    fn fold(&mut self, fold: Fold) {
        if let Fold::OpenAll | Fold::CloseAll = fold {
            self.ui.folds.set_all(if fold == Fold::OpenAll { rows::Set::Open } else { rows::Set::Folded });
            return;
        }
        let Some(here) = self.here() else { return };
        use rows::Set;
        let set = match fold {
            Fold::Toggle => here.toggles().map(|(f, open)| (f, if *open { Set::Folded } else { Set::Open })),
            Fold::Open => here.opens().map(|f| (f, Set::Open)),
            _ => here.closes().map(|f| (f, Set::Folded)),
        };
        if let Some((folding, set)) = set {
            self.ui.folds.set(folding, set);
        }
    }

    fn items(&self) -> Vec<Item> {
        self.thread().map(|v| v.stream().enumerate().map(|(i, item)| Item::new(i.to_string(), "", item_text(item))).collect()).unwrap_or_default()
    }

    fn spots(&self) -> Vec<Spot> {
        self.ui.spots.clone()
    }

    /// The row the cursor's on (a folded run's by its first item).
    fn cursor_key(&self) -> Option<String> {
        let shape = self.shape();
        self.ui.cursor.and_then(|c| shape.row_of(c)).map(|r| shape.rows[r].first.to_string())
    }

    /// A message ("3"), one of its links ("3:1") or code blocks ("3:c0").
    fn move_to(&mut self, key: &str) {
        let (index, part) = key.split_once(':').map_or((key, None), |(i, p)| (i, Some(p)));
        let Ok(index) = index.parse::<usize>() else { return };
        self.put_cursor(index, crate::viewport::Show::Middle);
        self.ui.pick = part.and_then(Pick::of);
    }
}
