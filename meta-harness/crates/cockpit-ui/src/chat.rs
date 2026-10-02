//! The chat/trajectory view. Renders only from the store (server events) and the
//! active instance's snapshot: pickers, commands and controls come from what the
//! harness reports and its capabilities, never from hardcoded harness knowledge.

use std::sync::{Arc, Mutex};

use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ApprovalDecision, ClientEvent, ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestResponse, RequestStatus, RuntimeMode, RuntimePolicy, SavedSession,
    StepStatus, SteerInput, ThreadId, ThreadStatus, ToolKind, TurnInput, TurnItem,
};

use egui_phosphor::regular as icon;

use crate::actions::{Action, Choice, Invocation, Owner, Param};
use crate::feedback::FeedbackInspectorState;
use crate::net::{NetClient, NetResult, OpenThreadBody};
use crate::store::{Store, ThreadView};

/// Agent connection as the status LED and banner see it.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionStatus {
    Idle,
    Connecting,
    Connected(String),
    Failed(String),
}

/// Results of async calls, drained once per frame.
enum Reply {
    Opened(NetResult<ThreadId>),
    Sent(NetResult<()>),
    Saved(NetResult<Vec<SavedSession>>),
    AuthStarted(NetResult<String>),
    Done(NetResult<()>),
}

type Mailbox = Arc<Mutex<Vec<Reply>>>;

fn post(mailbox: &Mailbox, reply: Reply) {
    if let Ok(mut m) = mailbox.lock() {
        m.push(reply);
    }
}

pub struct ChatStreamState {
    pub store: Store,
    pub active_instance: Option<InstanceId>,
    pub active_thread: Option<ThreadId>,
    opening: bool,
    error: Option<String>,
    queued_prompt: Option<String>,
    pub input_buffer: String,
    pub markdown_cache: CommonMarkCache,
    pub requested_file_jump: Option<(String, Option<usize>)>,
    runtime_mode: RuntimeMode,
    model: Option<String>,
    saved_sessions: Option<Vec<SavedSession>>,
    /// Action to open in the palette once saved sessions arrive.
    after_saved: Option<&'static str>,
    /// Asks the app to open the palette at this action (set by toolbar buttons
    /// and by actions that first need to load their choices).
    pub pending_palette: Option<String>,
    /// An approval answered from the keyboard, sent on the next frame.
    pending_answer: Option<(ThreadId, harness_contracts::RequestId, String)>,
    /// MCP servers the user added; passed to new sessions and reconnects.
    extra_mcp: Vec<McpServer>,
    auth_instructions: Option<String>,
    auth_paste: String,
    mailbox: Mailbox,
}

impl Default for ChatStreamState {
    fn default() -> Self {
        Self {
            store: Store::default(),
            active_instance: None,
            active_thread: None,
            opening: false,
            error: None,
            queued_prompt: None,
            input_buffer: String::new(),
            markdown_cache: CommonMarkCache::default(),
            requested_file_jump: None,
            runtime_mode: RuntimeMode::ApprovalRequired,
            model: None,
            saved_sessions: None,
            after_saved: None,
            pending_palette: None,
            pending_answer: None,
            extra_mcp: Vec::new(),
            auth_instructions: None,
            auth_paste: String::new(),
            mailbox: Mailbox::default(),
        }
    }
}

fn mode_label(mode: RuntimeMode) -> String {
    match mode {
        RuntimeMode::ApprovalRequired => format!("{} Ask first", icon::SHIELD_CHECK),
        RuntimeMode::AutoAcceptEdits => format!("{} Auto-edit", icon::PENCIL_SIMPLE),
        RuntimeMode::Auto => format!("{} Auto", icon::LIGHTNING),
        RuntimeMode::FullAccess => format!("{} Full access", icon::FIRE),
    }
}

fn tool_icon(kind: &ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => icon::BOOK_OPEN,
        ToolKind::Edit => icon::PENCIL_SIMPLE,
        ToolKind::Delete => icon::TRASH,
        ToolKind::Move => icon::ARROWS_LEFT_RIGHT,
        ToolKind::Search => icon::MAGNIFYING_GLASS,
        ToolKind::Execute => icon::PLAY,
        ToolKind::Think => icon::LIGHTBULB,
        ToolKind::Fetch => icon::GLOBE,
        ToolKind::Other => icon::WRENCH,
    }
}

fn status_tag(status: ItemStatus) -> (&'static str, Color32) {
    match status {
        ItemStatus::Pending => ("[..]", Color32::from_rgb(140, 150, 165)),
        ItemStatus::InProgress => ("[RUN]", Color32::from_rgb(250, 180, 50)),
        ItemStatus::Completed => ("[OK]", Color32::from_rgb(80, 200, 120)),
        ItemStatus::Failed => ("[FAIL]", Color32::from_rgb(240, 80, 80)),
        ItemStatus::Cancelled => ("[--]", Color32::from_rgb(140, 150, 165)),
    }
}

fn banner(ui: &mut egui::Ui, fill: Color32, stroke: Color32, add: impl FnOnce(&mut egui::Ui)) {
    Frame::NONE
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(4.0)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| ui.horizontal_wrapped(add));
    ui.add_space(4.0);
}

impl ChatStreamState {
    pub fn process_event(&mut self, event: ClientEvent) {
        if let ClientEvent::InstanceUpdated { snapshot } = &event
            && self.active_instance.is_none()
        {
            self.active_instance = Some(snapshot.instance_id.clone());
        }
        if let ClientEvent::ThreadClosed { thread_id } = &event
            && self.active_thread.as_ref() == Some(thread_id)
        {
            self.active_thread = None;
        }
        // After a reload the server replays live sessions; adopt one if none is active.
        if let ClientEvent::Session { event: harness_contracts::AdapterEvent::ThreadUpdated { thread } } = &event
            && self.active_thread.is_none()
            && !self.opening
            && thread.status != ThreadStatus::Closed
        {
            self.active_thread = Some(thread.id.clone());
            self.active_instance = Some(thread.instance_id.clone());
        }
        self.store.apply(event);
    }

    fn instance(&self) -> Option<&ProviderSnapshot> {
        self.active_instance.as_ref().and_then(|id| self.store.instances.get(id))
    }

    fn thread(&self) -> Option<&ThreadView> {
        self.active_thread.as_ref().and_then(|id| self.store.threads.get(id))
    }

    pub fn session_status(&self) -> SessionStatus {
        if let Some(err) = &self.error {
            return SessionStatus::Failed(err.clone());
        }
        match self.thread().map(|t| t.thread.status) {
            Some(ThreadStatus::Starting) => SessionStatus::Connecting,
            Some(ThreadStatus::Error) => SessionStatus::Failed("the agent reported an error".into()),
            Some(ThreadStatus::Closed) => SessionStatus::Failed("the agent session closed".into()),
            Some(_) => SessionStatus::Connected(self.active_thread.as_ref().map(ToString::to_string).unwrap_or_default()),
            None if self.opening => SessionStatus::Connecting,
            None => SessionStatus::Idle,
        }
    }

    pub fn agent_activity(&self) -> u64 {
        self.thread().map(|t| t.activity).unwrap_or(0)
    }

    fn policy(&self) -> RuntimePolicy {
        RuntimePolicy { runtime_mode: self.runtime_mode, ..RuntimePolicy::default() }
    }

    fn model_selection(&self) -> Option<ModelSelection> {
        Some(ModelSelection { instance_id: self.active_instance.clone()?, model: self.model.clone()?, options: Vec::new() })
    }

    fn open_thread(&mut self, net: &NetClient, ctx: &egui::Context, resume: Option<harness_contracts::NativeId>) {
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
            mcp_servers: self.extra_mcp.clone(),
        };
        let mailbox = self.mailbox.clone();
        net.open_thread(&body, ctx.clone(), move |r| post(&mailbox, Reply::Opened(r)));
    }

    fn send_prompt(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
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

    fn steer(&mut self, net: &NetClient, ctx: &egui::Context, input: SteerInput) {
        if let Some(thread) = &self.active_thread {
            let mailbox = self.mailbox.clone();
            net.steer(thread, &input, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
        }
    }

    /// The single place async results land.
    fn drain_replies(&mut self, net: &NetClient, ctx: &egui::Context) {
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
                    self.error = Some(e);
                }
                Reply::Saved(Ok(sessions)) => {
                    self.saved_sessions = Some(sessions);
                    if let Some(action) = self.after_saved.take() {
                        self.pending_palette = Some(action.to_owned());
                    }
                }
                Reply::AuthStarted(Ok(instructions)) => {
                    self.error = None;
                    self.auth_instructions = Some(instructions);
                }
                Reply::Sent(Err(e)) | Reply::Saved(Err(e)) | Reply::AuthStarted(Err(e)) | Reply::Done(Err(e)) => {
                    self.error = Some(e);
                }
                Reply::Sent(Ok(())) | Reply::Done(Ok(())) => {}
            }
        }
    }

    pub fn execute_action(&mut self, action: crate::keymap::ChatAction, input_mode: &mut crate::keymap::InputMode, slot_idx: usize) {
        match action {
            crate::keymap::ChatAction::EnterInsert => *input_mode = crate::keymap::InputMode::Insert { slot: slot_idx },
            crate::keymap::ChatAction::ClearPrompt => self.input_buffer.clear(),
        }
    }

    pub fn handle_input(
        &mut self,
        i: &egui::InputState,
        keymap: &crate::keymap::ChatKeymap,
        input_mode: &mut crate::keymap::InputMode,
        slot_idx: usize,
    ) {
        if keymap.enter_prompt.is_pressed(i) {
            self.execute_action(crate::keymap::ChatAction::EnterInsert, input_mode, slot_idx);
        }
        if keymap.clear_prompt.is_pressed(i) {
            self.execute_action(crate::keymap::ChatAction::ClearPrompt, input_mode, slot_idx);
        }
        // Pending approval: y = accept once, Y = accept for the session, n = decline.
        let typed: Option<char> = i.events.iter().find_map(|e| match e {
            egui::Event::Text(t) => t.chars().next(),
            _ => None,
        });
        let wanted = match typed {
            Some('y') => Some(ApprovalDecision::Accept),
            Some('Y') => Some(ApprovalDecision::AcceptForSession),
            Some('n') => Some(ApprovalDecision::Decline),
            _ => None,
        };
        if let Some(decision) = wanted {
            self.answer_pending(decision);
        }
    }

    /// Answers the oldest pending request with the option matching `decision`.
    fn answer_pending(&mut self, decision: ApprovalDecision) {
        let Some(view) = self.thread() else { return };
        let Some(request) = view.requests.iter().find(|r| r.status == RequestStatus::Pending) else { return };
        let option = request
            .options
            .iter()
            .find(|o| o.decision == decision)
            .or_else(|| match decision {
                ApprovalDecision::AcceptForSession => request.options.iter().find(|o| o.decision == ApprovalDecision::AcceptAlways),
                _ => None,
            });
        if let Some(option) = option {
            self.pending_answer = Some((view.thread.id.clone(), request.id.clone(), option.id.clone()));
        }
    }

    pub fn render(
        &mut self,
        ui: &mut egui::Ui,
        net: &NetClient,
        feedback: &mut FeedbackInspectorState,
        slot_idx: usize,
        is_focused: bool,
        input_mode: &mut crate::keymap::InputMode,
    ) {
        let ctx = ui.ctx().clone();
        self.drain_replies(net, &ctx);
        if let Some((thread, request, option)) = self.pending_answer.take() {
            let response = RequestResponse { option_id: Some(option), ..Default::default() };
            let mailbox = self.mailbox.clone();
            net.respond(&thread, &request, &response, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
        }

        ui.vertical(|ui| {
            let start = ui.cursor().min;
            let w = ui.available_width();
            self.render_controls(ui, net);
            let controls = Rect::from_min_size(start, Vec2::new(w, (ui.cursor().min.y - start.y).max(24.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:controls", slot_idx + 1), controls);
            ui.separator();

            self.render_banners(ui, net);
            self.render_plan(ui);

            let scroll_height = (ui.available_height() - 105.0).max(50.0);
            let stream = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .max_height(scroll_height)
                .show(ui, |ui| self.render_stream(ui, net));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:stream", slot_idx + 1), stream.inner_rect);
            ui.separator();

            let start = ui.cursor().min;
            let w = ui.available_width();
            self.render_input(ui, net, feedback, slot_idx, is_focused, input_mode);
            let input = Rect::from_min_size(Pos2::new(start.x, start.y), Vec2::new(w, (ui.cursor().min.y - start.y).max(40.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:input_bar", slot_idx + 1), input);
        });
    }

    fn render_controls(&mut self, ui: &mut egui::Ui, net: &NetClient) {
        let ctx = ui.ctx().clone();
        let instances: Vec<ProviderSnapshot> = self.store.instances.values().cloned().collect();
        let snapshot = self.instance().cloned();
        let caps = snapshot.as_ref().map(|s| s.capabilities.clone()).unwrap_or_default();

        ui.horizontal(|ui| {
            let selected = snapshot.as_ref().map(|s| s.display_name.clone()).unwrap_or_else(|| "No harness".into());
            ui.add_enabled_ui(self.active_thread.is_none(), |ui| {
                egui::ComboBox::from_id_salt("chat_instance").width(130.0).selected_text(selected).show_ui(ui, |ui| {
                    for inst in &instances {
                        let label = match &inst.status {
                            ProviderStatus::Ready => inst.display_name.clone(),
                            other => format!("{} ({other:?})", inst.display_name),
                        };
                        ui.selectable_value(&mut self.active_instance, Some(inst.instance_id.clone()), label);
                    }
                });
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                match self.active_thread.clone() {
                    Some(thread) => {
                        if ui.small_button(icon::X).on_hover_text("Close session").clicked() {
                            net.close_thread(&thread, ctx.clone());
                            self.active_thread = None;
                        }
                        let running = self.thread().and_then(|t| t.running_turn()).is_some();
                        if running && caps.turns.interrupt && ui.small_button(format!("{} Stop", icon::STOP)).clicked() {
                            net.interrupt(&thread, ctx.clone());
                        }
                        let short: String = thread.as_str().chars().take(8).collect();
                        ui.label(RichText::new(format!("[{short}]")).monospace().size(10.0).color(Color32::from_rgb(140, 200, 255)));
                    }
                    None if self.opening => {
                        ui.label(RichText::new(format!("{} Connecting…", icon::HOURGLASS)).size(10.0).color(Color32::from_rgb(250, 180, 50)));
                    }
                    None => {
                        if ui.button(format!("{} Connect", icon::LIGHTNING)).clicked() {
                            self.open_thread(net, &ctx, None);
                        }
                        if caps.sessions.list_saved && ui.button(format!("{} Resume…", icon::ARROW_COUNTER_CLOCKWISE)).clicked() {
                            self.load_saved(net, &ctx, "session.resume");
                        }
                    }
                }
            });
        });

        ui.horizontal_wrapped(|ui| {
            match self.thread().map(|v| v.thread.clone()) {
                // Live session: render exactly the controls the harness reports.
                Some(thread) => {
                    let controls = &thread.controls;
                    // A mode-category config option supersedes the legacy mode list.
                    let has_mode_option = controls.config.iter().any(|c| c.category == harness_contracts::ConfigCategory::Mode);
                    if !controls.modes.is_empty() && !has_mode_option {
                        let current = controls.current_mode.clone().unwrap_or_default();
                        let shown = controls.modes.iter().find(|m| m.id == current).map(|m| m.name.clone()).unwrap_or(current.clone());
                        let mut chosen = current.clone();
                        ui.label(RichText::new("Mode:").strong().size(11.0));
                        egui::ComboBox::from_id_salt("chat_native_mode").width(110.0).selected_text(shown).show_ui(ui, |ui| {
                            for m in &controls.modes {
                                ui.selectable_value(&mut chosen, m.id.clone(), &m.name);
                            }
                        });
                        if chosen != current {
                            self.steer(net, &ctx, SteerInput { mode: Some(chosen), ..Default::default() });
                        }
                    }
                    for control in &controls.config {
                        match &control.kind {
                            ConfigKind::Select { choices, current } => {
                                let shown = choices.iter().find(|c| &c.value == current).map(|c| c.name.clone()).unwrap_or(current.clone());
                                let mut chosen = current.clone();
                                ui.label(RichText::new(format!("{}:", control.name)).strong().size(11.0));
                                egui::ComboBox::from_id_salt(("chat_config", control.id.as_str())).width(130.0).selected_text(shown).show_ui(ui, |ui| {
                                    for c in choices {
                                        ui.selectable_value(&mut chosen, c.value.clone(), &c.name);
                                    }
                                });
                                if &chosen != current {
                                    let value = serde_json::Value::String(chosen);
                                    self.steer(net, &ctx, SteerInput { config: vec![OptionValue { id: control.id.clone(), value }], ..Default::default() });
                                }
                            }
                            ConfigKind::Boolean { current } => {
                                let mut on = *current;
                                if ui.checkbox(&mut on, RichText::new(&control.name).size(11.0)).changed() {
                                    let value = serde_json::Value::Bool(on);
                                    self.steer(net, &ctx, SteerInput { config: vec![OptionValue { id: control.id.clone(), value }], ..Default::default() });
                                }
                            }
                        }
                    }
                    if !thread.mcp_servers.is_empty() {
                        let names: Vec<String> = thread.mcp_servers.iter().map(mcp_label).collect();
                        ui.label(RichText::new(format!("{} MCP {}", icon::PLUGS_CONNECTED, thread.mcp_servers.len())).size(10.0).color(Color32::GRAY))
                            .on_hover_text(names.join("\n"));
                    }
                }
                // Before a session: choose what the next session starts with.
                None => {
                    if let Some(snapshot) = &snapshot {
                        if !snapshot.models.is_empty() {
                            ui.label(RichText::new("Model:").strong().size(11.0));
                            let shown = self.model.clone().unwrap_or_else(|| "default".into());
                            egui::ComboBox::from_id_salt("chat_model").width(120.0).selected_text(shown).show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.model, None, "default");
                                for m in &snapshot.models {
                                    ui.selectable_value(&mut self.model, Some(m.id.clone()), &m.name);
                                }
                            });
                        }
                        if !snapshot.runtime_modes.is_empty() {
                            ui.label(RichText::new("Mode:").strong().size(11.0));
                            egui::ComboBox::from_id_salt("chat_mode").width(110.0).selected_text(mode_label(self.runtime_mode)).show_ui(ui, |ui| {
                                for mode in &snapshot.runtime_modes {
                                    ui.selectable_value(&mut self.runtime_mode, *mode, mode_label(*mode));
                                }
                            });
                        }
                    }
                }
            }
            if let Some(usage) = self.thread().and_then(|t| t.usage.clone())
                && let (Some(used), Some(window)) = (usage.context_used_tokens, usage.context_window_tokens)
            {
                ui.label(RichText::new(format!("ctx {:.1}k/{:.0}k", used as f64 / 1000.0, window as f64 / 1000.0)).monospace().size(10.0).color(Color32::GRAY));
            }
        });
    }

    fn render_banners(&mut self, ui: &mut egui::Ui, net: &NetClient) {
        let ctx = ui.ctx().clone();
        if let Some(err) = self.error.clone() {
            let mut retry = false;
            let mut dismiss = false;
            banner(ui, Color32::from_rgb(60, 20, 24), Color32::from_rgb(220, 80, 80), |ui| {
                ui.label(RichText::new(icon::X_CIRCLE).size(12.0).color(Color32::from_rgb(255, 140, 140)));
                ui.label(RichText::new(&err).size(12.0).color(Color32::from_rgb(240, 200, 200)));
                if self.active_thread.is_none() {
                    retry = ui.button(format!("{} Retry", icon::ARROW_COUNTER_CLOCKWISE)).clicked();
                }
                dismiss = ui.small_button("Dismiss").clicked();
            });
            if retry {
                self.open_thread(net, &ctx, None);
            } else if dismiss {
                self.error = None;
            }
        }

        // Sign in, when the harness offers login methods and isn't signed in.
        let needs_login = self.instance().is_some_and(|s| {
            !s.auth.methods.is_empty()
                && (s.auth.status == harness_contracts::AuthStatus::SignedOut
                    || self.error.as_deref().is_some_and(|e| e.to_lowercase().contains("authentication")))
        });
        if (needs_login || self.auth_instructions.is_some())
            && let Some(snapshot) = self.instance().cloned()
        {
            let instance = snapshot.instance_id.clone();
            banner(ui, Color32::from_rgb(22, 30, 46), Color32::from_rgb(90, 140, 220), |ui| match self.auth_instructions.clone() {
                None => {
                    ui.label(RichText::new(format!("{} Sign in:", icon::KEY)).strong().size(12.0));
                    for method in &snapshot.auth.methods {
                        let button = ui.button(&method.name);
                        let button = match &method.description {
                            Some(d) => button.on_hover_text(d),
                            None => button,
                        };
                        if button.clicked() {
                            let mailbox = self.mailbox.clone();
                            net.begin_auth(&instance, &method.id, ctx.clone(), move |r| post(&mailbox, Reply::AuthStarted(r)));
                        }
                    }
                }
                Some(instructions) => {
                    ui.label(RichText::new(instructions).size(12.0));
                    ui.add(egui::TextEdit::singleline(&mut self.auth_paste).hint_text("http://127.0.0.1:…/?code=…").desired_width(220.0));
                    if ui.button("Complete").clicked() && !self.auth_paste.trim().is_empty() {
                        let mailbox = self.mailbox.clone();
                        let pasted = std::mem::take(&mut self.auth_paste);
                        net.complete_auth(&instance, &pasted, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                        self.auth_instructions = None;
                    }
                    if ui.small_button("Done").clicked() {
                        self.auth_instructions = None;
                    }
                }
            });
        }
    }

    fn render_plan(&mut self, ui: &mut egui::Ui) {
        let Some(PlanBody::TodoList { steps }) = self.thread().and_then(|t| t.plan.clone()).map(|p| p.body) else {
            return;
        };
        let done = steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        egui::CollapsingHeader::new(RichText::new(format!("{} Plan ({done}/{})", icon::CLIPBOARD_TEXT, steps.len())).size(11.0))
            .default_open(true)
            .show(ui, |ui| render_steps(ui, &steps));
    }

    fn render_stream(&mut self, ui: &mut egui::Ui, net: &NetClient) {
        let Some(view) = self.thread().cloned() else {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Meta-Harness Trajectory Stream").strong().size(15.0).color(Color32::from_rgb(100, 160, 240)));
                let hint = match self.instance() {
                    Some(s) => format!("{} • type below or Connect to start a session", s.display_name),
                    None => "Waiting for the server…".into(),
                };
                ui.label(RichText::new(hint).size(12.0).color(Color32::GRAY));
            });
            return;
        };
        for turn in &view.turns {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Turn {}", turn.ordinal)).monospace().size(10.0).color(Color32::from_rgb(90, 110, 135)));
                if !turn.status.is_terminal() {
                    ui.label(RichText::new("RUNNING").size(9.0).color(Color32::from_rgb(250, 180, 50)));
                } else if let Some(f) = &turn.failure {
                    ui.label(RichText::new(format!("failed: {}", f.message)).size(9.0).color(Color32::from_rgb(240, 80, 80)));
                }
            });
            for item in view.items_of(&turn.id) {
                self.render_item(ui, net, &view, item);
                ui.add_space(3.0);
            }
            ui.add(egui::Separator::default().spacing(8.0));
        }
    }

    fn render_item(&mut self, ui: &mut egui::Ui, net: &NetClient, view: &ThreadView, item: &TurnItem) {
        match &item.body {
            ItemBody::UserMessage { text } => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(">").monospace().size(14.0).strong().color(Color32::from_rgb(80, 180, 255)));
                    ui.label(RichText::new(text).size(13.0).strong().color(Color32::WHITE));
                });
            }
            ItemBody::AssistantMessage { content, .. } => {
                let text: String = content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                if !text.is_empty() {
                    CommonMarkViewer::new().show(ui, &mut self.markdown_cache, &text);
                }
                for block in content.iter().filter(|b| !matches!(b, ContentBlock::Text { .. })) {
                    ui.label(RichText::new(block_summary(block)).monospace().size(10.0).color(Color32::GRAY));
                }
            }
            ItemBody::Reasoning { text, streaming } => {
                let label = if *streaming { format!("{} Thinking… ({} chars)", icon::BRAIN, text.len()) } else { format!("{} Reasoning ({} chars)", icon::BRAIN, text.len()) };
                egui::CollapsingHeader::new(RichText::new(label).size(11.0).color(Color32::from_rgb(175, 155, 225)))
                    .id_salt(item.id.as_str())
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().size(11.0).color(Color32::from_rgb(150, 155, 175)));
                    });
            }
            ItemBody::ToolCall { kind, name, output, locations, .. } => {
                let (tag, color) = status_tag(item.status);
                let title = item.title.clone().unwrap_or_else(|| name.clone());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{tag} {} {title}", tool_icon(kind))).monospace().size(11.0).color(color));
                    for loc in locations {
                        let label = match loc.line { Some(l) => format!("{} {}:{l}", icon::FOLDER_OPEN, loc.path), None => format!("{} {}", icon::FOLDER_OPEN, loc.path) };
                        if ui.small_button(RichText::new(label).monospace().size(10.0)).clicked() {
                            self.requested_file_jump = Some((loc.path.clone(), loc.line.map(|l| l as usize)));
                        }
                    }
                });
                if !output.is_empty() {
                    egui::CollapsingHeader::new(RichText::new("output").size(10.0).color(Color32::GRAY))
                        .id_salt((item.id.as_str(), "out"))
                        .show(ui, |ui| {
                            for block in output {
                                ui.label(RichText::new(block_detail(block)).monospace().size(10.0).color(Color32::from_rgb(160, 170, 185)));
                            }
                        });
                }
            }
            ItemBody::FileChange { path, .. } => {
                if ui.small_button(RichText::new(format!("{} {path}", icon::PENCIL_SIMPLE)).monospace().size(11.0)).clicked() {
                    self.requested_file_jump = Some((path.clone(), None));
                }
            }
            ItemBody::CommandExecution { command, output, exit_code } => {
                ui.label(RichText::new(format!("$ {command}")).monospace().size(11.0));
                if let Some(out) = output {
                    ui.label(RichText::new(out).monospace().size(10.0).color(Color32::GRAY));
                }
                if let Some(code) = exit_code {
                    ui.label(RichText::new(format!("exit {code}")).monospace().size(10.0).color(Color32::GRAY));
                }
            }
            ItemBody::ProposedPlan { markdown, .. } => {
                Frame::NONE.fill(Color32::from_rgb(18, 24, 32)).inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.label(RichText::new(format!("{} Proposed plan", icon::CLIPBOARD_TEXT)).strong().size(11.0));
                    CommonMarkViewer::new().show(ui, &mut self.markdown_cache, markdown);
                });
            }
            ItemBody::TodoList { steps, .. } => render_steps(ui, steps),
            ItemBody::Request { request_id } => {
                if let Some(request) = view.request(request_id) {
                    render_request(ui, net, &view.thread.id, request);
                }
            }
            ItemBody::Subagent { subagent_id } => {
                ui.label(RichText::new(format!("{} subagent {subagent_id}", icon::ROBOT)).size(11.0));
            }
            ItemBody::Notice { level, text } => {
                let color = match level { NoticeLevel::Info => Color32::GRAY, NoticeLevel::Warning => Color32::from_rgb(250, 180, 50) };
                ui.label(RichText::new(text).size(11.0).color(color));
            }
            ItemBody::Error { message } => {
                ui.label(RichText::new(format!("{} {message}", icon::WARNING)).size(11.0).color(Color32::from_rgb(240, 80, 80)));
            }
            ItemBody::Unknown { native_kind, display } => {
                let shown: String = display.chars().take(300).collect();
                ui.label(RichText::new(format!("[{native_kind}] {shown}")).monospace().size(10.0).color(Color32::GRAY));
            }
            ItemBody::Unsupported => {
                ui.label(RichText::new("(item type not supported by this client)").size(10.0).color(Color32::GRAY));
            }
        }
    }

    fn render_input(
        &mut self,
        ui: &mut egui::Ui,
        net: &NetClient,
        feedback: &FeedbackInspectorState,
        slot_idx: usize,
        is_focused: bool,
        input_mode: &mut crate::keymap::InputMode,
    ) {
        let ctx = ui.ctx().clone();
        // Slash-command suggestions from the harness's live command list.
        if let Some(prefix) = self.input_buffer.strip_prefix('/')
            && !prefix.contains(' ')
            && let Some(snapshot) = self.instance()
        {
            let matches: Vec<_> = snapshot.commands.iter().filter(|c| c.name.starts_with(prefix)).cloned().collect();
            ui.horizontal_wrapped(|ui| {
                for command in matches {
                    let label = RichText::new(format!("/{}", command.name)).monospace().size(10.0);
                    let hint = match &command.input_hint { Some(h) => format!("{} — {h}", command.description), None => command.description.clone() };
                    if ui.small_button(label).on_hover_text(hint).clicked() {
                        self.input_buffer = format!("/{} ", command.name);
                    }
                }
            });
        }

        ui.horizontal(|ui| {
            let send_w = 64.0;
            let text_w = (ui.available_width() - send_w - ui.spacing().item_spacing.x - 4.0).max(60.0);
            let inserting = *input_mode == crate::keymap::InputMode::Insert { slot: slot_idx };
            let hint = if inserting {
                "Instructions… (Enter to send, Shift+Enter newline, Esc normal mode, / for commands)"
            } else {
                "Press 'i' or click to enter insert mode..."
            };
            let output = ui.add(
                egui::TextEdit::multiline(&mut self.input_buffer)
                    .id(egui::Id::new("chat_persistent_input_text_edit"))
                    .desired_rows(2)
                    .desired_width(text_w)
                    .hint_text(hint),
            );
            if inserting && is_focused && !feedback.is_modal_open && !feedback.is_issues_window_open && !feedback.is_inspecting {
                output.request_focus();
            }
            if output.clicked() || output.gained_focus() {
                *input_mode = crate::keymap::InputMode::Insert { slot: slot_idx };
            }
            let send = ui.add_sized([send_w, 36.0], egui::Button::new(RichText::new("Send").strong().size(11.0))).clicked();
            let enter = inserting && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if (send || enter) && !self.input_buffer.trim().is_empty() {
                let text = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                self.send_prompt(net, &ctx, text);
            }
        });
    }
}

/// Leader key for the n-th dynamic entry in a group (1-9, then a-z).
fn nth_key(n: usize) -> char {
    const KEYS: &[u8] = b"123456789abcdefghijklmnopqrstuvwxz";
    KEYS.get(n).map(|b| *b as char).unwrap_or('0')
}

fn mcp_label(server: &McpServer) -> String {
    match &server.transport {
        McpTransport::Http { url, .. } => format!("{} — {url}", server.name),
        McpTransport::Stdio { command, .. } => format!("{} — {command}", server.name),
    }
}

impl ChatStreamState {
    fn load_saved(&mut self, net: &NetClient, ctx: &egui::Context, then: &'static str) {
        let Some(instance) = self.active_instance.clone() else { return };
        self.after_saved = Some(then);
        let mailbox = self.mailbox.clone();
        net.list_saved_sessions(&instance, ".", ctx.clone(), move |r| post(&mailbox, Reply::Saved(r.map(|p| p.sessions))));
    }

    /// Restarts the active session with the current MCP list (ACP only takes MCP
    /// servers when a session is created or loaded).
    fn reconnect(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(view) = self.thread() else { return };
        let native = view.thread.native_id.clone();
        let thread = view.thread.id.clone();
        net.close_thread(&thread, ctx.clone());
        self.active_thread = None;
        self.open_thread(net, ctx, native);
    }

    fn saved_choices(&self) -> Option<Vec<Choice>> {
        self.saved_sessions.as_ref().map(|list| {
            list.iter()
                .map(|s| Choice::new(s.native_id.to_string(), s.title.clone().unwrap_or_else(|| s.native_id.to_string())).detail(s.updated_at.clone()))
                .collect()
        })
    }

    /// Every agent action available right now, from the instance snapshot, the
    /// active session's controls and what is pending.
    pub fn actions(&self) -> Vec<Action> {
        let mut out = Vec::new();
        let snapshot = self.instance();
        let caps = snapshot.map(|s| s.capabilities.clone()).unwrap_or_default();
        let view = self.thread();
        let chat = |id: &str, label: &str| Action::new(Owner::Chat, id, label);

        // Session
        out.push(chat("session.new", "New session").keys("sn"));
        if caps.sessions.list_saved {
            let param = self.saved_choices().map(Param::Choice).unwrap_or(Param::None);
            out.push(chat("session.resume", "Resume a saved session").keys("sr").param(param));
            if caps.sessions.delete_saved {
                let param = self.saved_choices().map(Param::Choice).unwrap_or(Param::None);
                out.push(chat("session.delete", "Delete a saved session").keys("sd").param(param));
            }
        }
        if self.store.threads.len() > 1 {
            let choices = self
                .store
                .threads
                .values()
                .map(|t| Choice::new(t.thread.id.to_string(), t.thread.title.clone().unwrap_or_else(|| t.thread.id.to_string()))
                    .current(Some(&t.thread.id) == self.active_thread.as_ref()))
                .collect();
            out.push(chat("session.switch", "Switch session").keys("st").param(Param::Choice(choices)));
        }
        if self.store.instances.len() > 1 && self.active_thread.is_none() {
            let choices = self
                .store
                .instances
                .values()
                .map(|i| Choice::new(i.instance_id.to_string(), &i.display_name).current(Some(&i.instance_id) == self.active_instance.as_ref()))
                .collect();
            out.push(chat("instance.select", "Choose harness").keys("si").param(Param::Choice(choices)));
        }
        if let Some(view) = view {
            if view.running_turn().is_some() && caps.turns.interrupt {
                out.push(chat("session.stop", "Stop the running turn").keys("ss"));
            }
            out.push(chat("session.close", "Close session").keys("sc"));
        }
        out.push(chat("instance.refresh", "Re-probe the harness"));

        // Model & config
        match view {
            Some(view) => {
                let mut n = 0;
                for control in &view.thread.controls.config {
                    let choices = match &control.kind {
                        ConfigKind::Select { choices, current } => choices
                            .iter()
                            .map(|c| Choice::new(&c.value, &c.name).detail(c.group.clone().or(c.description.clone())).current(&c.value == current))
                            .collect(),
                        ConfigKind::Boolean { current } => vec![Choice::new("true", "On").current(*current), Choice::new("false", "Off").current(!*current)],
                    };
                    let keys = match control.category {
                        harness_contracts::ConfigCategory::Model => "mm".to_owned(),
                        harness_contracts::ConfigCategory::Mode => "oo".to_owned(),
                        _ => {
                            let k = nth_key(n);
                            n += 1;
                            format!("m{k}")
                        }
                    };
                    let mut action = chat(&format!("config.{}", control.id), &format!("Set {}", control.name)).keys(&keys).param(Param::Choice(choices));
                    if let Some(d) = &control.description {
                        action = action.detail(d.clone());
                    }
                    out.push(action);
                }
            }
            None => {
                if let Some(s) = snapshot.filter(|s| !s.models.is_empty()) {
                    let choices = s.models.iter().map(|m| Choice::new(&m.id, &m.name).current(self.model.as_ref() == Some(&m.id))).collect();
                    out.push(chat("model.next", "Model for the next session").keys("mm").param(Param::Choice(choices)));
                }
            }
        }

        // Mode: in-session from the harness (unless a mode config option covers it),
        // otherwise the runtime mode for the next session.
        match view {
            Some(view) => {
                let controls = &view.thread.controls;
                let has_mode_option = controls.config.iter().any(|c| c.category == harness_contracts::ConfigCategory::Mode);
                if !controls.modes.is_empty() && !has_mode_option {
                    let current = controls.current_mode.clone();
                    let choices = controls
                        .modes
                        .iter()
                        .map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone()).current(current.as_ref() == Some(&m.id)))
                        .collect();
                    out.push(chat("mode.native", "Set session mode").keys("oo").param(Param::Choice(choices)));
                }
            }
            None => {
                if let Some(s) = snapshot.filter(|s| !s.runtime_modes.is_empty()) {
                    let choices = s
                        .runtime_modes
                        .iter()
                        .map(|m| Choice::new(serde_json::to_value(m).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default(), mode_label(*m)).current(*m == self.runtime_mode))
                        .collect();
                    out.push(chat("mode.runtime", "Runtime mode").keys("oo").param(Param::Choice(choices)));
                }
            }
        }

        // MCP servers
        let active_servers: Vec<McpServer> = view.map(|v| v.thread.mcp_servers.clone()).unwrap_or_else(|| self.extra_mcp.clone());
        if !active_servers.is_empty() {
            let choices = active_servers.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
            out.push(chat("mcp.list", "MCP servers in this session").keys("pl").param(Param::Choice(choices)));
        }
        if caps.tools.mcp_http {
            out.push(chat("mcp.add", "Add an MCP server (HTTP)").keys("pa").param(Param::Text { hint: "name https://host/mcp".into() }));
        }
        if !self.extra_mcp.is_empty() {
            let choices = self.extra_mcp.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
            out.push(chat("mcp.remove", "Remove an added MCP server").keys("pr").param(Param::Choice(choices)));
        }

        // Harness commands
        if let Some(s) = snapshot {
            for (n, command) in s.commands.iter().enumerate() {
                let param = match &command.input_hint {
                    Some(hint) => Param::Text { hint: hint.clone() },
                    None => Param::None,
                };
                out.push(chat(&format!("cmd.{}", command.name), &format!("/{}", command.name)).keys(&format!("h{}", nth_key(n))).param(param).detail(command.description.clone()));
            }
        }

        // Account
        if let Some(s) = snapshot.filter(|s| !s.auth.methods.is_empty()) {
            let choices = s.auth.methods.iter().map(|m| Choice::new(&m.id, &m.name).detail(m.description.clone())).collect();
            out.push(chat("auth.login", "Sign in").keys("al").param(Param::Choice(choices)));
            out.push(chat("auth.complete", "Complete sign-in with a callback URL").keys("ac").param(Param::Text { hint: "http://127.0.0.1:…/?code=…".into() }));
            out.push(chat("auth.logout", "Sign out").keys("ao"));
        }

        // Pending requests (contextual)
        if let Some(view) = view
            && let Some(request) = view.requests.iter().find(|r| r.status == RequestStatus::Pending)
        {
            for (n, option) in request.options.iter().enumerate() {
                let key = match option.decision {
                    ApprovalDecision::Accept => 'y',
                    ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways => 'a',
                    ApprovalDecision::Decline => 'n',
                    ApprovalDecision::Cancel => 'c',
                };
                let key = if request.options.iter().take(n).any(|o| o.decision == option.decision) { nth_key(n) } else { key };
                let mut action = chat(&format!("request.{}.{}", request.id, option.id), &format!("{}: {}", option.label, request.title)).keys(&format!("r{key}"));
                if let Some(w) = &option.warning {
                    action = action.detail(w.clone());
                }
                out.push(action);
            }
        }

        // Prompt
        out.push(chat("prompt.clear", "Clear the prompt"));
        out
    }

    pub fn invoke(&mut self, inv: &Invocation, net: &NetClient, ctx: &egui::Context) {
        let value = inv.value.clone();
        let thread = self.active_thread.clone();
        let id = inv.id.as_str();
        match id {
            "session.new" => {
                self.active_thread = None;
                self.open_thread(net, ctx, None);
            }
            "session.resume" => match value {
                Some(native) => {
                    self.active_thread = None;
                    self.open_thread(net, ctx, Some(NativeId::new(native)));
                }
                None => self.load_saved(net, ctx, "session.resume"),
            },
            "session.delete" => match (value, self.active_instance.clone()) {
                (Some(native), Some(instance)) => {
                    let mailbox = self.mailbox.clone();
                    net.delete_saved_session(&instance, &NativeId::new(native), ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                    self.saved_sessions = None;
                }
                (None, _) => self.load_saved(net, ctx, "session.delete"),
                _ => {}
            },
            "session.switch" => {
                if let Some(t) = value {
                    self.active_thread = Some(ThreadId::new(t));
                }
            }
            "session.stop" => {
                if let Some(t) = &thread {
                    net.interrupt(t, ctx.clone());
                }
            }
            "session.close" => {
                if let Some(t) = &thread {
                    net.close_thread(t, ctx.clone());
                    self.active_thread = None;
                }
            }
            "instance.select" => {
                if let Some(i) = value {
                    self.active_instance = Some(InstanceId::new(i));
                }
            }
            "instance.refresh" => {
                if let Some(instance) = &self.active_instance {
                    let mailbox = self.mailbox.clone();
                    net.refresh_instance(instance, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
            "model.next" => self.model = value,
            "mode.native" => {
                if let Some(mode) = value {
                    self.steer(net, ctx, SteerInput { mode: Some(mode), ..Default::default() });
                }
            }
            "mode.runtime" => {
                if let Some(mode) = value.and_then(|v| serde_json::from_value::<RuntimeMode>(serde_json::Value::String(v)).ok()) {
                    self.runtime_mode = mode;
                }
            }
            "mcp.add" => {
                let text = value.unwrap_or_default();
                let mut parts = text.split_whitespace();
                if let (Some(name), Some(url)) = (parts.next(), parts.next()) {
                    self.extra_mcp.retain(|m| m.name != name);
                    self.extra_mcp.push(McpServer { name: name.into(), transport: McpTransport::Http { url: url.into(), headers: Vec::new() } });
                    self.reconnect(net, ctx);
                } else {
                    self.error = Some("MCP server needs a name and a URL, e.g. `docs https://host/mcp`".into());
                }
            }
            "mcp.remove" => {
                if let Some(name) = value {
                    self.extra_mcp.retain(|m| m.name != name);
                    self.reconnect(net, ctx);
                }
            }
            "mcp.list" => {}
            "auth.login" => {
                if let (Some(method), Some(instance)) = (value, self.active_instance.clone()) {
                    let mailbox = self.mailbox.clone();
                    net.begin_auth(&instance, &method, ctx.clone(), move |r| post(&mailbox, Reply::AuthStarted(r)));
                }
            }
            "auth.complete" => {
                if let (Some(url), Some(instance)) = (value, self.active_instance.clone()) {
                    let mailbox = self.mailbox.clone();
                    net.complete_auth(&instance, &url, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                    self.auth_instructions = None;
                }
            }
            "auth.logout" => {
                if let Some(instance) = &self.active_instance {
                    let mailbox = self.mailbox.clone();
                    net.logout(instance, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
            "prompt.clear" => self.input_buffer.clear(),
            _ => {
                if let Some(config) = id.strip_prefix("config.") {
                    if let Some(v) = value {
                        let is_bool = self
                            .thread()
                            .and_then(|t| t.thread.controls.config.iter().find(|c| c.id == config))
                            .is_some_and(|c| matches!(c.kind, ConfigKind::Boolean { .. }));
                        let json = if is_bool { serde_json::Value::Bool(v == "true") } else { serde_json::Value::String(v) };
                        self.steer(net, ctx, SteerInput { config: vec![OptionValue { id: config.to_owned(), value: json }], ..Default::default() });
                    }
                } else if let Some(command) = id.strip_prefix("cmd.") {
                    let text = match value {
                        Some(args) if !args.is_empty() => format!("/{command} {args}"),
                        _ => format!("/{command}"),
                    };
                    self.send_prompt(net, ctx, text);
                } else if let Some(rest) = id.strip_prefix("request.")
                    && let (Some(thread), Some((request, option))) = (thread, rest.split_once('.'))
                {
                    let response = RequestResponse { option_id: Some(option.to_owned()), ..Default::default() };
                    let mailbox = self.mailbox.clone();
                    net.respond(&thread, &harness_contracts::RequestId::new(request), &response, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
        }
    }
}

fn render_steps(ui: &mut egui::Ui, steps: &[PlanStep]) {
    for step in steps {
        let (mark, color) = match step.status {
            StepStatus::Completed => (icon::CHECK_SQUARE, Color32::from_rgb(80, 200, 120)),
            StepStatus::InProgress => (icon::CIRCLE_HALF, Color32::from_rgb(250, 180, 50)),
            StepStatus::Pending => (icon::SQUARE, Color32::GRAY),
        };
        ui.label(RichText::new(format!("{mark} {}", step.text)).size(11.0).color(color));
    }
}

fn render_request(ui: &mut egui::Ui, net: &NetClient, thread: &ThreadId, request: &harness_contracts::RuntimeRequest) {
    Frame::NONE
        .fill(Color32::from_rgb(36, 30, 18))
        .stroke(Stroke::new(1.0, Color32::from_rgb(220, 150, 40)))
        .corner_radius(4)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("{} PERMISSION REQUIRED", icon::WARNING)).strong().size(11.0).color(Color32::from_rgb(250, 180, 50)));
            ui.label(RichText::new(&request.title).size(12.0));
            if let Some(detail) = &request.detail {
                ui.label(RichText::new(detail).size(11.0).color(Color32::GRAY));
            }
            match request.status {
                RequestStatus::Pending => {
                    ui.horizontal_wrapped(|ui| {
                        for option in &request.options {
                            let color = match option.decision {
                                ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways => Color32::from_rgb(80, 200, 120),
                                ApprovalDecision::Decline | ApprovalDecision::Cancel => Color32::from_rgb(240, 100, 100),
                            };
                            let button = ui.button(RichText::new(&option.label).color(color));
                            let button = match &option.warning { Some(w) => button.on_hover_text(w), None => button };
                            if button.clicked() {
                                let response = RequestResponse { option_id: Some(option.id.clone()), ..Default::default() };
                                net.respond(thread, &request.id, &response, ui.ctx().clone(), |_| {});
                            }
                        }
                    });
                }
                status => {
                    let chosen = request
                        .response
                        .as_ref()
                        .and_then(|r| r.option_id.as_ref())
                        .and_then(|id| request.options.iter().find(|o| &o.id == id))
                        .map(|o| o.label.clone());
                    let text = match (status, chosen) {
                        (RequestStatus::Resolved, Some(label)) => format!("{} {label}", icon::CHECK),
                        (RequestStatus::Resolved, None) => format!("{} Resolved", icon::CHECK),
                        (other, _) => format!("{other:?}"),
                    };
                    ui.label(RichText::new(text).size(11.0).color(Color32::from_rgb(140, 200, 140)));
                }
            }
        });
}

fn block_summary(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::Image { mime_type, .. } => format!("[image {mime_type}]"),
        ContentBlock::ResourceLink { uri, name } => format!("[link {}]", name.clone().unwrap_or_else(|| uri.clone())),
        ContentBlock::Resource { uri, .. } => format!("[resource {uri}]"),
        ContentBlock::Diff(d) => format!("[diff {}]", d.path),
        ContentBlock::Unsupported => "[unsupported content]".into(),
    }
}

fn block_detail(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Diff(d) => {
            let old = d.old_text.as_deref().map(|t| t.lines().count()).unwrap_or(0);
            let new = d.new_text.lines().count();
            let preview: String = d.new_text.lines().take(12).collect::<Vec<_>>().join("\n");
            format!("{}  (−{old} +{new} lines)\n{preview}", d.path)
        }
        ContentBlock::Resource { uri, text: Some(text) } => format!("{uri}\n{}", text.chars().take(600).collect::<String>()),
        other => block_summary(other),
    }
}
