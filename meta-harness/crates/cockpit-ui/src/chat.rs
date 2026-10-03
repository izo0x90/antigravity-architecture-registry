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

use crate::actions::{Action, ActionProvider, Choice, Param, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::net::{NetClient, NetResult, OpenThreadBody};
use crate::store::{Store, ThreadView};
use crate::theme::colors;

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
    after_saved: Option<String>,
    /// Asks the app to open the palette at this action (set by toolbar buttons
    /// and by actions that first need to load their choices).
    pending_palette: Option<String>,
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
        ItemStatus::Pending => ("[..]", colors().muted),
        ItemStatus::InProgress => ("[RUN]", colors().warn),
        ItemStatus::Completed => ("[OK]", colors().ok),
        ItemStatus::Failed => ("[FAIL]", colors().error),
        ItemStatus::Cancelled => ("[--]", colors().muted),
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
                        self.pending_palette = Some(action);
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

    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, is_focused: bool, cx: &mut RunCx<'_>) {
        let (net, slot_idx) = (cx.net, cx.slot);
        let ctx = ui.ctx().clone();
        self.drain_replies(net, &ctx);

        ui.vertical(|ui| {
            let start = ui.cursor().min;
            let w = ui.available_width();
            self.render_controls(ui, cx);
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
            self.render_input(ui, net, feedback, slot_idx, is_focused, cx.input_mode);
            let input = Rect::from_min_size(Pos2::new(start.x, start.y), Vec2::new(w, (ui.cursor().min.y - start.y).max(40.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:input_bar", slot_idx + 1), input);
        });
    }

    /// The toolbar: the view's own actions marked for it, plus status chrome.
    fn render_controls(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let actions = self.actions();
        let on_toolbar = |style: ToolbarStyle| actions.iter().filter(move |a| a.toolbar.as_ref().is_some_and(|t| t.style == style));
        let mut used: Option<(Action, ToolbarUse)> = None;

        ui.horizontal(|ui| {
            let name = self.instance().map(|s| s.display_name.clone()).unwrap_or_else(|| "No harness".into());
            ui.label(RichText::new(name).strong().size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for action in on_toolbar(ToolbarStyle::Button) {
                    if let Some(u) = toolbar_control(ui, action) {
                        used = Some((action.clone(), u));
                    }
                }
                if let Some(thread) = &self.active_thread {
                    let short: String = thread.as_str().chars().take(8).collect();
                    ui.label(RichText::new(format!("[{short}]")).monospace().size(10.0).color(colors().info));
                } else if self.opening {
                    ui.label(RichText::new(format!("{} Connecting…", icon::HOURGLASS)).size(10.0).color(colors().warn));
                }
            });
        });

        ui.horizontal_wrapped(|ui| {
            for action in on_toolbar(ToolbarStyle::Control) {
                if let Some(u) = toolbar_control(ui, action) {
                    used = Some((action.clone(), u));
                }
            }
            if let Some(thread) = self.thread().map(|v| &v.thread)
                && !thread.mcp_servers.is_empty()
            {
                let names: Vec<String> = thread.mcp_servers.iter().map(mcp_label).collect();
                ui.add(egui::Label::new(RichText::new(format!("{} MCP {}", icon::PLUGS_CONNECTED, thread.mcp_servers.len())).size(10.0).color(colors().muted)).extend())
                    .on_hover_text(names.join("\n"));
            }
            if let Some(usage) = self.thread().and_then(|t| t.usage.clone())
                && let (Some(used), Some(window)) = (usage.context_used_tokens, usage.context_window_tokens)
            {
                ui.add(egui::Label::new(RichText::new(format!("ctx {:.1}k/{:.0}k", used as f64 / 1000.0, window as f64 / 1000.0)).monospace().size(10.0).color(colors().muted)).extend());
            }
        });

        match used {
            Some((action, ToolbarUse::Run(value))) => self.run(&action, value, cx),
            Some((action, ToolbarUse::OpenPalette)) => self.pending_palette = Some(action.id),
            None => {}
        }
    }

    fn render_banners(&mut self, ui: &mut egui::Ui, net: &NetClient) {
        let ctx = ui.ctx().clone();
        if let Some(err) = self.error.clone() {
            let mut retry = false;
            let mut dismiss = false;
            banner(ui, colors().error_bg, colors().error, |ui| {
                ui.label(RichText::new(icon::X_CIRCLE).size(12.0).color(colors().error));
                ui.label(RichText::new(&err).size(12.0).color(colors().text));
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
            banner(ui, colors().info_bg, colors().focus, |ui| match self.auth_instructions.clone() {
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
                ui.label(RichText::new("No session yet").strong().size(15.0).color(colors().accent));
                let hint = match self.instance() {
                    Some(s) => format!("{} • type below or Connect to start a session", s.display_name),
                    None => "Waiting for the server…".into(),
                };
                ui.label(RichText::new(hint).size(12.0).color(colors().muted));
            });
            return;
        };
        for turn in &view.turns {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Turn {}", turn.ordinal)).monospace().size(10.0).color(colors().faint));
                if !turn.status.is_terminal() {
                    ui.label(RichText::new("RUNNING").size(9.0).color(colors().warn));
                } else if let Some(f) = &turn.failure {
                    ui.label(RichText::new(format!("failed: {}", f.message)).size(9.0).color(colors().error));
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
                    ui.label(RichText::new(">").monospace().size(14.0).strong().color(colors().accent));
                    ui.label(RichText::new(text).size(13.0).strong().color(colors().text_strong));
                });
            }
            ItemBody::AssistantMessage { content, .. } => {
                let text: String = content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                if !text.is_empty() {
                    CommonMarkViewer::new().show(ui, &mut self.markdown_cache, &text);
                }
                for block in content.iter().filter(|b| !matches!(b, ContentBlock::Text { .. })) {
                    ui.label(RichText::new(block_summary(block)).monospace().size(10.0).color(colors().muted));
                }
            }
            ItemBody::Reasoning { text, streaming } => {
                let label = if *streaming { format!("{} Thinking… ({} chars)", icon::BRAIN, text.len()) } else { format!("{} Reasoning ({} chars)", icon::BRAIN, text.len()) };
                egui::CollapsingHeader::new(RichText::new(label).size(11.0).color(colors().special))
                    .id_salt(item.id.as_str())
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().size(11.0).color(colors().muted));
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
                    egui::CollapsingHeader::new(RichText::new("output").size(10.0).color(colors().muted))
                        .id_salt((item.id.as_str(), "out"))
                        .show(ui, |ui| {
                            for block in output {
                                ui.label(RichText::new(block_detail(block)).monospace().size(10.0).color(colors().muted));
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
                    ui.label(RichText::new(out).monospace().size(10.0).color(colors().muted));
                }
                if let Some(code) = exit_code {
                    ui.label(RichText::new(format!("exit {code}")).monospace().size(10.0).color(colors().muted));
                }
            }
            ItemBody::ProposedPlan { markdown, .. } => {
                Frame::NONE.fill(colors().surface).inner_margin(Margin::same(6)).show(ui, |ui| {
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
                let color = match level { NoticeLevel::Info => colors().muted, NoticeLevel::Warning => colors().warn };
                ui.label(RichText::new(text).size(11.0).color(color));
            }
            ItemBody::Error { message } => {
                ui.label(RichText::new(format!("{} {message}", icon::WARNING)).size(11.0).color(colors().error));
            }
            ItemBody::Unknown { native_kind, display } => {
                let shown: String = display.chars().take(300).collect();
                ui.label(RichText::new(format!("[{native_kind}] {shown}")).monospace().size(10.0).color(colors().muted));
            }
            ItemBody::Unsupported => {
                ui.label(RichText::new("(item type not supported by this client)").size(10.0).color(colors().muted));
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

fn mcp_label(server: &McpServer) -> String {
    match &server.transport {
        McpTransport::Http { url, .. } => format!("{} — {url}", server.name),
        McpTransport::Stdio { command, .. } => format!("{} — {command}", server.name),
    }
}

impl ChatStreamState {
    fn load_saved(&mut self, net: &NetClient, ctx: &egui::Context, then: &str) {
        let Some(instance) = self.active_instance.clone() else { return };
        self.after_saved = Some(then.to_owned());
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
    HarnessCommand { name: String },
    Login,
    CompleteLogin,
    Logout,
    Answer { thread: ThreadId, request: harness_contracts::RequestId, option: String },
}

/// Snake/kebab name of a contract enum as serialized, for keymap names.
fn wire_name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
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
        out.push(Action::new("chat.insert", "Type a prompt", C::Insert).focused());
        out.push(Action::new("chat.clear", "Clear the prompt", C::ClearPrompt).focused());

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

        // MCP servers
        let active_servers: Vec<McpServer> = view.map(|v| v.thread.mcp_servers.clone()).unwrap_or_else(|| self.extra_mcp.clone());
        if !active_servers.is_empty() {
            let choices = active_servers.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
            out.push(Action::new("mcp.list", "MCP servers in this session", C::ListMcp).param(Param::Choice(choices)));
        }
        if caps.tools.mcp_http {
            out.push(Action::new("mcp.add", "Add an MCP server (HTTP)", C::AddMcp).param(Param::Text { hint: "name https://host/mcp".into() }));
        }
        if !self.extra_mcp.is_empty() {
            let choices = self.extra_mcp.iter().map(|m| Choice::new(&m.name, mcp_label(m))).collect();
            out.push(Action::new("mcp.remove", "Remove an added MCP server", C::RemoveMcp).param(Param::Choice(choices)));
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
            out.push(Action::new("auth.complete", "Complete sign-in with a callback URL", C::CompleteLogin).param(Param::Text { hint: "http://127.0.0.1:…/?code=…".into() }));
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
            ChatCommand::ClearPrompt => self.input_buffer.clear(),
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
            ChatCommand::ListMcp => {}
            ChatCommand::AddMcp => {
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
            ChatCommand::RemoveMcp => {
                if let Some(name) = value {
                    self.extra_mcp.retain(|m| m.name != name);
                    self.reconnect(net, ctx);
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
                    self.auth_instructions = None;
                }
            }
            ChatCommand::Logout => {
                if let Some(instance) = &self.active_instance {
                    let mailbox = self.mailbox.clone();
                    net.logout(instance, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                }
            }
            ChatCommand::Answer { thread, request, option } => {
                let response = RequestResponse { option_id: Some(option.clone()), ..Default::default() };
                let mailbox = self.mailbox.clone();
                net.respond(thread, request, &response, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
            }
        }
    }

    fn take_palette_request(&mut self) -> Option<String> {
        self.pending_palette.take()
    }
}

fn render_steps(ui: &mut egui::Ui, steps: &[PlanStep]) {
    for step in steps {
        let (mark, color) = match step.status {
            StepStatus::Completed => (icon::CHECK_SQUARE, colors().ok),
            StepStatus::InProgress => (icon::CIRCLE_HALF, colors().warn),
            StepStatus::Pending => (icon::SQUARE, colors().muted),
        };
        ui.label(RichText::new(format!("{mark} {}", step.text)).size(11.0).color(color));
    }
}

fn render_request(ui: &mut egui::Ui, net: &NetClient, thread: &ThreadId, request: &harness_contracts::RuntimeRequest) {
    Frame::NONE
        .fill(colors().warn_bg)
        .stroke(Stroke::new(1.0, colors().warn))
        .corner_radius(4)
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("{} PERMISSION REQUIRED", icon::WARNING)).strong().size(11.0).color(colors().warn));
            ui.label(RichText::new(&request.title).size(12.0));
            if let Some(detail) = &request.detail {
                ui.label(RichText::new(detail).size(11.0).color(colors().muted));
            }
            match request.status {
                RequestStatus::Pending => {
                    ui.horizontal_wrapped(|ui| {
                        for option in &request.options {
                            let color = match option.decision {
                                ApprovalDecision::Accept | ApprovalDecision::AcceptForSession | ApprovalDecision::AcceptAlways => colors().ok,
                                ApprovalDecision::Decline | ApprovalDecision::Cancel => colors().error,
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
                    ui.label(RichText::new(text).size(11.0).color(colors().ok));
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
