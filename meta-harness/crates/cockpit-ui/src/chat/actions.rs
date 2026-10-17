//! What the chat offers as actions, and running them.

use super::*;
use crate::store::Store;

use crate::actions::wire_name;

#[derive(Clone, Debug)]
enum ChatCommand {
    Insert,
    ClearPrompt,
    NewSession,
    ResumeSession,
    DeleteSaved,
    SwitchSession,
    SelectInstance,
    PickSlot,
    StepSlot(i32),
    NewSlot,
    CloseSlot,
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
    CancelLogin,
    OpenLoginPage,
    OpenPage(String),
    Send,
    DismissNotice,
    Retry,
    DismissError,
    Answer { thread: ThreadId, request: harness_contracts::RequestId, option: String },
}

impl ActionProvider for ChatStreamState {
    /// Every agent action available right now, from the instance snapshot, the
    /// active session's controls and what is pending.
    fn actions(&self) -> Vec<Action> {
        use ChatCommand as C;
        use Style::{Button, Setting};
        let mut out = self.moves();
        let agents = self.agents.clone();
        let agents = agents.get();
        let store = agents.store();
        let snapshot = agents.instance_in(&self.ui.slot);
        let caps = snapshot.map(|s| s.capabilities.clone()).unwrap_or_default();
        let view = agents.thread_in(&self.ui.slot);
        let idle = self.active_thread().is_none() && !self.opening();

        // Prompt (this view only)
        out.push(Action::new("chat.insert", "Type a prompt", C::Insert).focused().importance(crate::actions::PRIMARY));
        if !self.ui.input_buffer.is_empty() {
            out.push(Action::new("chat.clear", "Clear the prompt", C::ClearPrompt).focused().importance(MINOR));
        }

        // Session
        let mut new = Action::new("session.new", "New session", C::NewSession);
        if idle {
            new = new.header(Button, "Connect", Some(icon::LIGHTNING));
        }
        out.push(new);
        if caps.sessions.list_saved {
            let param = self.saved_choices().map(Param::Choice).unwrap_or(Param::None);
            let mut resume = Action::new("session.resume", "Resume a saved session", C::ResumeSession).param(param.clone());
            if idle {
                resume = resume.header(Button, "Resume…", Some(icon::ARROW_COUNTER_CLOCKWISE));
            }
            out.push(resume);
            if caps.sessions.delete_saved {
                out.push(Action::new("session.delete", "Delete a saved session", C::DeleteSaved).param(param));
            }
        }
        if let Some(slot) = agents.slot(&self.ui.slot).filter(|s| s.sessions.len() > 1) {
            let choices = slot
                .sessions
                .iter()
                .map(|s| {
                    let label = s.title.clone().or_else(|| s.native_id.as_ref().map(|n| n.to_string())).unwrap_or_else(|| s.key.to_string());
                    Choice::new(s.key.to_string(), label).detail(s.thread.is_none().then(|| "not running".to_owned())).current(slot.current.as_ref() == Some(&s.key))
                })
                .collect();
            out.push(Action::new("session.switch", "Switch session", C::SwitchSession).param(Param::Choice(choices)));
        }
        if store.instances.len() > 1 && self.active_thread().is_none() {
            let choices = store
                .instances
                .values()
                .map(|i| {
                    let label = match &i.status {
                        ProviderStatus::Ready => i.display_name.clone(),
                        other => format!("{} ({other:?})", i.display_name),
                    };
                    Choice::new(i.instance_id.to_string(), label).current(Some(&i.instance_id) == self.active_instance().as_ref())
                })
                .collect();
            out.push(Action::new("instance.select", "Choose harness", C::SelectInstance).param(Param::Choice(choices)).header(Setting, "Harness", None));
        }
        if let Some(view) = view {
            out.push(Action::new("session.close", "Close session", C::CloseSession).header(Button, "", Some(icon::X)));
            if view.running_turn().is_some() && caps.turns.interrupt {
                out.push(Action::new("session.stop", "Stop the running turn", C::StopTurn).header(Button, "Stop", Some(icon::STOP)));
            }
        }
        out.push(Action::new("instance.refresh", "Re-probe the harness", C::RefreshInstance));

        // Agent slots: which one this view shows; starting and closing sub-slots
        if store.slots.len() > 1 {
            let choices = super::slots::ordered(store)
                .into_iter()
                .map(|s| Choice::new(s.id.to_string(), slot_label(s, store)).detail(slot_detail(s)).current(s.id == self.ui.slot))
                .collect();
            out.push(Action::new("agent.pick", "Show an agent slot", C::PickSlot).param(Param::Choice(choices)));
            out.push(Action::new("chat.next_slot", "Show the next agent slot", C::StepSlot(1)).focused());
            out.push(Action::new("chat.prev_slot", "Show the previous agent slot", C::StepSlot(-1)).focused());
        }
        if !store.instances.is_empty() {
            let choices = store.instances.values().map(|i| Choice::new(i.instance_id.to_string(), &i.display_name)).collect();
            out.push(Action::new("agent.new", "Start an agent sub-slot", C::NewSlot).param(Param::Choice(choices)));
        }
        if agents.slot(&self.ui.slot).is_some_and(|s| !s.is_lead()) {
            out.push(Action::new("agent.close", "Close this agent sub-slot", C::CloseSlot));
        }

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
                    out.push(Action::new("mode.native", "Set session mode", C::NativeMode).param(Param::Choice(choices)).header(Setting, "Mode", None));
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
                        .header(Setting, &control.name, None);
                    if let Some(d) = &control.description {
                        action = action.detail(d.clone());
                    }
                    out.push(action);
                }
            }
            None => {
                if let Some(s) = snapshot.filter(|s| !s.models.is_empty()) {
                    let model = self.model();
                    let default = Choice::new("", "default").current(model.is_none());
                    let choices = std::iter::once(default).chain(s.models.iter().map(|m| Choice::new(&m.id, &m.name).current(model.as_ref() == Some(&m.id)))).collect();
                    out.push(Action::new("model.next", "Model for the next session", C::NextModel).param(Param::Choice(choices)).header(Setting, "Model", None));
                }
                if let Some(s) = snapshot.filter(|s| !s.runtime_modes.is_empty()) {
                    let current = self.policy().runtime_mode;
                    let choices = s.runtime_modes.iter().map(|m| Choice::new(wire_name(m), mode_label(*m)).current(*m == current)).collect();
                    out.push(Action::new("mode.runtime", "Runtime mode", C::RuntimeMode).param(Param::Choice(choices)).header(Setting, "Mode", None));
                }
            }
        }

        // Tool servers and skills
        let mut servers: Vec<Choice> = match view {
            Some(v) => v.thread.mcp_servers.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect(),
            None => agents.tools().iter().flat_map(|t| t.ours.iter().chain(&t.project)).map(|m| Choice::new(&m.name, mcp_label(m))).collect(),
        };
        if let Some(s) = snapshot {
            servers.extend(s.tool_servers.iter().map(|t| Choice::new(&t.name, format!("{} — {} (its own, {:?})", t.name, t.transport, t.scope).to_lowercase()).detail(Some(t.file.clone()))));
        }
        if !servers.is_empty() {
            out.push(Action::new("mcp.list", "Tool servers its sessions get", C::ListMcp).param(Param::Choice(servers)));
        }
        if let Some(tools) = agents.tools() {
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
            out.push(Action::new("chat.inject", "Add the prompt to what it's doing", C::AddToTurn).focused().on_typing(Button, "Add to what it's doing", None));
        }

        // Harness commands; typing `/` and its start, those it matches show
        // under the box.
        let typed = self.ui.input_buffer.strip_prefix('/').filter(|p| !p.contains(' '));
        if let Some(s) = snapshot {
            for command in &s.commands {
                let param = match &command.input_hint {
                    Some(hint) => Param::Text { hint: hint.clone() },
                    None => Param::None,
                };
                let action = Action::new(format!("command.{}", command.name), format!("/{}", command.name), C::HarnessCommand { name: command.name.clone() }).param(param).detail(command.description.clone());
                out.push(if typed.is_some_and(|p| command.name.starts_with(p)) { action.on_typing(Button, format!("/{}", command.name), None) } else { action });
            }
        }

        // Account
        if let Some(s) = snapshot.filter(|s| !s.auth.methods.is_empty()) {
            let choices = s.auth.methods.iter().map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone())).collect();
            let login = Action::new("auth.login", "Sign in", C::Login).param(Param::Choice(choices));
            out.push(if self.needs_login() && agents.login().is_none() { login.header(Setting, "Sign in", Some(icon::KEY)) } else { login });
            let hint = agents.login().and_then(|l| l.expects.clone()).unwrap_or_else(|| "what the sign-in page gave you".into());
            let complete = Action::new("auth.complete", "Finish signing in", C::CompleteLogin).param(Param::Text { hint });
            out.push(if agents.login().is_some_and(|l| l.expects.is_some()) { complete.header(Button, "Finish signing in", Some(icon::KEY)) } else { complete });
            out.push(Action::new("auth.logout", "Sign out", C::Logout));
        }

        // The pending request's answers (here: the inbox offers them too, with the same keys)
        if let Some(view) = view
            && let Some(request) = view.requests.iter().find(|r| r.status == RequestStatus::Pending)
        {
            // Under the request while you're on it.
            let here = crate::hosted::Hosted::here(self).or_else(|| self.ui.cursor.map(|c| c.to_string())) == self.pending_item().map(|i| i.to_string());
            if let Some(url) = crate::request_card::page(request) {
                out.push(crate::request_card::place_page(Action::new(format!("request.{}.page", request.id), format!("Open the page: {}", request.title), C::OpenPage(url)).focused(), here));
            }
            for answer in crate::request_card::answers(request) {
                let command = C::Answer { thread: view.thread.id.clone(), request: request.id.clone(), option: answer.option.clone() };
                out.push(answer.place(Action::new(answer.id.clone(), answer.label.clone(), command).focused(), here));
            }
        }
        // Typed in: sending it.
        out.push(Action::new("chat.send", "Send the prompt", C::Send).focused().importance(crate::actions::PRIMARY).on_typing(Button, "Send", Some(icon::PAPER_PLANE_RIGHT)));
        // What the chat says on its own line, and its ways out.
        if agents.login().is_some() {
            out.push(Action::new("auth.cancel", "Stop signing in", C::CancelLogin).focused().importance(MINOR).header(Button, "Cancel sign-in", None));
            if agents.login().is_some_and(|l| l.url.is_some()) {
                out.push(Action::new("auth.page", "Open the sign-in page", C::OpenLoginPage).focused().importance(crate::actions::PRIMARY).header(Button, "Open the sign-in page", Some(icon::ARROW_SQUARE_OUT)));
            }
        }
        if agents.notice().is_some() {
            out.push(Action::new("chat.dismiss", "Put the notice away", C::DismissNotice).focused().importance(MINOR).header(Button, "Dismiss", None));
        }
        if self.error().is_some() {
            if self.active_thread().is_none() {
                out.push(Action::new("chat.retry", "Try connecting again", C::Retry).focused().header(Button, "Retry", Some(icon::ARROW_COUNTER_CLOCKWISE)));
            }
            out.push(Action::new("chat.dismiss_error", "Put the error away", C::DismissError).focused().importance(MINOR).header(Button, "Dismiss", None));
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<ChatCommand>() else { return };
        let ctx = cx.ctx;
        match command {
            ChatCommand::Insert => *cx.input_mode = crate::keymap::InputMode::Insert { panel: cx.panel },
            ChatCommand::ClearPrompt => self.ui.input_buffer.clear(),
            ChatCommand::NewSession | ChatCommand::Retry => self.open_thread(None),
            ChatCommand::ResumeSession => match value {
                Some(native) => self.open_thread(Some(NativeId::new(native))),
                None => self.load_saved(&action.id),
            },
            ChatCommand::DeleteSaved => match (value, self.active_instance()) {
                (Some(native), Some(instance)) => self.act(Act::DeleteSaved { instance, native: NativeId::new(native) }),
                (None, _) => self.load_saved(&action.id),
                _ => {}
            },
            ChatCommand::SwitchSession => {
                if let Some(key) = value {
                    self.act(Act::SwitchSession { key: SessionKey::new(key) });
                }
            }
            ChatCommand::SelectInstance => {
                if let Some(i) = value {
                    self.act(Act::SetHarness { instance: InstanceId::new(i) });
                }
            }
            ChatCommand::PickSlot => {
                if let Some(picked) = value.map(SlotId::new).filter(|s| self.agents().slot(s).is_some()) {
                    self.ui.slot = picked;
                    self.ui.cursor = None;
                }
            }
            ChatCommand::StepSlot(by) => match self.beside(*by) {
                Some(next) => {
                    self.ui.slot = next;
                    self.ui.cursor = None;
                }
                None => *cx.note = Some(if *by > 0 { "This is the last agent slot." } else { "This is the first agent slot." }.into()),
            },
            ChatCommand::NewSlot => {
                if let Some(i) = value {
                    self.act(Act::NewSlot { instance: InstanceId::new(i) });
                }
            }
            ChatCommand::CloseSlot => self.act(Act::CloseSlot),
            ChatCommand::CloseSession => {
                let key = self.slot().and_then(|s| s.current.clone());
                if let Some(key) = key {
                    self.act(Act::CloseSession { key });
                }
            }
            ChatCommand::StopTurn => {
                if let Some(thread) = self.active_thread() {
                    self.act(Act::Stop { thread });
                }
            }
            ChatCommand::RefreshInstance => {
                if let Some(instance) = self.active_instance() {
                    self.act(Act::Reprobe { instance });
                }
            }
            ChatCommand::SetConfig { option, boolean } => {
                if let Some(v) = value {
                    let json = if *boolean { serde_json::Value::Bool(v == "true") } else { serde_json::Value::String(v) };
                    self.steer(SteerInput { config: vec![OptionValue { id: option.clone(), value: json }], ..Default::default() });
                }
            }
            ChatCommand::NextModel => {
                let model = value.filter(|v| !v.is_empty()).zip(self.active_instance()).map(|(model, instance_id)| ModelSelection { instance_id, model, options: Vec::new() });
                self.act(Act::Settings { policy: self.policy(), model });
            }
            ChatCommand::NativeMode => {
                if let Some(mode) = value {
                    self.steer(SteerInput { mode: Some(mode), ..Default::default() });
                }
            }
            ChatCommand::RuntimeMode => {
                if let Some(mode) = value.and_then(|v| serde_json::from_value::<RuntimeMode>(serde_json::Value::String(v)).ok()) {
                    let policy = RuntimePolicy { runtime_mode: mode, ..self.policy() };
                    self.act(Act::Settings { policy, model: self.model_selection() });
                }
            }
            ChatCommand::ListMcp | ChatCommand::ListSkills => {}
            ChatCommand::AddToTurn => {
                let text = std::mem::take(&mut self.ui.input_buffer).trim().to_owned();
                if !text.is_empty() {
                    self.add_to_turn(text);
                }
            }
            ChatCommand::AddMcp => {
                let text = value.unwrap_or_default();
                let mut parts = text.split_whitespace();
                let project = self.agents().tools().map(|t| t.project.clone());
                match (parts.next(), parts.next(), parts.next(), project) {
                    (Some(name), Some(url), kind @ (None | Some("sse")), Some(project)) => {
                        let mut project: Vec<McpServer> = project.into_iter().filter(|m| m.name != name).collect();
                        let transport = match kind {
                            Some(_) => McpTransport::Sse { url: url.into(), headers: Vec::new() },
                            None => McpTransport::Http { url: url.into(), headers: Vec::new() },
                        };
                        project.push(McpServer { name: name.into(), transport });
                        self.act(Act::ToolServers(project));
                    }
                    _ => *cx.note = Some("A tool server needs a name and a URL, e.g. `docs https://host/mcp` (`… sse` for the older kind)".into()),
                }
            }
            ChatCommand::RemoveMcp => {
                let project = self.agents().tools().map(|t| t.project.clone());
                if let (Some(name), Some(project)) = (value, project) {
                    self.act(Act::ToolServers(project.into_iter().filter(|m| m.name != name).collect()));
                }
            }
            ChatCommand::HarnessCommand { name } => {
                // Picked while typing its start: what was typed goes.
                if self.ui.input_buffer.strip_prefix('/').is_some_and(|p| name.starts_with(p.trim())) {
                    self.ui.input_buffer.clear();
                }
                let text = match value {
                    Some(args) if !args.is_empty() => format!("/{name} {args}"),
                    _ => format!("/{name}"),
                };
                self.send_prompt(text);
            }
            ChatCommand::Login => {
                if let (Some(method), Some(instance)) = (value, self.active_instance()) {
                    self.act(Act::SignIn { instance, method });
                }
            }
            ChatCommand::CompleteLogin => {
                if let (Some(answer), Some(instance)) = (value, self.active_instance()) {
                    self.act(Act::FinishSignIn { instance, answer });
                }
            }
            ChatCommand::Logout => {
                if let Some(instance) = self.active_instance() {
                    self.act(Act::SignOut { instance });
                }
            }
            ChatCommand::CancelLogin => self.act(Act::StopSigningIn),
            ChatCommand::OpenPage(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            ChatCommand::OpenLoginPage => {
                let url = self.agents().login().and_then(|l| l.url.clone());
                if let Some(url) = url {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
            }
            ChatCommand::Send => {
                let text = std::mem::take(&mut self.ui.input_buffer).trim().to_owned();
                if !text.is_empty() {
                    self.send_prompt(text);
                }
            }
            ChatCommand::DismissNotice => self.act(Act::PutNoticeAway),
            ChatCommand::DismissError => self.act(Act::PutErrorAway),
            ChatCommand::Answer { thread, request, option } => {
                if let Err(why) = self.answer(thread, request, option) {
                    *cx.note = Some(why);
                }
            }
        }
    }

    fn take_palette_request(&mut self) -> Option<String> {
        self.pending_palette.take()
    }
}

/// A slot as the picker lists it: its name, and its harness.
fn slot_label(slot: &Slot, store: &Store) -> String {
    match slot.instance_id.as_ref().and_then(|i| store.instances.get(i)) {
        Some(instance) => format!("{} ({})", slot.name, instance.display_name),
        None => slot.name.clone(),
    }
}

/// Its role, and whether its session runs (or why it can't).
fn slot_detail(slot: &Slot) -> Option<String> {
    let state = match &slot.status {
        SlotStatus::Unavailable { reason, .. } => reason.clone(),
        SlotStatus::Ready if slot.current_thread().is_some() => "running".into(),
        SlotStatus::Ready => "not running".into(),
    };
    Some(format!("{}: {state}", slot.role))
}
