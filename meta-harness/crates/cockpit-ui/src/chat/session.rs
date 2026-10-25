//! What the chat asks of the agents for its slot: sessions, prompts,
//! steering, saved sessions, tool servers; and what it takes from them
//! each frame (a sub-slot it asked for, the saved sessions it waited on,
//! its slot gone).

use super::*;

impl ChatStreamState {
    /// Opens a session in this view's slot: a new one, or a saved one.
    pub(super) fn open_thread(&mut self, resume: Option<NativeId>) {
        self.act(Act::Open { resume });
    }

    /// A prompt to this view's slot (its session resumed, or opened, first
    /// if it isn't running).
    pub(super) fn send_prompt(&mut self, message: harness_contracts::Message) {
        self.act(Act::Prompt { message, add: false });
    }

    /// Adds `text` to what the agent is doing, when it takes that and a
    /// turn runs; else sends it (queued behind the running turn).
    pub(super) fn add_to_turn(&mut self, message: harness_contracts::Message) {
        let add = self.steerable();
        self.act(Act::Prompt { message, add });
    }

    /// The harness takes more while a turn runs, and one does.
    pub(super) fn steerable(&self) -> bool {
        self.instance().is_some_and(|s| s.capabilities.turns.active_steering) && self.thread().is_some_and(|t| t.running_turn().is_some())
    }

    pub(super) fn steer(&mut self, input: SteerInput) {
        if let Some(thread) = self.active_thread() {
            self.act(Act::Steer { thread, input });
        }
    }

    /// Lists the saved sessions; `then` opens in the palette once they're in.
    pub(super) fn load_saved(&mut self, then: &str) {
        let Some(instance) = self.active_instance() else { return };
        self.after_saved = Some(then.to_owned());
        self.act(Act::ListSaved { instance });
    }

    pub(super) fn saved_choices(&self) -> Option<Vec<Choice>> {
        self.agents().saved().map(|list| {
            list.iter()
                .map(|s| Choice::new(s.native_id.to_string(), s.title.clone().unwrap_or_else(|| s.native_id.to_string())).detail(s.updated_at.clone()))
                .collect()
        })
    }

    /// What the agents did since: a sub-slot made from this slot is shown;
    /// the saved sessions waited on open in the palette; its slot closed,
    /// it shows the lead.
    pub(super) fn took(&mut self) {
        let made = self.agents().made().0;
        if made != self.made_seen {
            self.made_seen = made;
            let made = self.agents().made().1.cloned();
            if let Some((_, made)) = made.filter(|(from, _)| *from == self.ui.slot) {
                self.ui.slot = made;
                self.cursor_to(None);
            }
        }
        if self.after_saved.is_some() && self.agents().saved().is_some() {
            self.pending_palette = self.after_saved.take();
        }
        let gone = self.agents().store().closed.contains(&self.ui.slot);
        if gone {
            self.ui.slot = SlotId::new(LEAD);
            self.cursor_to(None);
        }
    }

    /// Its slot's updates were seen (now and then, while it has some and
    /// it's on screen).
    pub(super) fn seen_now(&mut self, now: f64) {
        let slot = self.ui.slot.clone();
        let has_updates = self.agents().store().notifications.values().any(|n| n.level == harness_contracts::Level::Update && n.slot.as_ref() == Some(&slot));
        if has_updates && self.seen.due(&slot, now, SEEN_EVERY) {
            self.act(Act::Seen);
        }
    }
}
