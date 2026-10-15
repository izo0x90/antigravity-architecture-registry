//! Drawing the chat: controls, banners, the stream, items, the input.

use super::*;
use crate::panel::{Status, Tone};

pub(super) fn mcp_label(server: &McpServer) -> String {
    match &server.transport {
        McpTransport::Http { url, .. } | McpTransport::Sse { url, .. } => format!("{} — {url}", server.name),
        McpTransport::Stdio { command, .. } => format!("{} — {command}", server.name),
    }
}

pub(super) fn render_steps(ui: &mut egui::Ui, steps: &[PlanStep]) {
    for step in steps {
        let (mark, color) = match step.status {
            StepStatus::Completed => (icon::CHECK_SQUARE, colors().ok),
            StepStatus::InProgress => (icon::CIRCLE_HALF, colors().warn),
            StepStatus::Pending => (icon::SQUARE, colors().muted),
        };
        ui.label(RichText::new(format!("{mark} {}", step.text)).size(crate::theme::look().text_body).color(color));
    }
}

pub(super) fn block_summary(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text { text } => text.clone(),
        ContentBlock::Image { mime_type, .. } => format!("[image {mime_type}]"),
        ContentBlock::ResourceLink { uri, name } => format!("[link {}]", name.clone().unwrap_or_else(|| uri.clone())),
        ContentBlock::Resource { uri, .. } => format!("[resource {uri}]"),
        ContentBlock::Diff(d) => format!("[diff {}]", d.path),
        ContentBlock::Unsupported => "[unsupported content]".into(),
    }
}

pub(super) fn block_detail(block: &ContentBlock) -> String {
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

impl ChatStreamState {
    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, is_focused: bool, cx: &mut RunCx<'_>) {
        let (net, slot_idx) = (cx.net, cx.slot);
        let ctx = ui.ctx().clone();
        self.drain_replies(net, &ctx);
        self.drawn.insert(self.ui.slot.clone());

        ui.vertical(|ui| {
            self.render_banners(ui);
            self.render_plan(ui);

            let scroll_height = (ui.available_height() - 105.0).max(50.0);
            self.ui.drawing.clear();
            let stream = egui::ScrollArea::vertical()
                .id_salt(("chat_stream", slot_idx))
                .auto_shrink([false, false])
                .stick_to_bottom(self.ui.cursor.is_none())
                .max_height(scroll_height)
                .show(ui, |ui| self.render_stream(ui));
            self.ui.view = stream.inner_rect;
            self.ui.spots = std::mem::take(&mut self.ui.drawing).into_iter().filter(|s| s.rect.intersects(stream.inner_rect)).collect();
            feedback.register_inspectable(ui, &format!("slot_{}:chat:stream", slot_idx + 1), stream.inner_rect);
            ui.separator();

            let start = ui.cursor().min;
            let w = ui.available_width();
            self.render_input(ui, feedback, is_focused, cx);
            let input = Rect::from_min_size(Pos2::new(start.x, start.y), Vec2::new(w, (ui.cursor().min.y - start.y).max(40.0)));
            feedback.register_inspectable(ui, &format!("slot_{}:chat:input_bar", slot_idx + 1), input);
        });
    }

    pub(super) fn render_banners(&mut self, ui: &mut egui::Ui) {
        // What it says; its ways out (Dismiss, Retry, signing in) are its
        // actions, on its top line.
        if let Some(notice) = self.notice.clone() {
            banner(ui, colors().info_bg, colors().focus, |ui| {
                ui.label(RichText::new(icon::INFO).size(crate::theme::look().text_label).color(colors().info));
                ui.label(RichText::new(&notice).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        if let Some(err) = self.error().cloned() {
            banner(ui, colors().error_bg, colors().error, |ui| {
                ui.label(RichText::new(icon::X_CIRCLE).size(crate::theme::look().text_label).color(colors().error));
                ui.label(RichText::new(&err).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        if (self.needs_login() || self.login.is_some())
            && let Some(snapshot) = self.instance().cloned()
        {
            let look = crate::theme::look();
            banner(ui, colors().info_bg, colors().focus, |ui| match self.login.clone() {
                None => {
                    ui.label(RichText::new(format!("{} Sign in to {}", icon::KEY, snapshot.display_name)).strong().size(look.text_label));
                }
                Some(login) => {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&login.instructions).size(look.text_label));
                        if let Some(url) = &login.url {
                            // Its address, to copy when the page can't open here.
                            let mut shown = url.clone();
                            ui.add(egui::TextEdit::singleline(&mut shown).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace));
                        }
                    });
                }
            });
        }
    }

    /// The harness offers sign-in and isn't signed in (or said so).
    pub(super) fn needs_login(&self) -> bool {
        self.instance().is_some_and(|s| {
            !s.auth.methods.is_empty() && (s.auth.status == harness_contracts::AuthStatus::SignedOut || self.error().is_some_and(|e| e.to_lowercase().contains("authentication")))
        })
    }

    pub(super) fn render_plan(&mut self, ui: &mut egui::Ui) {
        let Some(PlanBody::TodoList { steps }) = self.thread().and_then(|t| t.plan.clone()).map(|p| p.body) else {
            return;
        };
        let done = steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        egui::CollapsingHeader::new(RichText::new(format!("{} Plan ({done}/{})", icon::CLIPBOARD_TEXT, steps.len())).size(crate::theme::look().text_body))
            .default_open(true)
            .show(ui, |ui| render_steps(ui, &steps));
    }

    pub(super) fn render_stream(&mut self, ui: &mut egui::Ui) {
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
        let scroll = std::mem::take(&mut self.ui.scroll);
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
                let on = self.ui.cursor == Some(index);
                let fill = if on { colors().selection } else { Color32::TRANSPARENT };
                let shown = Frame::NONE.fill(fill).corner_radius(crate::theme::look().radius_s).inner_margin(crate::theme::margin2(crate::theme::look().space_xs, crate::theme::look().space_hair)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    self.render_item(ui, &view, item, index);
                });
                if on && scroll {
                    shown.response.scroll_to_me(Some(egui::Align::Center));
                }
                self.ui.drawing.push(Spot { key: index.to_string(), rect: shown.response.rect });
                index += 1;
                ui.add_space(crate::theme::look().space_xs);
            }
            ui.add(egui::Separator::default().spacing(8.0));
        }
    }

    /// Every item in the active thread, in the order shown.
    pub(super) fn stream(&self) -> Vec<TurnItem> {
        let Some(view) = self.thread() else { return Vec::new() };
        view.turns.iter().flat_map(|t| view.items_of(&t.id).cloned()).collect()
    }

    pub(super) fn put_cursor(&mut self, index: usize) {
        let n = self.stream().len();
        if n > 0 {
            self.ui.cursor = Some(index.min(n - 1));
            self.ui.link_pick = None;
            self.ui.scroll = true;
        }
    }

    pub(super) fn half_page_items(&self) -> usize {
        let avg = if self.ui.spots.is_empty() { 40.0 } else { self.ui.spots.iter().map(|s| s.rect.height()).sum::<f32>() / self.ui.spots.len() as f32 };
        if self.ui.view.height() > 0.0 { ((self.ui.view.height() / avg.max(8.0)) / 2.0).floor().max(1.0) as usize } else { 4 }
    }

    pub(super) fn render_item(&mut self, ui: &mut egui::Ui, view: &ThreadView, item: &TurnItem, index: usize) {
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
                    CommonMarkViewer::new().show(ui, &mut self.ui.markdown_cache, &text);
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
                        // A place it went: the cursor goes on it (a click),
                        // and it opens (Enter, a double click).
                        let link = ui.label(RichText::new(label).monospace().size(crate::theme::look().text_small).color(colors().info));
                        self.ui.drawing.push(Spot { key: format!("{index}:{n}"), rect: link.rect });
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
                let link = ui.label(RichText::new(format!("{} {path}", icon::PENCIL_SIMPLE)).monospace().size(crate::theme::look().text_body).color(colors().info));
                self.ui.drawing.push(Spot { key: format!("{index}:0"), rect: link.rect });
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
                    CommonMarkViewer::new().show(ui, &mut self.ui.markdown_cache, markdown);
                });
            }
            ItemBody::TodoList { steps, .. } => render_steps(ui, steps),
            ItemBody::Request { request_id } => {
                if let Some(request) = view.request(request_id) {
                    render_request(ui, request, &mut self.drafts);
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

    pub(super) fn render_input(&mut self, ui: &mut egui::Ui, feedback: &FeedbackInspectorState, is_focused: bool, cx: &mut RunCx<'_>) {
        let (net, slot_idx, hints) = (cx.net, cx.slot, cx.hints);
        let input_mode = &mut *cx.input_mode;
        let ctx = ui.ctx().clone();

        ui.horizontal(|ui| {
            let text_w = ui.available_width();
            let inserting = *input_mode == crate::keymap::InputMode::Insert { slot: slot_idx };
            let steerable = self.instance().is_some_and(|s| s.capabilities.turns.active_steering) && self.thread().and_then(|t| t.running_turn()).is_some();
            let keys = crate::keymap::local();
            let hint = if inserting && steerable {
                format!("While it works: {} queues this next, {} adds it to what it's doing", keys.input.send_label(), keys.input.add_label())
            } else if inserting {
                format!("Instructions… ({}; / for commands)", keys.typing_hint("leaves the field"))
            } else {
                match crate::actions::key_of(hints, "chat.insert") {
                    Some(key) => format!("{key} or a click to type…"),
                    None => "Click to type…".to_owned(),
                }
            };
            let output = ui.add(
                keys.text_box(&mut self.ui.input_buffer)
                    .id(egui::Id::new(("chat_persistent_input_text_edit", slot_idx)))
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
            // Its Send (and Add) are actions, under it while it's typed in.
            self.ui.typing = inserting.then_some(output.rect);
            let sent = if inserting { ui.input(|i| keys.input.sent(i)) } else { None };
            let (enter, add) = (sent.is_some(), sent == Some(crate::keymap::Sent::Add));
            if enter && !self.ui.input_buffer.trim().is_empty() {
                let text = self.ui.input_buffer.trim().to_string();
                self.ui.input_buffer.clear();
                if add {
                    self.add_to_turn(net, &ctx, text);
                } else {
                    self.send_prompt(net, &ctx, text);
                }
            }
        });
    }
}

impl crate::panel::Panel for ChatStreamState {
    /// The agent slot it shows (its role on hover).
    fn title(&self) -> Status {
        match self.slot() {
            Some(slot) => Status::new(format!("{} {}", icon::USER_FOCUS, slot.name), if slot.is_lead() { Tone::Accent } else { Tone::Info }).detail(format!("agent slot ({})", slot.role)),
            None => Status::new("No agent slot", Tone::Muted),
        }
    }

    fn typing_rect(&self) -> Option<Rect> {
        self.ui.typing
    }

    /// Following the latest, you're at what waits on you: its request.
    fn here(&self) -> Option<String> {
        self.ui.cursor.is_none().then(|| self.pending_item()).flatten().map(|i| i.to_string())
    }

    /// Its harness, who it's signed in as, what of its configuration
    /// couldn't be read, its session (or that it's connecting), its tool
    /// servers and how much of its context is used.
    fn status(&self) -> Vec<Status> {
        let mut out = vec![Status::new(self.instance().map(|s| s.display_name.clone()).unwrap_or_else(|| "No harness".into()), Tone::Plain)];
        if let Some(account) = self.instance().and_then(|s| s.auth.account.clone()) {
            let detail = [account.email.clone(), account.organization.clone(), account.plan.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let status = Status::new(format!("{} {}", icon::USER_CIRCLE, account.label), Tone::Muted);
            out.push(if detail.is_empty() { status } else { status.detail(detail) });
        }
        if let Some(problems) = self.instance().map(|s| s.problems.clone()).filter(|p| !p.is_empty()) {
            out.push(Status::new(icon::WARNING, Tone::Warn).detail(problems.join("\n")));
        }
        if let Some(thread) = &self.active_thread() {
            out.push(Status::new(format!("[{}]", thread.as_str().chars().take(8).collect::<String>()), Tone::Info));
        } else if self.opening() {
            out.push(Status::new(format!("{} Connecting…", icon::HOURGLASS), Tone::Warn));
        }
        if let Some(thread) = self.thread().map(|v| &v.thread)
            && !thread.mcp_servers.is_empty()
        {
            let names: Vec<String> = thread.mcp_servers.iter().map(mcp_label).collect();
            out.push(Status::new(format!("{} MCP {}", icon::PLUGS_CONNECTED, thread.mcp_servers.len()), Tone::Muted).detail(names.join("\n")));
        }
        if let Some(usage) = self.thread().and_then(|t| t.usage.clone())
            && let (Some(used), Some(window)) = (usage.context_used_tokens, usage.context_window_tokens)
        {
            out.push(Status::new(format!("ctx {:.1}k/{:.0}k", used as f64 / 1000.0, window as f64 / 1000.0), Tone::Muted));
        }
        out
    }
}
