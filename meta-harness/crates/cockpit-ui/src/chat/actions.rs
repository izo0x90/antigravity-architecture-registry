//! What the chat offers as actions, and running them.

use super::*;
use crate::store::Store;

use crate::actions::wire_name;
use crate::config::Named;

#[derive(Clone, Debug)]
enum ChatCommand {
    Insert,
    ClearPrompt,
    NewSession,
    ResumeSession,
    /// The slot's last session, resumed.
    ResumeLast(NativeId),
    DeleteSaved,
    SwitchSession,
    SelectInstance,
    PickSlot,
    StepSlot(i32),
    /// To the next sub-agent (1), or the one before (-1).
    StepSubagent(i32),
    /// The sub-agent where the cursor is, opened (its own work, alone).
    OpenSubagent,
    /// The sub-agent shown alone: its work stopped, the turn going on;
    /// where it started.
    StopSubagent,
    SubagentStart,
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
    CopyLoginLink,
    OpenPage(String),
    Send,
    DismissNotice,
    PutSignInEndAway(u64),
    Retry,
    DismissError,
    Answer { thread: ThreadId, request: harness_contracts::RequestId, option: String },
    Indicator,
    Verbosity,
}

impl ActionProvider for ChatStreamState {
    /// Every agent action available right now, from the instance snapshot, the
    /// active session's controls and what is pending.
    fn actions(&self) -> Vec<Action> {
        use ChatCommand as C;
        use Style::Button;
        let mut out = self.moves();
        let agents = self.agents.clone();
        let agents = agents.get();
        let store = agents.store();
        let snapshot = agents.instance_in(&self.ui.slot);
        let caps = snapshot.map(|s| s.capabilities.clone()).unwrap_or_default();
        let view = agents.thread_in(&self.ui.slot);
        let idle = self.active_thread().is_none() && !self.opening();

        // Prompt (this view only)
        out.push(Action::new("chat.insert", "Type a prompt", "The keys go to the prompt box until you stop typing", C::Insert).focused().importance(crate::actions::PRIMARY));
        if !self.ui.input.text.is_empty() {
            out.push(Action::new("chat.clear", "Clear the prompt", "Empties what's typed, unsent", C::ClearPrompt).focused().importance(MINOR));
        }

        // The banners' ways out, under them while they're where you are.
        let on_banners = self.on_banners();
        let banner = |action: Action, caption: &str, icon: Option<&'static str>| if on_banners { action.on_item(Button, caption, icon) } else { action };

        // Session
        let new = Action::new("session.new", "New session", "A fresh conversation in this agent slot (the last one is saved)", C::NewSession);
        out.push(if idle { banner(new, "New session", Some(icon::LIGHTNING)) } else { new });
        // Its last session, not running: resumed at once.
        if let crate::agents::AgentState::NoSession { resume: Some(native) } = agents.state(&self.ui.slot) {
            out.push(banner(Action::new("session.resume_last", "Resume the last session", "Picks up this agent slot's last conversation where it was left", C::ResumeLast(native)).focused().importance(crate::actions::PRIMARY), "Resume the last", Some(icon::ARROW_COUNTER_CLOCKWISE)));
        }
        if caps.sessions.list_saved {
            let param = self.saved_choices().map(Param::Choice).unwrap_or(Param::None);
            let resume = Action::new("session.resume", "Resume a saved session", "Picks up an earlier conversation where it was left", C::ResumeSession).param(param.clone());
            out.push(if idle { banner(resume, "Resume…", Some(icon::ARROW_COUNTER_CLOCKWISE)) } else { resume });
            if caps.sessions.delete_saved {
                out.push(Action::new("session.delete", "Delete a saved session", "Forgets it, for good", C::DeleteSaved).param(param));
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
            out.push(Action::new("session.switch", "Switch session", "Shows another of this slot's open sessions", C::SwitchSession).param(Param::Choice(choices)));
        }
        if store.instances.len() > 1 && self.active_thread().is_none() {
            let choices = store
                .instances
                .values()
                .map(|i| {
                    let label = match &i.status {
                        ProviderStatus::Ready => i.display_name.clone(),
                        other => format!("{} ({})", i.display_name, crate::agents::status_words(other)),
                    };
                    Choice::new(i.instance_id.to_string(), label).current(Some(&i.instance_id) == self.active_instance().as_ref())
                })
                .collect();
            out.push(Action::new("instance.select", "Choose harness", "Which agent harness this slot's next session runs on", C::SelectInstance).param(Param::Choice(choices)).setting("Harness", None));
        }
        if let Some(view) = view {
            out.push(Action::new("session.close", "Close session", "Ends it; it stays saved to resume", C::CloseSession));
            if view.running_turn().is_some() && caps.turns.interrupt {
                out.push(Action::new("session.stop", "Stop the running turn", "The agent stops what it's doing; what it did stays", C::StopTurn));
            }
        }
        out.push(Action::new("instance.refresh", "Re-probe the harness", "Asks the harness again what it can do and whether you're signed in", C::RefreshInstance));

        // Agent slots: which one this view shows; starting and closing sub-slots
        if store.slots.len() > 1 {
            let choices = super::slots::ordered(store)
                .into_iter()
                .map(|s| Choice::new(s.id.to_string(), slot_label(s, store)).detail(Some(format!("{}: {}", s.role, agents.state(&s.id).words()))).current(s.id == self.ui.slot))
                .collect();
            out.push(Action::new("agent.pick", "Show an agent slot", "Pick one of the agents the server runs, for this chat", C::PickSlot).param(Param::Choice(choices)));
            out.push(Action::new("chat.next_slot", "Show the next agent slot", "This chat shows the next agent the server runs", C::StepSlot(1)).focused());
            out.push(Action::new("chat.prev_slot", "Show the previous agent slot", "This chat shows the agent before", C::StepSlot(-1)).focused());
        }
        // Its sub-agents: the one you're on opened; from one to the next.
        // Shown alone: stopping it (where its harness can), where it
        // started.
        if self.subagent_here().is_some() {
            out.push(Action::new("chat.open_subagent", "Open the sub-agent", "Its steps as they come, what it was told, stopping it", C::OpenSubagent).focused().importance(crate::actions::PRIMARY).group(crate::actions::Group::GoTo).on_item(crate::actions::Style::Button, "Open", Some(icon::ROBOT)));
        }
        if let Some(seen) = self.ui.alone.as_ref().and_then(|id| Some(self.thread()?.subagent(id)?.subagent.clone())) {
            if seen.status.working() {
                let stop = Action::new("chat.stop_subagent", "Stop the sub-agent", "Stops its work alone; the agent's turn goes on", C::StopSubagent).focused().importance(crate::actions::PRIMARY).on_view(crate::actions::Style::Button, "Stop it", Some(icon::STOP));
                out.push(match (seen.can_cancel, agents.stopping(&seen.id)) {
                    (true, false) => stop,
                    (true, true) => stop.when(crate::actions::When::Unavailable).detail("Stopping it…"),
                    (false, _) => stop.when(crate::actions::When::Unavailable).detail("Its harness can't stop one sub-agent alone: stopping the turn stops it"),
                });
            }
            out.push(Action::new("chat.subagent_start", "Where the sub-agent started", "Its line in the agent's chat, in a panel", C::SubagentStart).focused().group(crate::actions::Group::GoTo).on_view(crate::actions::Style::Button, "Where it started", Some(icon::ARROW_BEND_UP_LEFT)));
        }
        let any = self.thread().is_some_and(|v| !v.subagents().is_empty());
        for (id, label, by) in [("chat.next_subagent", "Next sub-agent", 1), ("chat.prev_subagent", "Previous sub-agent", -1)] {
            let action = Action::new(id, label, "Where the next (or previous) sub-agent the agent started is, in the chat", C::StepSubagent(by)).focused().importance(crate::actions::MINOR).group(crate::actions::Group::Navigate);
            out.push(if any { action } else { action.when(crate::actions::When::Unavailable) });
        }
        if !store.instances.is_empty() {
            let choices = store.instances.values().map(|i| Choice::new(i.instance_id.to_string(), &i.display_name)).collect();
            out.push(Action::new("agent.new", "Start an agent sub-slot", "Another agent beside the lead, on a harness you pick", C::NewSlot).param(Param::Choice(choices)));
        }
        if agents.slot(&self.ui.slot).is_some_and(|s| !s.is_lead()) {
            out.push(Action::new("agent.close", "Close this agent sub-slot", "Its sessions end and the slot goes", C::CloseSlot));
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
                    out.push(Action::new("mode.native", "Set session mode", "The harness's own modes (what it may do without asking)", C::NativeMode).param(Param::Choice(choices)).setting("Mode", None));
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
                    let description = match control.description.as_deref().map(str::trim) {
                        Some(description) if !description.is_empty() => description.to_owned(),
                        _ => format!("The harness's {} setting", control.name),
                    };
                    let action = Action::new(format!("config.{}", control.id), format!("Set {}", control.name), description, C::SetConfig { option: control.id.clone(), boolean })
                        .alias(format!("config.category.{}", wire_name(&control.category)))
                        .param(param)
                        .setting(&control.name, None);
                    out.push(action);
                }
            }
            None => {
                if let Some(s) = snapshot.filter(|s| !s.models.is_empty()) {
                    let model = self.model();
                    let default = Choice::new("", "default").current(model.is_none());
                    let choices = std::iter::once(default).chain(s.models.iter().map(|m| Choice::new(&m.id, &m.name).current(model.as_ref() == Some(&m.id)))).collect();
                    out.push(Action::new("model.next", "Model for the next session", "Which model the next session in this slot starts with", C::NextModel).param(Param::Choice(choices)).setting("Model", None));
                }
                if let Some(s) = snapshot.filter(|s| !s.runtime_modes.is_empty()) {
                    let current = self.policy().runtime_mode;
                    let choices = s.runtime_modes.iter().map(|m| Choice::new(wire_name(m), mode_label(*m)).current(*m == current)).collect();
                    out.push(Action::new("mode.runtime", "Runtime mode", "What the agent may do without asking you (files, commands)", C::RuntimeMode).param(Param::Choice(choices)).setting("Mode", None));
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
            out.push(Action::new("mcp.list", "Tool servers its sessions get", "The MCP servers this slot's sessions are given", C::ListMcp).param(Param::Choice(servers)));
        }
        if let Some(tools) = agents.tools() {
            out.push(Action::new("mcp.add", "Add a tool server to the project", "Every session in the project gets it from the next one on", C::AddMcp).param(Param::Text { hint: "name https://host/mcp (then sse for the older kind)".into(), start: String::new() }));
            if !tools.project.is_empty() {
                let choices = tools.project.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
                out.push(Action::new("mcp.remove", "Remove a tool server from the project", "Sessions from the next one on don't get it", C::RemoveMcp).param(Param::Choice(choices)));
            }
        }
        if let Some(s) = snapshot.filter(|s| !s.skills.is_empty()) {
            let choices = s
                .skills
                .iter()
                .map(|k| Choice::new(&k.name, format!("{} ({})", k.name, wire_name(&k.scope))).detail(k.description.clone().or(k.path.clone())))
                .collect();
            out.push(Action::new("harness.skills", format!("{}'s skills", s.display_name), "What the harness can be told to do by name", C::ListSkills).param(Param::Choice(choices)));
        }
        // While the box is a sign-in's, nothing typed goes to the agent.
        let to_agent = self.sign_in_box().is_none();
        if let Some(view) = view
            && to_agent
            && view.running_turn().is_some()
            && caps.turns.active_steering
            && !self.ui.input.text.trim().is_empty()
        {
            out.push(Action::new("chat.inject", "Add the prompt to what it's doing", "Into the running turn, rather than after it", C::AddToTurn).focused().typed(crate::keymap::Sent::Add).on_typing(Button, "Add to what it's doing", None));
        }

        // Harness commands; typing `/` and its start, those it matches show
        // under the box.
        let typed = self.ui.input.text.strip_prefix('/').filter(|p| !p.contains(' ') && to_agent);
        if let Some(s) = snapshot {
            for command in &s.commands {
                let param = match &command.input_hint {
                    Some(hint) => Param::Text { hint: hint.clone(), start: String::new() },
                    None => Param::None,
                };
                let description = if command.description.trim().is_empty() { format!("The harness's /{} command", command.name) } else { command.description.clone() };
                let action = Action::new(format!("command.{}", command.name), format!("/{}", command.name), description, C::HarnessCommand { name: command.name.clone() }).param(param);
                out.push(if typed.is_some_and(|p| command.name.starts_with(p)) { action.on_typing(Button, format!("/{}", command.name), None) } else { action });
            }
        }

        // Account
        if let Some(s) = snapshot.filter(|s| !s.auth.methods.is_empty()) {
            let choices = s.auth.methods.iter().map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone())).collect();
            let login = Action::new("auth.login", "Sign in", "Sign the harness in, the way it offers", C::Login).param(Param::Choice(choices));
            let now = self.sign_in_now();
            // Signing in (again): when it isn't, or the last one stopped
            // waiting; not while one's being asked for.
            let again = now.as_ref().is_none_or(|n| n.ran_out) && !agents.signing_in(&s.instance_id);
            out.push(if again && (self.needs_login() || now.is_some() || self.sign_in_end_to_say().is_some()) { banner(login, "Sign in", Some(icon::KEY)) } else { login });
            // Finishing is the typing box's while a sign-in has it: this sends
            // what's in it, or puts you in it to paste.
            if self.sign_in_box().is_some_and(|n| !n.pasted.as_ref().is_some_and(|p| p.finishing)) {
                out.push(banner(Action::new("auth.complete", "Finish signing in", "Sends what's pasted in the box to the sign-in; with nothing there, puts you in the box to paste", C::CompleteLogin).focused().importance(crate::actions::PRIMARY), "Finish signing in", Some(icon::KEY)));
            }
            out.push(Action::new("auth.logout", "Sign out", "The harness forgets your sign-in on this machine", C::Logout));
        }

        // Each request waiting on you, answered (the inbox offers them too,
        // with the same keys): under each, always; its keys (`y`, `d`…) the
        // one's you're on, else the first's, so several at once are each
        // answered and no key is two's.
        if let Some(view) = view {
            let pending = self.pending_in(view, &agents);
            let on = self.on_item();
            let on_request = pending.iter().find(|(index, _)| index.is_some() && *index == on).map(|(_, r)| r.id.clone());
            let keyed = crate::request_card::keyed(pending.iter().map(|(_, r)| *r), on_request.as_ref());
            for (index, request) in &pending {
                let under = index.map(|i| i.to_string());
                let under = under.as_deref().map_or(crate::request_card::Under::Nowhere, crate::request_card::Under::Place);
                let at = crate::request_card::Offer { under, keyed: keyed.as_ref() == Some(&request.id) };
                let answer = |option| C::Answer { thread: view.thread.id.clone(), request: request.id.clone(), option };
                out.extend(crate::request_card::offer(request, at, C::OpenPage, answer));
            }
        }
        // Its settings: how much of what the agent does it shows (what's
        // folded at first), what shows the agent at work.
        fn choices<T: Named>(current: T) -> Param {
            Param::Choice(T::ALL.iter().map(|c| Choice::new(c.name(), c.words()).detail(Some(c.detail().into())).current(*c == current)).collect())
        }
        out.push(Action::new("chat.verbosity", "How much it shows", "Quiet: messages and what needs you; normal: calls folded into runs; everything: every call and its output", C::Verbosity).param(choices(self.ui.folds.verbosity())).setting("Shows", None).group(crate::actions::Group::View).importance(MINOR));
        out.push(Action::new("chat.indicator", "What shows the agent at work", "Beside the running turn's words: a heartbeat of its updates, or nothing", C::Indicator).param(choices(self.ui.indicator.kind())).setting("At work", None).group(crate::actions::Group::View).importance(MINOR));
        // Typed in: sending it (to the sign-in, while one waits for it).
        out.push(match self.sign_in_box() {
            Some(_) => Action::new("chat.send", "Send to the sign-in", "What's typed goes to the sign-in waiting for it, not the agent", C::Send).on_typing(Button, "Send to the sign-in", Some(icon::KEY)),
            None => Action::new("chat.send", "Send the prompt", "To this slot's session (one opens if there's none); #name hands it what a tag is on", C::Send).on_typing(Button, "Send", Some(icon::PAPER_PLANE_RIGHT)),
        }.focused().importance(crate::actions::PRIMARY).typed(crate::keymap::Sent::Send));
        // The sign-in's steps, putting things away, trying again.
        if let Some(now) = self.sign_in_now() {
            out.push(banner(Action::new("auth.cancel", "Stop signing in", "The sign-in stops waiting; nothing else changes", C::CancelLogin).focused().importance(MINOR), "Stop signing in", None));
            if now.waiting.url.is_some() && !now.ran_out {
                out.push(banner(Action::new("auth.page", "Open the sign-in page", "In a new tab, here", C::OpenLoginPage).focused().importance(crate::actions::PRIMARY), "Open the sign-in page", Some(icon::ARROW_SQUARE_OUT)));
                // Not minor, unlike other copies: here it's one of the two ways to the page.
                out.push(banner(Action::new("auth.copy", "Copy the sign-in link", "To open it somewhere else: another browser, another device", C::CopyLoginLink).focused(), "Copy link", Some(icon::COPY)));
            }
        }
        if let Some(ended) = self.sign_in_end_to_say().filter(|_| self.sign_in_now().is_none()) {
            out.push(banner(Action::new("auth.end.dismiss", "Put the sign-in's end away", "Off this chat", C::PutSignInEndAway(ended.id)).focused().importance(MINOR), "Put away", None));
        }
        if agents.notice().is_some() {
            out.push(banner(Action::new("chat.dismiss", "Put the notice away", "Off the chat", C::DismissNotice).focused().importance(MINOR), "Put away", None));
        }
        if self.error().is_some() {
            if self.active_thread().is_none() {
                out.push(banner(Action::new("chat.retry", "Try connecting again", "To the harness, now", C::Retry).focused(), "Retry", Some(icon::ARROW_COUNTER_CLOCKWISE)));
            }
            out.push(banner(Action::new("chat.dismiss_error", "Put the error away", "Off the chat", C::DismissError).focused().importance(MINOR), "Put away", None));
        }
        // Shown alone, a sub-agent's: the agent's own settings (its harness,
        // mode, model) aren't on its row (they'd read as the sub-agent's);
        // still offered, by their keys and the action list.
        if self.ui.alone.is_some() {
            const ITS: [&str; 3] = ["chat.stop_subagent", "chat.subagent_start", "chat.verbosity"];
            for action in &mut out {
                if action.at() == Some(crate::actions::At::Settings) && !ITS.contains(&action.id.as_str()) {
                    action.control = None;
                }
            }
        }
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<ChatCommand>() else { return };
        let ctx = cx.ctx;
        match command {
            ChatCommand::Insert => *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at },
            ChatCommand::ClearPrompt => self.ui.input.clear(),
            ChatCommand::NewSession | ChatCommand::Retry => self.open_thread(None),
            ChatCommand::ResumeLast(native) => self.open_thread(Some(native.clone())),
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
                    self.cursor_to(None);
                }
            }
            ChatCommand::StepSlot(by) => match self.beside(*by) {
                Some(next) => {
                    self.ui.slot = next;
                    self.cursor_to(None);
                }
                None => *cx.note = Some(if *by > 0 { "This is the last agent slot." } else { "This is the first agent slot." }.into()),
            },
            ChatCommand::OpenSubagent => {
                self.open_subagent_here();
            }
            ChatCommand::StopSubagent => {
                if let (Some(id), Some(thread)) = (self.ui.alone.clone(), self.active_thread()) {
                    self.act(Act::StopSubagent { thread, id });
                }
            }
            ChatCommand::SubagentStart => {
                if let Some(id) = self.ui.alone.clone() {
                    self.asks.push(Ask::Show { target: Target::SubagentStart { slot: self.ui.slot.clone(), id }, in_: Some(crate::kind::BufferKind::Chat) });
                }
            }
            ChatCommand::StepSubagent(by) => {
                if let Step::Nothing(why) = self.step_to_subagent(*by) {
                    *cx.note = Some(why);
                }
            }
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
            ChatCommand::AddToTurn => self.send_typed(true, cx.note),
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
                if self.ui.input.text.strip_prefix('/').is_some_and(|p| name.starts_with(p.trim())) {
                    self.ui.input.take();
                }
                let text = match value {
                    Some(args) if !args.is_empty() => format!("/{name} {args}"),
                    _ => format!("/{name}"),
                };
                self.send_prompt(text.into());
            }
            ChatCommand::Login => {
                if let (Some(method), Some(instance)) = (value, self.active_instance()) {
                    self.act(Act::SignIn { instance, method });
                }
            }
            ChatCommand::CompleteLogin => {
                if self.ui.input.text.trim().is_empty() {
                    *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at };
                    *cx.note = Some("Paste what the page gave back here, and send it.".into());
                } else {
                    self.send_typed(false, cx.note);
                }
            }
            ChatCommand::Logout => {
                if let Some(instance) = self.active_instance() {
                    self.act(Act::SignOut { instance });
                }
            }
            ChatCommand::CancelLogin => {
                if let (Some(instance), Some(now)) = (self.active_instance(), self.sign_in_now()) {
                    self.act(Act::StopSigningIn { instance, id: now.waiting.id });
                }
            }
            ChatCommand::OpenPage(url) => ctx.open_url(egui::OpenUrl::new_tab(url)),
            ChatCommand::CopyLoginLink => {
                if let Some(url) = self.sign_in_now().and_then(|n| n.waiting.url) {
                    *cx.note = Some(crate::nav::Copyable::new("the sign-in link", url.as_str()).said());
                    ctx.copy_text(url);
                }
            }
            ChatCommand::OpenLoginPage => {
                let url = self.sign_in_now().and_then(|n| n.waiting.url);
                if let Some(url) = url {
                    ctx.open_url(egui::OpenUrl::new_tab(url));
                }
            }
            ChatCommand::Send => self.send_typed(false, cx.note),
            ChatCommand::DismissNotice => self.act(Act::PutNoticeAway),
            ChatCommand::PutSignInEndAway(id) => self.act(Act::PutSignInEndAway { id: *id }),
            ChatCommand::DismissError => self.act(Act::PutErrorAway),
            ChatCommand::Answer { thread, request, option } => {
                if let Err(why) = self.answer(thread, request, option) {
                    *cx.note = Some(why);
                }
            }
            // Its defaults change: what was folded or opened by hand starts
            // afresh (the same one: nothing changes).
            ChatCommand::Verbosity => {
                if let Some(verbosity) = value.as_deref().and_then(Named::named) {
                    self.ui.picked.entry(self.ui.slot.clone()).or_default().shows = Some(verbosity);
                    self.settle();
                }
            }
            ChatCommand::Indicator => {
                if let Some(kind) = value.as_deref().and_then(Named::named) {
                    self.ui.picked.entry(self.ui.slot.clone()).or_default().at_work = Some(kind);
                    self.settle();
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


