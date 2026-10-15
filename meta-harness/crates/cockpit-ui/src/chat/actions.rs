//! What the chat offers as actions, and running them.

use super::*;

/// Snake/kebab name of a contract enum as serialized, for keymap names.
pub(super) fn wire_name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

#[derive(Clone, Debug)]
enum ChatCommand {
    Insert,
    ClearPrompt,
    NewSession,
    ResumeSession,
    DeleteSaved,
    SwitchSession,
    SelectInstance,
    CloseSession,
    StopTurn,
    RefreshInstance,
    SetConfig { option: String, boolean: bool },
    NextModel,
    NativeMode,
    RuntimeMode,
    ListMcp,
    AddMcp,
    RemoveMcp,
    ListSkills,
    AddToTurn,
    HarnessCommand { name: String },
    Login,
    CompleteLogin,
    Logout,
    Answer { thread: ThreadId, request: harness_contracts::RequestId, option: String },
}

impl ActionProvider for ChatStreamState {
    /// Every agent action available right now, from the instance snapshot, the
    /// active session's controls and what is pending.
    fn actions(&self) -> Vec<Action> {
        use ChatCommand as C;
        use ToolbarStyle::{Button, Control};
        let mut out = Vec::new();
        let snapshot = self.instance();
        let caps = snapshot.map(|s| s.capabilities.clone()).unwrap_or_default();
        let view = self.thread();
        let idle = self.active_thread.is_none() && !self.opening;

        // Prompt (this view only)
        out.push(Action::new("chat.insert", "Type a prompt", C::Insert).focused().importance(crate::actions::PRIMARY));
        if !self.ui.input_buffer.is_empty() {
            out.push(Action::new("chat.clear", "Clear the prompt", C::ClearPrompt).focused().importance(MINOR));
        }

        // Session
        let mut new = Action::new("session.new", "New session", C::NewSession);
        if idle {
            new = new.toolbar(Button, "Connect", Some(icon::LIGHTNING));
        }
        out.push(new);
        if caps.sessions.list_saved {
            let param = self.saved_choices().map(Param::Choice).unwrap_or(Param::None);
            let mut resume = Action::new("session.resume", "Resume a saved session", C::ResumeSession).param(param.clone());
            if idle {
                resume = resume.toolbar(Button, "Resume…", Some(icon::ARROW_COUNTER_CLOCKWISE));
            }
            out.push(resume);
            if caps.sessions.delete_saved {
                out.push(Action::new("session.delete", "Delete a saved session", C::DeleteSaved).param(param));
            }
        }
        if self.store.threads.len() > 1 {
            let choices = self
                .store
                .threads
                .values()
                .map(|t| {
                    Choice::new(t.thread.id.to_string(), t.thread.title.clone().unwrap_or_else(|| t.thread.id.to_string()))
                        .current(Some(&t.thread.id) == self.active_thread.as_ref())
                })
                .collect();
            out.push(Action::new("session.switch", "Switch session", C::SwitchSession).param(Param::Choice(choices)));
        }
        if self.store.instances.len() > 1 && self.active_thread.is_none() {
            let choices = self
                .store
                .instances
                .values()
                .map(|i| {
                    let label = match &i.status {
                        ProviderStatus::Ready => i.display_name.clone(),
                        other => format!("{} ({other:?})", i.display_name),
                    };
                    Choice::new(i.instance_id.to_string(), label).current(Some(&i.instance_id) == self.active_instance.as_ref())
                })
                .collect();
            out.push(Action::new("instance.select", "Choose harness", C::SelectInstance).param(Param::Choice(choices)).toolbar(Control, "Harness", None));
        }
        if let Some(view) = view {
            out.push(Action::new("session.close", "Close session", C::CloseSession).toolbar(Button, "", Some(icon::X)));
            if view.running_turn().is_some() && caps.turns.interrupt {
                out.push(Action::new("session.stop", "Stop the running turn", C::StopTurn).toolbar(Button, "Stop", Some(icon::STOP)));
            }
        }
        out.push(Action::new("instance.refresh", "Re-probe the harness", C::RefreshInstance));

        // Model, config and mode: exactly what the harness reports for the session;
        // before one, what the next session starts with.
        match view {
            Some(view) => {
                let controls = &view.thread.controls;
                if !controls.modes.is_empty() && !controls.config.iter().any(|c| c.category == harness_contracts::ConfigCategory::Mode) {
                    let current = controls.current_mode.clone();
                    let choices = controls
                        .modes
                        .iter()
                        .map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone()).current(current.as_ref() == Some(&m.id)))
                        .collect();
                    out.push(Action::new("mode.native", "Set session mode", C::NativeMode).param(Param::Choice(choices)).toolbar(Control, "Mode", None));
                }
                for control in &controls.config {
                    let (param, boolean) = match &control.kind {
                        ConfigKind::Select { choices, current } => (
                            Param::Choice(
                                choices
                                    .iter()
                                    .map(|c| Choice::new(&c.value, &c.name).detail(c.group.clone().or(c.description.clone())).current(&c.value == current))
                                    .collect(),
                            ),
                            false,
                        ),
                        ConfigKind::Boolean { current } => (Param::Toggle { on: *current }, true),
                    };
                    let mut action = Action::new(format!("config.{}", control.id), format!("Set {}", control.name), C::SetConfig { option: control.id.clone(), boolean })
                        .alias(format!("config.category.{}", wire_name(&control.category)))
                        .param(param)
                        .toolbar(Control, &control.name, None);
                    if let Some(d) = &control.description {
                        action = action.detail(d.clone());
                    }
                    out.push(action);
                }
            }
            None => {
                if let Some(s) = snapshot.filter(|s| !s.models.is_empty()) {
                    let default = Choice::new("", "default").current(self.model.is_none());
                    let choices = std::iter::once(default).chain(s.models.iter().map(|m| Choice::new(&m.id, &m.name).current(self.model.as_ref() == Some(&m.id)))).collect();
                    out.push(Action::new("model.next", "Model for the next session", C::NextModel).param(Param::Choice(choices)).toolbar(Control, "Model", None));
                }
                if let Some(s) = snapshot.filter(|s| !s.runtime_modes.is_empty()) {
                    let choices = s.runtime_modes.iter().map(|m| Choice::new(wire_name(m), mode_label(*m)).current(*m == self.runtime_mode)).collect();
                    out.push(Action::new("mode.runtime", "Runtime mode", C::RuntimeMode).param(Param::Choice(choices)).toolbar(Control, "Mode", None));
                }
            }
        }

        // Tool servers and skills
        let mut servers: Vec<Choice> = match view {
            Some(v) => v.thread.mcp_servers.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect(),
            None => self.tools.iter().flat_map(|t| t.ours.iter().chain(&t.project)).map(|m| Choice::new(&m.name, mcp_label(m))).collect(),
        };
        if let Some(s) = snapshot {
            servers.extend(s.tool_servers.iter().map(|t| Choice::new(&t.name, format!("{} — {} (its own, {:?})", t.name, t.transport, t.scope).to_lowercase()).detail(Some(t.file.clone()))));
        }
        if !servers.is_empty() {
            out.push(Action::new("mcp.list", "Tool servers its sessions get", C::ListMcp).param(Param::Choice(servers)));
        }
        if let Some(tools) = &self.tools {
            out.push(Action::new("mcp.add", "Add a tool server to the project", C::AddMcp).param(Param::Text { hint: "name https://host/mcp (then sse for the older kind)".into() }));
            if !tools.project.is_empty() {
                let choices = tools.project.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
                out.push(Action::new("mcp.remove", "Remove a tool server from the project", C::RemoveMcp).param(Param::Choice(choices)));
            }
        }
        if let Some(s) = snapshot.filter(|s| !s.skills.is_empty()) {
            let choices = s
                .skills
                .iter()
                .map(|k| Choice::new(&k.name, format!("{} ({})", k.name, wire_name(&k.scope))).detail(k.description.clone().or(k.path.clone())))
                .collect();
            out.push(Action::new("harness.skills", format!("{}'s skills", s.display_name), C::ListSkills).param(Param::Choice(choices)));
        }
        if let Some(view) = view
            && view.running_turn().is_some()
            && caps.turns.active_steering
            && !self.ui.input_buffer.trim().is_empty()
        {
            out.push(Action::new("chat.inject", "Add the prompt to what it's doing", C::AddToTurn).focused());
        }

        // Harness commands
        if let Some(s) = snapshot {
            for command in &s.commands {
                let param = match &command.input_hint {
                    Some(hint) => Param::Text { hint: hint.clone() },
                    None => Param::None,
                };
                out.push(
                    Action::new(format!("command.{}", command.name), format!("/{}", command.name), C::HarnessCommand { name: command.name.clone() })
                        .param(param)
                        .detail(command.description.clone()),
                );
            }
        }

        // Account
        if let Some(s) = snapshot.filter(|s| !s.auth.methods.is_empty()) {
            let choices = s.auth.methods.iter().map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone())).collect();
            out.push(Action::new("auth.login", "Sign in", C::Login).param(Param::Choice(choices)));
            let hint = self.login.as_ref().and_then(|l| l.expects.clone()).unwrap_or_else(|| "what the sign-in page gave you".into());
            out.push(Action::new("auth.complete", "Finish signing in", C::CompleteLogin).param(Param::Text { hint }));
            out.push(Action::new("auth.logout", "Sign out", C::Logout));
        }

        // The pending request's options (contextual)
        if let Some(view) = view
            && let Some(request) = view.requests.iter().find(|r| r.status == RequestStatus::Pending)
        {
            for option in &request.options {
                let answer = C::Answer { thread: view.thread.id.clone(), request: request.id.clone(), option: option.id.clone() };
                let mut action = Action::new(format!("request.{}.{}", request.id, option.id), format!("{}: {}", option.label, request.title), answer)
                    .alias(format!("request.{}", wire_name(&option.decision)));
                if let Some(w) = &option.warning {
                    action = action.detail(w.clone());
                }
                out.push(action);
            }
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<ChatCommand>() else { return };
        let (net, ctx) = (cx.net, cx.ctx);
        let thread = self.active_thread.clone();
        match command {
            ChatCommand::Insert => *cx.input_mode = crate::keymap::InputMode::Insert { slot: cx.slot },
            ChatCommand::ClearPrompt => self.ui.input_buffer.clear(),
            ChatCommand::NewSession => {
                self.active_thread = None;
                self.open_thread(net, ctx, None);
            }
            ChatCommand::ResumeSession => match value {
                Some(native) => {
                    self.active_thread = None;
                    self.open_thread(net, ctx, Some(NativeId::new(native)));
                }
                None => self.load_saved(net, ctx, &action.id),
            },
            ChatCommand::DeleteSaved => match (value, self.active_instance.clone()) {
                (Some(native), Some(instance)) => {
                    let mailbox = self.mailbox.clone();
                    net.delete_saved_session(&instance, &NativeId::new(native), ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                    self.saved_sessions = None;
                }
                (None, _) => self.load_saved(net, ctx, &action.id),
                _ => {}
            },
            ChatCommand::SwitchSession => {
                if let Some(t) = value {
                    self.active_thread = Some(ThreadId::new(t));
                }
            }
            ChatCommand::SelectInstance => {
                if let Some(i) = value {
                    self.active_instance = Some(InstanceId::new(i));
                }
            }
            ChatCommand::CloseSession => {
                if let Some(t) = &thread {
                    net.close_thread(t, ctx.clone());
                    self.active_thread = None;
                }
            }
            ChatCommand::StopTurn => {
                if let Some(t) = &thread {
                    net.interrupt(t, ctx.clone());
                }
            }
            ChatCommand::RefreshInstance => {
                if let Some(instance) = &self.active_instance {
                    let mailbox = self.mailbox.clone();
                    net.refresh_instance(instance, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
            ChatCommand::SetConfig { option, boolean } => {
                if let Some(v) = value {
                    let json = if *boolean { serde_json::Value::Bool(v == "true") } else { serde_json::Value::String(v) };
                    self.steer(net, ctx, SteerInput { config: vec![OptionValue { id: option.clone(), value: json }], ..Default::default() });
                }
            }
            ChatCommand::NextModel => self.model = value.filter(|v| !v.is_empty()),
            ChatCommand::NativeMode => {
                if let Some(mode) = value {
                    self.steer(net, ctx, SteerInput { mode: Some(mode), ..Default::default() });
                }
            }
            ChatCommand::RuntimeMode => {
                if let Some(mode) = value.and_then(|v| serde_json::from_value::<RuntimeMode>(serde_json::Value::String(v)).ok()) {
                    self.runtime_mode = mode;
                }
            }
            ChatCommand::ListMcp | ChatCommand::ListSkills => {}
            ChatCommand::AddToTurn => {
                let text = std::mem::take(&mut self.ui.input_buffer).trim().to_owned();
                if !text.is_empty() {
                    self.add_to_turn(net, ctx, text);
                }
            }
            ChatCommand::AddMcp => {
                let text = value.unwrap_or_default();
                let mut parts = text.split_whitespace();
                match (parts.next(), parts.next(), parts.next(), &self.tools) {
                    (Some(name), Some(url), kind @ (None | Some("sse")), Some(tools)) => {
                        let mut project: Vec<McpServer> = tools.project.iter().filter(|m| m.name != name).cloned().collect();
                        let transport = match kind {
                            Some(_) => McpTransport::Sse { url: url.into(), headers: Vec::new() },
                            None => McpTransport::Http { url: url.into(), headers: Vec::new() },
                        };
                        project.push(McpServer { name: name.into(), transport });
                        self.save_tools(net, ctx, &project);
                    }
                    _ => self.error = Some("A tool server needs a name and a URL, e.g. `docs https://host/mcp` (`… sse` for the older kind)".into()),
                }
            }
            ChatCommand::RemoveMcp => {
                if let (Some(name), Some(tools)) = (value, &self.tools) {
                    let project: Vec<McpServer> = tools.project.iter().filter(|m| m.name != name).cloned().collect();
                    self.save_tools(net, ctx, &project);
                }
            }
            ChatCommand::HarnessCommand { name } => {
                let text = match value {
                    Some(args) if !args.is_empty() => format!("/{name} {args}"),
                    _ => format!("/{name}"),
                };
                self.send_prompt(net, ctx, text);
            }
            ChatCommand::Login => {
                if let (Some(method), Some(instance)) = (value, self.active_instance.clone()) {
                    let mailbox = self.mailbox.clone();
                    net.begin_auth(&instance, &method, ctx.clone(), move |r| post(&mailbox, Reply::AuthStarted(r)));
                }
            }
            ChatCommand::CompleteLogin => {
                if let (Some(url), Some(instance)) = (value, self.active_instance.clone()) {
                    let mailbox = self.mailbox.clone();
                    net.complete_auth(&instance, &url, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                    self.login = None;
                }
            }
            ChatCommand::Logout => {
                if let Some(instance) = &self.active_instance {
                    let mailbox = self.mailbox.clone();
                    net.logout(instance, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
            ChatCommand::Answer { thread, request, option } => {
                let Some(pending) = self.store.threads.get(thread).and_then(|t| t.request(request)) else { return };
                let response = self.drafts.response(pending, option);
                let mailbox = self.mailbox.clone();
                net.respond(thread, request, &response, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
            }
        }
    }

    fn take_palette_request(&mut self) -> Option<String> {
        self.pending_palette.take()
    }
}
