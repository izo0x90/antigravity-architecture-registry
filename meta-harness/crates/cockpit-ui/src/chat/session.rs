//! Opening sessions, sending to them, and what comes back.

use super::*;

impl ChatStreamState {
    pub(super) fn open_thread(&mut self, net: &NetClient, ctx: &egui::Context, resume: Option<harness_contracts::NativeId>) {
        let Some(instance_id) = self.active_instance.clone() else {
            self.error = Some("No harness instance is available yet.".into());
            return;
        };
        if self.opening {
            return;
        }
        self.opening = true;
        self.error = None;
        let body = OpenThreadBody {
            instance_id,
            policy: self.policy(),
            model: self.model_selection(),
            resume,
            mcp_servers: Vec::new(),
        };
        let mailbox = self.mailbox.clone();
        net.open_thread(&body, ctx.clone(), move |r| post(&mailbox, Reply::Opened(r)));
    }

    pub(super) fn send_prompt(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        match self.active_thread.clone() {
            Some(thread) => {
                let mailbox = self.mailbox.clone();
                net.start_turn(&thread, &TurnInput { text, attachments: Vec::new() }, ctx.clone(), move |r| post(&mailbox, Reply::Sent(r)));
            }
            None => {
                self.queued_prompt = Some(text);
                self.open_thread(net, ctx, None);
            }
        }
    }

    /// Adds `text` to what the agent is doing, when it takes that and a
    /// turn runs; else sends it (queued behind the running turn).
    pub(super) fn add_to_turn(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        let steerable = self.instance().is_some_and(|s| s.capabilities.turns.active_steering);
        match (&self.active_thread, self.thread().and_then(|t| t.running_turn()).is_some() && steerable) {
            (Some(thread), true) => {
                let mailbox = self.mailbox.clone();
                net.inject(thread, &TurnInput { text, attachments: Vec::new() }, ctx.clone(), move |r| post(&mailbox, Reply::Sent(r)));
            }
            _ => self.send_prompt(net, ctx, text),
        }
    }

    pub(super) fn steer(&mut self, net: &NetClient, ctx: &egui::Context, input: SteerInput) {
        if let Some(thread) = &self.active_thread {
            let mailbox = self.mailbox.clone();
            net.steer(thread, &input, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
        }
    }

    /// Sends `text` to the agent: into the session open, else a new one
    /// opened for it.
    pub fn say(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        self.send_prompt(net, ctx, text);
    }

    /// Who's answering, in words: the harness, and the session if one is
    /// open.
    pub fn who(&self) -> String {
        let name = self.active_instance.as_ref().and_then(|id| self.store.instances.get(id)).map_or_else(|| "the agent".to_owned(), |i| i.display_name.clone());
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
                Reply::Opened(Ok(thread)) => {
                    self.opening = false;
                    self.active_thread = Some(thread);
                    self.saved_sessions = None;
                    if let Some(text) = self.queued_prompt.take() {
                        self.send_prompt(net, ctx, text);
                    }
                }
                Reply::Opened(Err(e)) => {
                    self.opening = false;
                    self.error = Some(e.into());
                }
                Reply::Saved(Ok(sessions)) => {
                    self.saved_sessions = Some(sessions);
                    if let Some(action) = self.after_saved.take() {
                        self.pending_palette = Some(action);
                    }
                }
                Reply::AuthStarted(Ok(challenge)) => {
                    self.error = None;
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
                Reply::Sent(Err(e)) | Reply::Saved(Err(e)) | Reply::AuthStarted(Err(e)) | Reply::Done(Err(e)) | Reply::Tools { result: Err(e), .. } => {
                    self.error = Some(e.into());
                }
                Reply::Sent(Ok(())) | Reply::Done(Ok(())) => {}
            }
        }
    }

    pub(super) fn load_saved(&mut self, net: &NetClient, ctx: &egui::Context, then: &str) {
        let Some(instance) = self.active_instance.clone() else { return };
        self.after_saved = Some(then.to_owned());
        let mailbox = self.mailbox.clone();
        net.list_saved_sessions(&instance, ctx.clone(), move |r| post(&mailbox, Reply::Saved(r.map(|p| p.sessions))));
    }

    /// Restarts the active session with the current MCP list (ACP only takes MCP
    /// servers when a session is created or loaded).
    pub(super) fn reconnect(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(view) = self.thread() else { return };
        let native = view.thread.native_id.clone();
        let thread = view.thread.id.clone();
        net.close_thread(&thread, ctx.clone());
        self.active_thread = None;
        self.open_thread(net, ctx, native);
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
