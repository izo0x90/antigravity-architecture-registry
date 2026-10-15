//! Opening sessions, sending to them, and what comes back.

use super::*;

impl ChatStreamState {
    /// Opens a session in this view's slot: a new one, or a saved one.
    pub(super) fn open_thread(&mut self, net: &NetClient, ctx: &egui::Context, resume: Option<harness_contracts::NativeId>) {
        self.open_in(self.ui.slot.clone(), net, ctx, resume);
    }

    /// Opens a session in `slot`. A slot with no harness yet (a new
    /// project's lead) gets the first there is.
    fn open_in(&mut self, slot: SlotId, net: &NetClient, ctx: &egui::Context, resume: Option<harness_contracts::NativeId>) {
        let has_harness = self.store.slots.get(&slot).is_some_and(|s| s.instance_id.is_some());
        let instance_id = match has_harness {
            true => None,
            false => match self.store.instances.keys().next() {
                Some(first) => Some(first.clone()),
                None => {
                    self.queued.remove(&slot);
                    self.set_error(slot, "No harness instance is available yet.".into());
                    return;
                }
            },
        };
        if !self.opening.insert(slot.clone()) {
            return;
        }
        self.errors.remove(&slot);
        let body = OpenSlotSession { instance_id, resume, ..Default::default() };
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.open_slot_session(&slot, &body, ctx.clone(), move |r| post(&mailbox, Reply::Opened(for_slot, r)));
    }

    /// A prompt to this view's slot (its session resumed, or opened, first
    /// if it isn't running).
    pub(super) fn send_prompt(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        self.prompt(self.ui.slot.clone(), net, ctx, text, false);
    }

    fn prompt(&mut self, slot: SlotId, net: &NetClient, ctx: &egui::Context, text: String, add_to_running: bool) {
        self.errors.remove(&slot);
        if self.store.slots.get(&slot).is_none_or(|s| s.instance_id.is_none()) {
            self.queued.insert(slot.clone(), text);
            self.open_in(slot, net, ctx, None);
            return;
        }
        let mailbox = self.mailbox.clone();
        let for_slot = slot.clone();
        net.prompt_slot(&slot, &text, add_to_running, ctx.clone(), move |r| post(&mailbox, Reply::Sent(for_slot, r)));
    }

    /// Adds `text` to what the agent is doing, when it takes that and a
    /// turn runs; else sends it (queued behind the running turn).
    pub(super) fn add_to_turn(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        let steerable = self.instance().is_some_and(|s| s.capabilities.turns.active_steering);
        let running = self.thread().and_then(|t| t.running_turn()).is_some();
        self.prompt(self.ui.slot.clone(), net, ctx, text, steerable && running);
    }

    pub(super) fn steer(&mut self, net: &NetClient, ctx: &egui::Context, input: SteerInput) {
        if let Some(thread) = &self.active_thread() {
            let mailbox = self.mailbox.clone();
            net.steer(thread, &input, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
        }
    }

    /// Sends `text` to the lead (the talk box): into its session, opened
    /// first if it isn't running.
    pub fn say(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        self.prompt(SlotId::new(LEAD), net, ctx, text, false);
    }

    /// Who's answering, in words: the harness, and the session if one is
    /// open.
    pub fn who(&self) -> String {
        let name = self.instance().map_or_else(|| "the agent".to_owned(), |i| i.display_name.clone());
        match self.thread() {
            Some(t) => {
                let session: String = t.thread.id.to_string().chars().take(8).collect();
                format!("{name} · session {session} · {}", format!("{:?}", t.thread.status).to_lowercase())
            }
            None => format!("{name} · a new session"),
        }
    }

    /// The agent's last message in the open session, as text.
    pub fn last_reply(&self) -> Option<String> {
        self.thread()?.items.iter().rev().find_map(|item| match &item.body {
            ItemBody::AssistantMessage { content, .. } => Some(content.iter().map(block_summary).collect::<Vec<_>>().join(" ")),
            _ => None,
        })
    }

    /// Takes what the server answered (every frame, shown or not: a message
    /// sent from elsewhere waits on a session opening).
    pub fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        if !self.tools_asked && !self.store.instances.is_empty() {
            self.tools_asked = true;
            let mailbox = self.mailbox.clone();
            net.tool_servers(ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: false }));
        }
        self.drain_replies(net, ctx);
    }

    /// The single place async results land.
    pub(super) fn drain_replies(&mut self, net: &NetClient, ctx: &egui::Context) {
        let replies = std::mem::take(&mut *self.mailbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in replies {
            match reply {
                Reply::Opened(slot, Ok(opened)) => {
                    self.opening.remove(&slot);
                    self.saved_sessions = None;
                    let mut said = Vec::new();
                    said.extend(opened.note);
                    if opened.delivered_reports > 0 {
                        said.push(format!("{} report(s) from sub-slots were delivered to the lead.", opened.delivered_reports));
                    }
                    if !said.is_empty() {
                        self.notice = Some(said.join(" "));
                    }
                    if let Some(text) = self.queued.remove(&slot) {
                        self.prompt(slot, net, ctx, text, false);
                    }
                }
                Reply::Opened(slot, Err(e)) => {
                    self.opening.remove(&slot);
                    self.queued.remove(&slot);
                    self.set_error(slot, e.into());
                }
                Reply::Sent(slot, Err(e)) => self.set_error(slot, e.into()),
                Reply::SlotMade(Ok(slot)) => {
                    self.ui.slot = slot;
                    self.ui.cursor = None;
                }
                Reply::Restart { slot, resume, result: Ok(()) } => self.open_in(slot, net, ctx, resume),
                Reply::Saved(Ok(sessions)) => {
                    self.saved_sessions = Some(sessions);
                    if let Some(action) = self.after_saved.take() {
                        self.pending_palette = Some(action);
                    }
                }
                Reply::AuthStarted(Ok(challenge)) => {
                    self.errors.remove(&self.ui.slot.clone());
                    // Nothing to open or paste: it's done (the harness checks again).
                    self.login = (challenge.url.is_some() || challenge.expects.is_some()).then_some(challenge);
                }
                Reply::Tools { result: Ok(tools), saved } => {
                    self.tools = Some(tools);
                    // The open session takes tool servers only when it starts.
                    if saved {
                        self.reconnect(net, ctx);
                    }
                }
                Reply::Saved(Err(e)) | Reply::AuthStarted(Err(e)) | Reply::Done(Err(e)) | Reply::Tools { result: Err(e), .. } | Reply::SlotMade(Err(e)) | Reply::Restart { result: Err(e), .. } => {
                    self.set_error(self.ui.slot.clone(), e.into());
                }
                Reply::Sent(_, Ok(())) | Reply::Done(Ok(())) => {}
            }
        }
    }

    pub(super) fn load_saved(&mut self, net: &NetClient, ctx: &egui::Context, then: &str) {
        let Some(instance) = self.active_instance() else { return };
        self.after_saved = Some(then.to_owned());
        let mailbox = self.mailbox.clone();
        net.list_saved_sessions(&instance, ctx.clone(), move |r| post(&mailbox, Reply::Saved(r.map(|p| p.sessions))));
    }

    /// Restarts this view's session with the current tool servers (ACP takes
    /// them only when a session is created or loaded): ended, then resumed.
    pub(super) fn reconnect(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(slot) = self.slot().cloned() else { return };
        let Some(session) = slot.current_session().filter(|s| s.thread.is_some()).cloned() else { return };
        let mailbox = self.mailbox.clone();
        let (for_slot, resume) = (slot.id.clone(), session.native_id.clone());
        net.close_slot_session(&slot.id, &session.key, ctx.clone(), move |result| post(&mailbox, Reply::Restart { slot: for_slot, resume, result }));
    }

    /// Saves the project's tool servers (the session restarts with them).
    pub(super) fn save_tools(&mut self, net: &NetClient, ctx: &egui::Context, project: &[McpServer]) {
        let mailbox = self.mailbox.clone();
        net.set_tool_servers(project, ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: true }));
    }

    pub(super) fn saved_choices(&self) -> Option<Vec<Choice>> {
        self.saved_sessions.as_ref().map(|list| {
            list.iter()
                .map(|s| Choice::new(s.native_id.to_string(), s.title.clone().unwrap_or_else(|| s.native_id.to_string())).detail(s.updated_at.clone()))
                .collect()
        })
    }
}
