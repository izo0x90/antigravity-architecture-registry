//! The chat/trajectory view. Renders only from the store (server events) and the
//! active instance's snapshot: pickers, commands and controls come from what the
//! harness reports and its capabilities, never from hardcoded harness knowledge.

use std::sync::{Arc, Mutex};

use egui::{Color32, Frame, Margin, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ApprovalDecision, ClientEvent, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestResponse, RequestStatus, RuntimeMode, RuntimePolicy, SavedSession,
    StepStatus, SteerInput, ThreadId, ThreadStatus, ToolKind, TurnInput, TurnItem,
};

use egui_phosphor::regular as icon;

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
        let body = OpenThreadBody { instance_id, policy: self.policy(), model: self.model_selection(), resume };
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
                Reply::Saved(Ok(sessions)) => self.saved_sessions = Some(sessions),
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
                        if caps.sessions.list_saved
                            && ui.button(format!("{} Resume…", icon::ARROW_COUNTER_CLOCKWISE)).clicked()
                            && let Some(instance) = &self.active_instance
                        {
                            let mailbox = self.mailbox.clone();
                            net.list_saved_sessions(instance, ".", ctx.clone(), move |r| {
                                post(&mailbox, Reply::Saved(r.map(|p| p.sessions)))
                            });
                        }
                    }
                }
            });
        });

        ui.horizontal(|ui| {
            if let Some(snapshot) = &snapshot {
                if !snapshot.models.is_empty() {
                    ui.label(RichText::new("Model:").strong().size(11.0));
                    let previous = self.model.clone();
                    let shown = self.model.clone().unwrap_or_else(|| "default".into());
                    egui::ComboBox::from_id_salt("chat_model").width(120.0).selected_text(shown).show_ui(ui, |ui| {
                        for m in &snapshot.models {
                            ui.selectable_value(&mut self.model, Some(m.id.clone()), &m.name);
                        }
                    });
                    if self.model != previous && caps.sessions.model_switch_in_session {
                        let model = self.model_selection();
                        self.steer(net, &ctx, SteerInput { model, ..Default::default() });
                    }
                }
                if !snapshot.runtime_modes.is_empty() {
                    ui.label(RichText::new("Mode:").strong().size(11.0));
                    let previous = self.runtime_mode;
                    egui::ComboBox::from_id_salt("chat_mode").width(110.0).selected_text(mode_label(self.runtime_mode)).show_ui(ui, |ui| {
                        for mode in &snapshot.runtime_modes {
                            ui.selectable_value(&mut self.runtime_mode, *mode, mode_label(*mode));
                        }
                    });
                    if self.runtime_mode != previous && caps.sessions.runtime_mode_switch_in_session {
                        let policy = Some(self.policy());
                        self.steer(net, &ctx, SteerInput { policy, ..Default::default() });
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

        if let Some(sessions) = self.saved_sessions.clone() {
            let mut chosen = None;
            let mut close = false;
            banner(ui, Color32::from_rgb(20, 26, 34), Color32::from_rgb(70, 90, 120), |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Saved sessions").strong().size(12.0));
                        close = ui.small_button(icon::X).clicked();
                    });
                    if sessions.is_empty() {
                        ui.label(RichText::new("None yet.").size(11.0).color(Color32::GRAY));
                    }
                    for s in &sessions {
                        let title = s.title.clone().unwrap_or_else(|| s.native_id.to_string());
                        let when = s.updated_at.clone().unwrap_or_default();
                        if ui.button(format!("{title}  {when}")).clicked() {
                            chosen = Some(s.native_id.clone());
                        }
                    }
                });
            });
            if let Some(native) = chosen {
                self.open_thread(net, &ctx, Some(native));
            } else if close {
                self.saved_sessions = None;
            }
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
                        (RequestStatus::Resolved, Some(label)) => format!("✓ {label}"),
                        (RequestStatus::Resolved, None) => "✓ Resolved".into(),
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
