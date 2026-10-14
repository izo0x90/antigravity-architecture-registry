//! The chat/trajectory view. Renders only from the store (server events) and the
//! active instance's snapshot: pickers, commands and controls come from what the
//! harness reports and its capabilities, never from hardcoded harness knowledge.

use std::sync::{Arc, Mutex};

use egui::{Color32, Frame, Pos2, Rect, RichText, Stroke, Vec2};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use harness_contracts::{
    ClientEvent, ConfigKind, McpServer, McpTransport, NativeId, OptionValue, ContentBlock, InstanceId, ItemBody, ItemStatus, ModelSelection, NoticeLevel, PlanBody,
    PlanStep, ProviderSnapshot, ProviderStatus, RequestStatus, RuntimeMode, RuntimePolicy, SavedSession,
    StepStatus, SteerInput, ThreadId, ThreadStatus, ToolKind, ToolServers, TurnInput, TurnItem, TurnStatus,
};

use egui_phosphor::regular as icon;

use crate::actions::{Action, ActionProvider, Choice, MINOR, Param, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::nav::{Fold, GoTo, Item, Motion, Navigable, Place, Spot, Step, Target};
use crate::net::{AuthChallenge, NetClient, NetResult, OpenThreadBody};
use crate::request_card::{Drafts, render_request};
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
    AuthStarted(NetResult<AuthChallenge>),
    /// The tool servers every session gets; `saved`: after a change.
    Tools { result: NetResult<ToolServers>, saved: bool },
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
    /// The tool servers every session gets (ours and the project's), once
    /// asked for.
    tools: Option<ToolServers>,
    tools_asked: bool,
    /// A sign-in under way: the page to open and what to paste back.
    login: Option<AuthChallenge>,
    auth_paste: String,
    /// Answers being given to the agent's questions.
    drafts: Drafts,
    mailbox: Mailbox,
    /// The message the cursor is on (normal mode); None follows the latest.
    pub cursor: Option<usize>,
    /// Which of its links a hint picked.
    link_pick: Option<usize>,
    /// Scroll the cursor's message into view next frame.
    scroll: bool,
    /// Messages and links drawn last frame (and this one's, as drawn), and
    /// the stream's viewport.
    spots: Vec<Spot>,
    drawing: Vec<Spot>,
    view: Rect,
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
            tools: None,
            tools_asked: false,
            login: None,
            auth_paste: String::new(),
            drafts: Drafts::default(),
            mailbox: Mailbox::default(),
            cursor: None,
            link_pick: None,
            scroll: false,
            spots: Vec::new(),
            drawing: Vec::new(),
            view: Rect::NOTHING,
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
        .stroke(Stroke::new(crate::theme::look().line_thin, stroke))
        .corner_radius(crate::theme::look().radius_m)
        .inner_margin(crate::theme::margin2(crate::theme::look().space_m, crate::theme::look().space_sm))
        .show(ui, |ui| ui.horizontal_wrapped(add));
    ui.add_space(crate::theme::look().space_s);
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

    /// Why the chosen harness (else the only one) can't start, and how to
    /// fix it, as its instance says.
    pub fn harness_problem(&self) -> Option<String> {
        let instance = match &self.active_instance {
            Some(id) => self.store.instances.get(id),
            None if self.store.instances.len() == 1 => self.store.instances.values().next(),
            None => None,
        }?;
        match &instance.status {
            ProviderStatus::Unavailable { reason } => Some(format!("{}: {reason}", instance.display_name)),
            ProviderStatus::Error { message } => Some(format!("{}: {message}", instance.display_name)),
            _ => None,
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
            mcp_servers: Vec::new(),
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

    /// Adds `text` to what the agent is doing, when it takes that and a
    /// turn runs; else sends it (queued behind the running turn).
    fn add_to_turn(&mut self, net: &NetClient, ctx: &egui::Context, text: String) {
        let steerable = self.instance().is_some_and(|s| s.capabilities.turns.active_steering);
        match (&self.active_thread, self.thread().and_then(|t| t.running_turn()).is_some() && steerable) {
            (Some(thread), true) => {
                let mailbox = self.mailbox.clone();
                net.inject(thread, &TurnInput { text, attachments: Vec::new() }, ctx.clone(), move |r| post(&mailbox, Reply::Sent(r)));
            }
            _ => self.send_prompt(net, ctx, text),
        }
    }

    fn steer(&mut self, net: &NetClient, ctx: &egui::Context, input: SteerInput) {
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
            self.drawing.clear();
            let stream = egui::ScrollArea::vertical()
                .id_salt("chat_stream")
                .auto_shrink([false, false])
                .stick_to_bottom(self.cursor.is_none())
                .max_height(scroll_height)
                .show(ui, |ui| self.render_stream(ui, net));
            self.view = stream.inner_rect;
            self.spots = std::mem::take(&mut self.drawing).into_iter().filter(|s| s.rect.intersects(stream.inner_rect)).collect();
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
            ui.label(RichText::new(name).strong().size(crate::theme::look().text_label));
            // Who it's signed in as, as it says.
            if let Some(account) = self.instance().and_then(|s| s.auth.account.clone()) {
                let detail = [account.email.clone(), account.organization.clone(), account.plan.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                let label = ui.label(RichText::new(format!("{} {}", icon::USER_CIRCLE, account.label)).size(crate::theme::look().text_small).color(colors().muted));
                if !detail.is_empty() {
                    label.on_hover_text(detail);
                }
            }
            // What of its configuration couldn't be read.
            if let Some(problems) = self.instance().map(|s| s.problems.clone()).filter(|p| !p.is_empty()) {
                ui.label(RichText::new(icon::WARNING).color(colors().warn)).on_hover_text(problems.join("\n"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for action in on_toolbar(ToolbarStyle::Button) {
                    if let Some(u) = toolbar_control(ui, action) {
                        used = Some((action.clone(), u));
                    }
                }
                if let Some(thread) = &self.active_thread {
                    let short: String = thread.as_str().chars().take(8).collect();
                    ui.label(RichText::new(format!("[{short}]")).monospace().size(crate::theme::look().text_small).color(colors().info));
                } else if self.opening {
                    ui.label(RichText::new(format!("{} Connecting…", icon::HOURGLASS)).size(crate::theme::look().text_small).color(colors().warn));
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
                ui.add(egui::Label::new(RichText::new(format!("{} MCP {}", icon::PLUGS_CONNECTED, thread.mcp_servers.len())).size(crate::theme::look().text_small).color(colors().muted)).extend())
                    .on_hover_text(names.join("\n"));
            }
            if let Some(usage) = self.thread().and_then(|t| t.usage.clone())
                && let (Some(used), Some(window)) = (usage.context_used_tokens, usage.context_window_tokens)
            {
                ui.add(egui::Label::new(RichText::new(format!("ctx {:.1}k/{:.0}k", used as f64 / 1000.0, window as f64 / 1000.0)).monospace().size(crate::theme::look().text_small).color(colors().muted)).extend());
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
                ui.label(RichText::new(icon::X_CIRCLE).size(crate::theme::look().text_label).color(colors().error));
                ui.label(RichText::new(&err).size(crate::theme::look().text_label).color(colors().text));
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
        if (needs_login || self.login.is_some())
            && let Some(snapshot) = self.instance().cloned()
        {
            let instance = snapshot.instance_id.clone();
            let look = crate::theme::look();
            banner(ui, colors().info_bg, colors().focus, |ui| match self.login.clone() {
                None => {
                    ui.label(RichText::new(format!("{} Sign in to {}:", icon::KEY, snapshot.display_name)).strong().size(look.text_label));
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
                Some(login) => {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&login.instructions).size(look.text_label));
                        if let Some(url) = &login.url {
                            ui.hyperlink_to(RichText::new(format!("{} Open the sign-in page", icon::ARROW_SQUARE_OUT)).strong(), url);
                            // Its address, to copy when the link can't open here.
                            let mut shown = url.clone();
                            ui.add(egui::TextEdit::singleline(&mut shown).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                        }
                        ui.horizontal(|ui| {
                            if let Some(expects) = &login.expects {
                                let hint = format!("Paste {expects}");
                                ui.add(egui::TextEdit::singleline(&mut self.auth_paste).hint_text(hint).desired_width(260.0));
                                if ui.button("Sign in").clicked() && !self.auth_paste.trim().is_empty() {
                                    let mailbox = self.mailbox.clone();
                                    let pasted = std::mem::take(&mut self.auth_paste);
                                    net.complete_auth(&instance, &pasted, ctx.clone(), move |r| post(&mailbox, Reply::Done(r)));
                                    self.login = None;
                                }
                            }
                            if ui.small_button("Cancel").clicked() {
                                self.login = None;
                            }
                        });
                    });
                }
            });
        }
    }

    fn render_plan(&mut self, ui: &mut egui::Ui) {
        let Some(PlanBody::TodoList { steps }) = self.thread().and_then(|t| t.plan.clone()).map(|p| p.body) else {
            return;
        };
        let done = steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        egui::CollapsingHeader::new(RichText::new(format!("{} Plan ({done}/{})", icon::CLIPBOARD_TEXT, steps.len())).size(crate::theme::look().text_body))
            .default_open(true)
            .show(ui, |ui| render_steps(ui, &steps));
    }

    fn render_stream(&mut self, ui: &mut egui::Ui, net: &NetClient) {
        let Some(view) = self.thread().cloned() else {
            ui.add_space(crate::theme::look().space_xxl);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No session yet").strong().size(crate::theme::look().text_heading).color(colors().accent));
                let hint = match self.instance() {
                    Some(s) => format!("{} • type below or Connect to start a session", s.display_name),
                    None => "Waiting for the server…".into(),
                };
                ui.label(RichText::new(hint).size(crate::theme::look().text_label).color(colors().muted));
            });
            return;
        };
        let scroll = std::mem::take(&mut self.scroll);
        let mut index = 0;
        for turn in &view.turns {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Turn {}", turn.ordinal)).monospace().size(crate::theme::look().text_small).color(colors().faint));
                if turn.status == TurnStatus::Queued {
                    ui.label(RichText::new("QUEUED").size(crate::theme::look().text_tiny).color(colors().muted));
                } else if !turn.status.is_terminal() {
                    ui.label(RichText::new("RUNNING").size(crate::theme::look().text_tiny).color(colors().warn));
                } else if let Some(f) = &turn.failure {
                    ui.label(RichText::new(format!("failed: {}", f.message)).size(crate::theme::look().text_tiny).color(colors().error));
                }
            });
            for item in view.items_of(&turn.id) {
                let on = self.cursor == Some(index);
                let fill = if on { colors().selection } else { Color32::TRANSPARENT };
                let shown = Frame::NONE.fill(fill).corner_radius(crate::theme::look().radius_s).inner_margin(crate::theme::margin2(crate::theme::look().space_xs, crate::theme::look().space_hair)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    self.render_item(ui, net, &view, item, index);
                });
                if on && scroll {
                    shown.response.scroll_to_me(Some(egui::Align::Center));
                }
                self.drawing.push(Spot { key: index.to_string(), rect: shown.response.rect });
                index += 1;
                ui.add_space(crate::theme::look().space_xs);
            }
            ui.add(egui::Separator::default().spacing(8.0));
        }
    }

    /// Every item in the active thread, in the order shown.
    fn stream(&self) -> Vec<TurnItem> {
        let Some(view) = self.thread() else { return Vec::new() };
        view.turns.iter().flat_map(|t| view.items_of(&t.id).cloned()).collect()
    }

    fn put_cursor(&mut self, index: usize) {
        let n = self.stream().len();
        if n > 0 {
            self.cursor = Some(index.min(n - 1));
            self.link_pick = None;
            self.scroll = true;
        }
    }

    fn half_page_items(&self) -> usize {
        let avg = if self.spots.is_empty() { 40.0 } else { self.spots.iter().map(|s| s.rect.height()).sum::<f32>() / self.spots.len() as f32 };
        if self.view.height() > 0.0 { ((self.view.height() / avg.max(8.0)) / 2.0).floor().max(1.0) as usize } else { 4 }
    }

    fn render_item(&mut self, ui: &mut egui::Ui, net: &NetClient, view: &ThreadView, item: &TurnItem, index: usize) {
        match &item.body {
            ItemBody::UserMessage { text } => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(">").monospace().size(crate::theme::look().text_heading).strong().color(colors().accent));
                    ui.label(RichText::new(text).size(crate::theme::look().text_title).strong().color(colors().text_strong));
                });
            }
            ItemBody::AssistantMessage { content, .. } => {
                let text: String = content.iter().filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None }).collect();
                if !text.is_empty() {
                    CommonMarkViewer::new().show(ui, &mut self.markdown_cache, &text);
                }
                for block in content.iter().filter(|b| !matches!(b, ContentBlock::Text { .. })) {
                    ui.label(RichText::new(block_summary(block)).monospace().size(crate::theme::look().text_small).color(colors().muted));
                }
            }
            ItemBody::Reasoning { text, streaming } => {
                let label = if *streaming { format!("{} Thinking… ({} chars)", icon::BRAIN, text.len()) } else { format!("{} Reasoning ({} chars)", icon::BRAIN, text.len()) };
                egui::CollapsingHeader::new(RichText::new(label).size(crate::theme::look().text_body).color(colors().special))
                    .id_salt(item.id.as_str())
                    .show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(colors().muted));
                    });
            }
            ItemBody::ToolCall { kind, name, output, locations, .. } => {
                let (tag, color) = status_tag(item.status);
                let title = item.title.clone().unwrap_or_else(|| name.clone());
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{tag} {} {title}", tool_icon(kind))).monospace().size(crate::theme::look().text_body).color(color));
                    for (n, loc) in locations.iter().enumerate() {
                        let label = match loc.line { Some(l) => format!("{} {}:{l}", icon::FOLDER_OPEN, loc.path), None => format!("{} {}", icon::FOLDER_OPEN, loc.path) };
                        let link = ui.small_button(RichText::new(label).monospace().size(crate::theme::look().text_small));
                        self.drawing.push(Spot { key: format!("{index}:{n}"), rect: link.rect });
                        if link.clicked() {
                            self.requested_file_jump = Some((loc.path.clone(), loc.line.map(|l| l as usize)));
                        }
                    }
                });
                if !output.is_empty() {
                    egui::CollapsingHeader::new(RichText::new("output").size(crate::theme::look().text_small).color(colors().muted))
                        .id_salt((item.id.as_str(), "out"))
                        .show(ui, |ui| {
                            for block in output {
                                ui.label(RichText::new(block_detail(block)).monospace().size(crate::theme::look().text_small).color(colors().muted));
                            }
                        });
                }
            }
            ItemBody::FileChange { path, .. } => {
                let link = ui.small_button(RichText::new(format!("{} {path}", icon::PENCIL_SIMPLE)).monospace().size(crate::theme::look().text_body));
                self.drawing.push(Spot { key: format!("{index}:0"), rect: link.rect });
                if link.clicked() {
                    self.requested_file_jump = Some((path.clone(), None));
                }
            }
            ItemBody::CommandExecution { command, output, exit_code } => {
                ui.label(RichText::new(format!("$ {command}")).monospace().size(crate::theme::look().text_body));
                if let Some(out) = output {
                    ui.label(RichText::new(out).monospace().size(crate::theme::look().text_small).color(colors().muted));
                }
                if let Some(code) = exit_code {
                    ui.label(RichText::new(format!("exit {code}")).monospace().size(crate::theme::look().text_small).color(colors().muted));
                }
            }
            ItemBody::ProposedPlan { markdown, .. } => {
                Frame::NONE.fill(colors().surface).inner_margin(crate::theme::margin(crate::theme::look().space_sm)).show(ui, |ui| {
                    ui.label(RichText::new(format!("{} Proposed plan", icon::CLIPBOARD_TEXT)).strong().size(crate::theme::look().text_body));
                    CommonMarkViewer::new().show(ui, &mut self.markdown_cache, markdown);
                });
            }
            ItemBody::TodoList { steps, .. } => render_steps(ui, steps),
            ItemBody::Request { request_id } => {
                if let Some(request) = view.request(request_id) {
                    render_request(ui, net, &view.thread.id, request, &mut self.drafts);
                }
            }
            ItemBody::Subagent { subagent_id } => {
                ui.label(RichText::new(format!("{} subagent {subagent_id}", icon::ROBOT)).size(crate::theme::look().text_body));
            }
            ItemBody::Notice { level, text } => {
                let color = match level { NoticeLevel::Info => colors().muted, NoticeLevel::Warning => colors().warn };
                ui.label(RichText::new(text).size(crate::theme::look().text_body).color(color));
            }
            ItemBody::Error { message } => {
                ui.label(RichText::new(format!("{} {message}", icon::WARNING)).size(crate::theme::look().text_body).color(colors().error));
            }
            ItemBody::Unknown { native_kind, display } => {
                let shown: String = display.chars().take(300).collect();
                ui.label(RichText::new(format!("[{native_kind}] {shown}")).monospace().size(crate::theme::look().text_small).color(colors().muted));
            }
            ItemBody::Unsupported => {
                ui.label(RichText::new("(item type not supported by this client)").size(crate::theme::look().text_small).color(colors().muted));
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
                    let label = RichText::new(format!("/{}", command.name)).monospace().size(crate::theme::look().text_small);
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
            let steerable = self.instance().is_some_and(|s| s.capabilities.turns.active_steering) && self.thread().and_then(|t| t.running_turn()).is_some();
            let hint = if inserting && steerable {
                "While it works: Enter queues this next, Ctrl+Enter adds it to what it's doing"
            } else if inserting {
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
            let send = ui.add_sized([send_w, 36.0], egui::Button::new(RichText::new("Send").strong().size(crate::theme::look().text_body))).clicked();
            let (enter, add) = ui.input(|i| {
                let enter = inserting && i.key_pressed(egui::Key::Enter) && !i.modifiers.shift;
                (enter, enter && i.modifiers.command)
            });
            if (send || enter) && !self.input_buffer.trim().is_empty() {
                let text = self.input_buffer.trim().to_string();
                self.input_buffer.clear();
                if add {
                    self.add_to_turn(net, &ctx, text);
                } else {
                    self.send_prompt(net, &ctx, text);
                }
            }
        });
    }
}

/// The files an item points at, in the order they're drawn.
fn links_of(item: &TurnItem) -> Vec<(String, Option<usize>)> {
    match &item.body {
        ItemBody::ToolCall { locations, .. } => locations.iter().map(|l| (l.path.clone(), l.line.map(|l| l as usize))).collect(),
        ItemBody::FileChange { path, .. } => vec![(path.clone(), None)],
        _ => Vec::new(),
    }
}

/// What search looks through in an item.
fn item_text(item: &TurnItem) -> String {
    match &item.body {
        ItemBody::UserMessage { text } => text.clone(),
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

impl Navigable for ChatStreamState {
    fn place(&self) -> Place {
        Place::Chat { message: self.cursor }
    }

    fn restore(&mut self, place: &Place) {
        if let Place::Chat { message } = place {
            self.cursor = *message;
            self.link_pick = None;
            self.scroll = message.is_some();
        }
    }

    fn target(&self) -> Option<Target> {
        let index = self.cursor?;
        let item = self.stream().into_iter().nth(index)?;
        let links = links_of(&item);
        let link = self.link_pick.and_then(|n| links.get(n).cloned()).or_else(|| links.first().cloned());
        Some(Target::Message { thread: self.active_thread.as_ref().map(|t| t.to_string()), index, link })
    }

    /// From the latest message (none under the cursor yet), moves start at
    /// the bottom.
    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.stream().len();
        if n == 0 {
            return;
        }
        let at = self.cursor.unwrap_or(n - 1);
        let fresh = self.cursor.is_none();
        match motion {
            Motion::Down => self.put_cursor(if fresh { at } else { at + count }),
            Motion::Up => self.put_cursor(if fresh { at } else { at.saturating_sub(count) }),
            Motion::HalfDown => self.put_cursor(at + self.half_page_items() * count),
            Motion::HalfUp => self.put_cursor(at.saturating_sub(self.half_page_items() * count)),
            Motion::First => self.put_cursor(0),
            Motion::Last => self.put_cursor(n - 1),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1)),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right => {}
        }
    }

    /// A message's file opens in the code view.
    fn enter(&mut self) -> Step {
        match self.target() {
            Some(Target::Message { link: Some(_), .. }) => Step::GoTo(GoTo::Code),
            Some(_) => Step::Nothing("No file in this message.".into()),
            None => Step::Nothing("Move to a message first (j / k).".into()),
        }
    }

    fn can_enter(&self) -> bool {
        matches!(self.target(), Some(Target::Message { link: Some(_), .. }))
    }

    fn can_leave(&self) -> bool {
        self.cursor.is_some()
    }

    fn enter_label(&self) -> Option<String> {
        match self.target()? {
            Target::Message { link: Some((path, _)), .. } => Some(format!("Open {path}")),
            _ => None,
        }
    }

    /// Lets go of the cursor: the stream follows the latest again.
    fn leave(&mut self) -> Step {
        if self.cursor.take().is_none() {
            return Step::Nothing("Already following the latest.".into());
        }
        Step::Done
    }

    fn fold(&mut self, _fold: Fold) {}

    fn items(&self) -> Vec<Item> {
        self.stream().iter().enumerate().map(|(i, item)| Item::new(i.to_string(), "", item_text(item))).collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.cursor.map(|c| c.to_string())
    }

    /// A message ("3") or one of its links ("3:1").
    fn move_to(&mut self, key: &str) {
        let mut parts = key.split(':');
        let Some(index) = parts.next().and_then(|i| i.parse::<usize>().ok()) else { return };
        self.put_cursor(index);
        self.link_pick = parts.next().and_then(|n| n.parse().ok());
    }
}

fn mcp_label(server: &McpServer) -> String {
    match &server.transport {
        McpTransport::Http { url, .. } | McpTransport::Sse { url, .. } => format!("{} — {url}", server.name),
        McpTransport::Stdio { command, .. } => format!("{} — {command}", server.name),
    }
}

impl ChatStreamState {
    fn load_saved(&mut self, net: &NetClient, ctx: &egui::Context, then: &str) {
        let Some(instance) = self.active_instance.clone() else { return };
        self.after_saved = Some(then.to_owned());
        let mailbox = self.mailbox.clone();
        net.list_saved_sessions(&instance, ctx.clone(), move |r| post(&mailbox, Reply::Saved(r.map(|p| p.sessions))));
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

    /// Saves the project's tool servers (the session restarts with them).
    fn save_tools(&mut self, net: &NetClient, ctx: &egui::Context, project: &[McpServer]) {
        let mailbox = self.mailbox.clone();
        net.set_tool_servers(project, ctx.clone(), move |result| post(&mailbox, Reply::Tools { result, saved: true }));
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
    ListSkills,
    AddToTurn,
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
        out.push(Action::new("chat.insert", "Type a prompt", C::Insert).focused().importance(crate::actions::PRIMARY));
        if !self.input_buffer.is_empty() {
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
            && !self.input_buffer.trim().is_empty()
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
            ChatCommand::ListMcp | ChatCommand::ListSkills => {}
            ChatCommand::AddToTurn => {
                let text = std::mem::take(&mut self.input_buffer).trim().to_owned();
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

fn render_steps(ui: &mut egui::Ui, steps: &[PlanStep]) {
    for step in steps {
        let (mark, color) = match step.status {
            StepStatus::Completed => (icon::CHECK_SQUARE, colors().ok),
            StepStatus::InProgress => (icon::CIRCLE_HALF, colors().warn),
            StepStatus::Pending => (icon::SQUARE, colors().muted),
        };
        ui.label(RichText::new(format!("{mark} {}", step.text)).size(crate::theme::look().text_body).color(color));
    }
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use harness_contracts::{AdapterEvent, FileLocation, InstanceId, ItemId, RuntimePolicy, Thread, ThreadStatus, Turn, TurnId, TurnStatus};

    fn session(event: AdapterEvent) -> ClientEvent {
        ClientEvent::Session { event }
    }

    pub(crate) fn item(n: u32, body: ItemBody) -> ClientEvent {
        session(AdapterEvent::ItemUpdated {
            item: TurnItem {
                id: ItemId::new(format!("i{n}")),
                thread_id: ThreadId::new("t"),
                turn_id: TurnId::new("turn"),
                native_id: None,
                parent_id: None,
                ordinal: n,
                status: ItemStatus::Completed,
                title: None,
                body,
            },
        })
    }

    /// A thread with a prompt, an answer, a tool call reading two files and a file change.
    pub(crate) fn conversation() -> ChatStreamState {
        let mut chat = ChatStreamState::default();
        let thread = Thread { id: ThreadId::new("t"), instance_id: InstanceId::new("i"), native_id: None, title: None, status: ThreadStatus::Idle, policy: RuntimePolicy::default(), model: None, controls: Default::default(), mcp_servers: Vec::new() };
        chat.process_event(session(AdapterEvent::ThreadUpdated { thread }));
        chat.process_event(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Completed, failure: None, usage: None } }));
        chat.process_event(item(0, ItemBody::UserMessage { text: "fix the parser".into() }));
        chat.process_event(item(1, ItemBody::AssistantMessage { content: vec![ContentBlock::Text { text: "Looking at it.".into() }], streaming: false }));
        let locations = vec![FileLocation { path: "src/parse.rs".into(), line: Some(12) }, FileLocation { path: "src/lex.rs".into(), line: None }];
        chat.process_event(item(2, ItemBody::ToolCall { kind: ToolKind::Read, name: "read".into(), input: None, output: vec![], locations }));
        chat.process_event(item(3, ItemBody::FileChange { path: "src/parse.rs".into(), diff: None }));
        chat
    }

    #[test]
    fn a_harness_that_cant_start_says_why_and_the_fix() {
        let mut chat = ChatStreamState::default();
        assert_eq!(chat.harness_problem(), None);
        let snapshot = |status: ProviderStatus| ProviderSnapshot {
            instance_id: InstanceId::new("antigravity"),
            driver: harness_contracts::DriverKind::new("antigravity"),
            display_name: "Antigravity".into(),
            status,
            auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::Unknown, account: None, methods: Vec::new() },
            capabilities: Default::default(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: Vec::new(),
            runtime_modes: Vec::new(),
            supports_plan_mode: false,
            tool_servers: Vec::new(),
            problems: Vec::new(),
        };
        let reason = "antigravity-acp isn't installed (fix: meta-harness pack install antigravity-acp)";
        chat.process_event(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Unavailable { reason: reason.into() }) });
        assert_eq!(chat.harness_problem().as_deref(), Some(format!("Antigravity: {reason}").as_str()), "the AGENT light's hint");
        chat.process_event(ClientEvent::InstanceUpdated { snapshot: snapshot(ProviderStatus::Ready) });
        assert_eq!(chat.harness_problem(), None);
    }

    #[test]
    fn messages_take_a_cursor_and_their_links_open() {
        let mut chat = conversation();
        assert_eq!(chat.target(), None, "no cursor: the stream follows the latest");
        chat.motion(Motion::Up, 1);
        assert_eq!(chat.cursor, Some(3), "the first move starts at the latest message");
        chat.motion(Motion::Up, 2);
        assert_eq!(chat.cursor, Some(1));
        chat.motion(Motion::Last, 1);
        assert_eq!(chat.cursor, Some(3));
        chat.motion(Motion::First, 1);
        assert_eq!(chat.cursor, Some(0));
        assert!(matches!(chat.enter(), Step::Nothing(_)), "a prompt has no file");
        chat.motion(Motion::Line(3), 1);
        assert_eq!(chat.target(), Some(Target::Message { thread: Some("t".into()), index: 2, link: Some(("src/parse.rs".into(), Some(12))) }));
        assert_eq!(chat.enter(), Step::GoTo(GoTo::Code), "Enter opens the file it points to");
        // A hint on the second link picks it.
        chat.move_to("2:1");
        assert_eq!(chat.target().and_then(|t| t.code()), Some(("src/lex.rs".into(), None)));
        // Search finds messages by what they say.
        let items = chat.items();
        assert_eq!(crate::nav::rank_matches(&items, "parser"), vec![0]);
        assert_eq!(crate::nav::rank_matches(&items, "lex.rs"), vec![2]);
        // Backspace lets go: the stream follows the latest again.
        assert_eq!(chat.leave(), Step::Done);
        assert_eq!(chat.cursor, None);
        // What's drawn takes hints: each message, and each link.
        let ctx = egui::Context::default();
        let net = NetClient::new("http://127.0.0.1:9");
        let mut feedback = FeedbackInspectorState::default();
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 900.0))), ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let mut mode = crate::keymap::InputMode::Normal;
                    let mut note = None;
                    let mut cx = RunCx { net: &net, ctx, input_mode: &mut mode, slot: 0, note: &mut note, shell: &[] };
                    chat.render(ui, &mut feedback, true, &mut cx);
                });
            });
        }
        let keys: Vec<String> = chat.spots().into_iter().map(|s| s.key).collect();
        for key in ["0", "1", "2", "2:0", "2:1", "3", "3:0"] {
            assert!(keys.contains(&key.to_owned()), "{key} in {keys:?}");
        }
    }

    #[test]
    fn its_skills_tool_servers_and_adding_to_the_running_turn_are_actions() {
        let mut chat = conversation();
        let ids = |chat: &ChatStreamState| chat.actions().into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert!(!ids(&chat).iter().any(|a| a == "mcp.add" || a == "harness.skills" || a == "chat.inject"), "nothing known yet");
        let mut snapshot = ProviderSnapshot {
            instance_id: InstanceId::new("i"),
            driver: harness_contracts::DriverKind::new("acp"),
            display_name: "Claude Code".into(),
            status: ProviderStatus::Ready,
            auth: harness_contracts::AuthState { status: harness_contracts::AuthStatus::SignedIn, account: None, methods: Vec::new() },
            capabilities: Default::default(),
            models: Vec::new(),
            commands: Vec::new(),
            skills: vec![harness_contracts::Skill { name: "review".into(), description: Some("Review".into()), path: None, scope: harness_contracts::Scope::User }],
            runtime_modes: Vec::new(),
            supports_plan_mode: false,
            tool_servers: Vec::new(),
            problems: Vec::new(),
        };
        snapshot.capabilities.turns.active_steering = true;
        chat.active_instance = Some(snapshot.instance_id.clone());
        chat.process_event(ClientEvent::InstanceUpdated { snapshot });
        chat.tools = Some(ToolServers::default());
        chat.process_event(session(AdapterEvent::TurnUpdated { turn: Turn { id: TurnId::new("turn"), thread_id: ThreadId::new("t"), ordinal: 1, status: TurnStatus::Running, failure: None, usage: None } }));
        chat.input_buffer = "also".into();
        let ids = ids(&chat);
        for id in ["mcp.add", "harness.skills", "chat.inject"] {
            assert!(ids.iter().any(|a| a == id), "{id} in {ids:?}");
        }
    }
}
