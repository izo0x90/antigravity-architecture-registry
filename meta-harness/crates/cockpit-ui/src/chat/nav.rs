//! Moving through the chat: places, targets, links.

use super::*;

/// The files an item points at, in the order they're drawn: a call's
/// places, a change's file, the files a message's words name.
pub(super) fn links_of(item: &TurnItem) -> Vec<(String, Option<usize>)> {
    match &item.body {
        ItemBody::ToolCall { locations, .. } => locations.iter().map(|l| (l.path.clone(), l.line.map(|l| l as usize))).collect(),
        ItemBody::FileChange { path, .. } => vec![(path.clone(), None)],
        ItemBody::AssistantMessage { .. } => crate::conversation::places(&item.body.said().unwrap_or_default()),
        _ => Vec::new(),
    }
}

/// What search looks through in an item.
pub(super) fn item_text(item: &TurnItem) -> String {
    match &item.body {
        ItemBody::UserMessage { text, typed } => crate::conversation::as_typed(text, typed.as_ref()).0.to_owned(),
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
        out.push(Move::Motion(Motion::NextSibling).offer("Next message"));
        out.push(Move::Motion(Motion::PrevSibling).offer("Previous message"));
        // Enter: a folded run opens; else the file it points to does.
        let here = self.here().unwrap_or_default();
        out.push(match (here.unfolds(), self.chip_here(), self.target()) {
            (Some(folded), ..) => Move::Enter.offer(format!("Unfold {}", folded.words())),
            (None, Some((words, _)), _) => Move::Enter.offer(format!("Go to {words}")),
            (None, None, Some(Target::Message { link: Some((path, _)), .. })) => Move::Enter.offer(format!("Open {path}")),
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
    /// The `count`th message (yours or the agent's) after the cursor's row
    /// (`forward`), or before it: the calls and the rest between passed
    /// over (following the latest: from the newest).
    fn step_message(&mut self, forward: bool, count: usize) {
        let shape = self.shape();
        let Some(view) = self.thread() else { return };
        let is_message = |row: &rows::Row| !row.folded && view.in_stream(row.first).is_some_and(|i| matches!(i.body, ItemBody::UserMessage { .. } | ItemBody::AssistantMessage { .. }));
        let from = self.ui.cursor.and_then(|c| shape.row_of(c));
        let messages: Vec<usize> = match (forward, from) {
            (true, Some(at)) => shape.rows.iter().enumerate().skip(at + 1).filter(|(_, r)| is_message(r)).map(|(n, _)| n).collect(),
            (true, None) => Vec::new(),
            (false, at) => shape.rows.iter().enumerate().take(at.unwrap_or(shape.rows.len())).filter(|(_, r)| is_message(r)).map(|(n, _)| n).rev().collect(),
        };
        drop(view);
        let to = messages.get(count.max(1) - 1).or(messages.last()).map(|n| shape.rows[*n].first);
        if let Some(to) = to {
            self.put_cursor(to, crate::viewport::Show::InView);
        }
    }

    /// The chip picked (a hint), else your message's first: its words and
    /// where it goes.
    fn chip_here(&self) -> Option<(String, Target)> {
        let chips = self.chips_here();
        let n = match self.ui.pick {
            Some(Pick::Chip(n)) => n,
            _ => 0,
        };
        chips.into_iter().nth(n)
    }

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

    /// What's picked for it: how much it shows, what shows the agent at
    /// work.
    fn keep(&self) -> Option<serde_json::Value> {
        self.ui.picked.kept()
    }

    fn restore_kept(&mut self, kept: &serde_json::Value) {
        self.ui.picked = Picked::from_kept(kept);
        self.settle();
    }
}

impl ChatStreamState {
    /// How much it shows and what shows the agent at work: as picked for
    /// it, else as the config says (a change of what it shows starts the
    /// folds afresh).
    pub(super) fn settle(&mut self) {
        self.ui.folds.set_verbosity(self.ui.picked.shows.unwrap_or(self.settings.shows));
        let kind = self.ui.picked.at_work.unwrap_or(self.settings.indicator);
        if self.ui.indicator.kind() != kind {
            self.ui.indicator = crate::indicator::Shown::of(kind);
        }
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

    /// The code block picked (a hint), else the message (yours as you
    /// typed it), else the proposed plan.
    fn copy_here(&self) -> Option<crate::nav::Copyable> {
        use crate::nav::Copyable;
        if let Some(Pick::Code(n)) = self.ui.pick
            && let Some(code) = self.codes_here().into_iter().nth(n)
        {
            return Some(Copyable::new("the code block", code));
        }
        let view = self.thread()?;
        let item = view.in_stream(self.ui.cursor?)?;
        match &item.body {
            ItemBody::UserMessage { text, typed } => Some(Copyable::new("your message", crate::conversation::as_typed(text, typed.as_ref()).0)),
            ItemBody::AssistantMessage { .. } => Some(Copyable::new("the message", item.body.said().unwrap_or_default())),
            ItemBody::ProposedPlan { markdown, .. } => Some(Copyable::new("the plan", markdown.as_str())),
            _ => None,
        }
    }

    /// Its settings, the desk's agents (read from now on), and how many
    /// files each turn changed (the desk's timeline).
    fn share(&mut self, shared: &crate::shared::Shared) {
        if let Some(settings) = shared.settings.since(&mut self.seen_settings) {
            self.settings = settings.chat.clone();
            self.context = settings.changes.context as usize;
        }
        self.settle();
        self.agents.follow(&shared.agents);
        self.unsent.follow(&shared.drafts);
        self.ui.messages.follow(shared);
        self.tend();
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
        self.cursor_to(None);
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
            self.cursor_to(message);
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
            Motion::NextSibling | Motion::PrevSibling => self.step_message(motion == Motion::NextSibling, count),
            _ => {
                if let Some(go) = Go::of(motion, count) {
                    self.go(go);
                }
            }
        }
    }

    /// A folded run, or a sub-agent's folded work, opens; a chip of your
    /// message goes to what it links; a message's file opens in the code
    /// view.
    fn enter(&mut self) -> Step {
        if let Some(folded) = self.here().as_ref().and_then(rows::Here::unfolds) {
            self.ui.folds.set(folded, rows::Set::Open);
            return Step::Done;
        }
        if let Some((_, target)) = self.chip_here() {
            self.asks.push(Ask::Show { target, in_: None });
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
        let Some(at) = self.ui.cursor else {
            return Step::Nothing("Already following the latest.".into());
        };
        self.cursor_to(None);
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
