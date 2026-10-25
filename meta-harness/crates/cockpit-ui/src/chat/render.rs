//! Drawing the chat: controls, banners, the stream, items, the input.

use super::*;
use crate::hosted::{Status, Tone};

/// The banners' name as a place (`here`).
pub(crate) const BANNERS: &str = "banners";

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

/// What says a fold is folded or open.
fn fold_mark(open: bool) -> &'static str {
    if open { icon::CARET_DOWN } else { icon::CARET_RIGHT }
}

/// A folded run of calls: one line saying what it did.
fn render_run(ui: &mut egui::Ui, words: &str, calls: usize) {
    let line = format!("{} {words}", fold_mark(false));
    ui.add(egui::Label::new(RichText::new(line).size(crate::theme::look().text_body).color(colors().muted)).truncate())
        .on_hover_text(format!("{calls} calls, folded"));
}

/// The most of the chat's height what waits on you takes (it scrolls past
/// that).
const WAITING_SHARE: f32 = 0.4;

impl ChatStreamState {
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, typing: bool, cx: &mut RunCx<'_>) {
        let host = cx.at.name();
        let ctx = ui.ctx().clone();
        let connected = cx.net.ws_state.lock().is_ok_and(|s| *s == crate::net::WsState::Open);
        self.note_updates(ui.input(|i| i.time), connected);

        ui.vertical(|ui| {
            let banners = self.render_banners(ui);
            self.render_plan(ui);
            // The stream in what's left above what's under it (what waits
            // on you, the typing box), as tall as that was last frame.
            let whole = ui.available_height();
            let scroll_height = (whole - self.ui.below).max(50.0);
            self.ui.drawing.clear();
            self.ui.item_rects.clear();
            let area = egui::ScrollArea::vertical().id_salt(("chat_stream", cx.at)).auto_shrink([false, false]).stick_to_bottom(self.ui.cursor.is_none()).max_height(scroll_height);
            // Where the shared rules say (following the latest, nothing's
            // asked: the newest stays in view); each frame's messages are
            // measured either way, for the first move from it.
            let area = self.ui.scrolling.area(area, ui.input(|i| i.time));
            let stream = area.show(ui, |ui| self.render_stream(ui));
            // Each item's stop, in the stream's order.
            for rect in std::mem::take(&mut self.ui.item_rects) {
                self.ui.scrolling.item(rect);
            }
            self.ui.scrolling.drawn(&stream);
            if self.ui.scrolling.busy() {
                ctx.request_repaint();
            }
            self.ui.spots = std::mem::take(&mut self.ui.drawing).into_iter().filter(|s| s.rect.intersects(stream.inner_rect)).collect();
            // The banners: where you are while nothing else waits (their
            // ways out are under them).
            self.ui.banners = banners;
            feedback.register_inspectable(ui, &format!("{host}:chat:stream"), stream.inner_rect);
            let top = ui.cursor().top();
            ui.separator();
            // What waits on you, in one place, always in sight (the
            // history marks where it was asked): each its card, a place of
            // the chat's; at most a share of the chat's height, scrolling
            // past that.
            let pending = self.pending();
            if !pending.is_empty() {
                let most = (whole * WAITING_SHARE).max(80.0);
                egui::ScrollArea::vertical().id_salt(("chat_waiting", cx.at)).max_height(most).min_scrolled_height(most).auto_shrink([false, true]).show(ui, |ui| {
                    for (index, request) in &pending {
                        let rect = self.item_frame(ui, *index, |chat, ui| {
                            crate::request_card::render_request(ui, request, &mut chat.drafts);
                        });
                        if let Some(index) = index {
                            self.ui.spots.push(Spot { key: index.to_string(), rect });
                        }
                    }
                });
            }
            let start = ui.cursor().min;
            let w = ui.available_width();
            self.render_input(ui, typing, cx);
            let input = Rect::from_min_size(Pos2::new(start.x, start.y), Vec2::new(w, (ui.cursor().min.y - start.y).max(40.0)));
            feedback.register_inspectable(ui, &format!("{host}:chat:input_bar"), input);
            // What's under the stream changed height: the stream makes room
            // for it in the next frame, at once.
            let below = ui.cursor().top() - top + ui.spacing().item_spacing.y;
            if (below - self.ui.below).abs() > 0.5 {
                self.ui.below = below;
                ctx.request_repaint();
            }
        });
    }

    /// An item as the chat draws it, in the stream or where what waits is
    /// kept: in the column's width, filled while the cursor's on it
    /// (`index`: its place in the stream). Where it was drawn.
    fn item_frame(&mut self, ui: &mut egui::Ui, index: Option<usize>, draw: impl FnOnce(&mut Self, &mut egui::Ui)) -> Rect {
        let on = index.is_some() && self.ui.cursor == index;
        let fill = if on { colors().selection } else { Color32::TRANSPARENT };
        let look = crate::theme::look();
        Frame::NONE
            .fill(fill)
            .corner_radius(look.radius_s)
            .inner_margin(crate::theme::margin2(look.space_xs, look.space_hair))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                draw(self, ui);
            })
            .response
            .rect
    }

    /// The banners: what the chat says to you now (a notice, an error, a
    /// sign-in, no session yet). Their ways out (Put away, Retry, the
    /// sign-in's steps, New session) are its actions, under them while
    /// they're where you are. Where they were drawn.
    pub(super) fn render_banners(&mut self, ui: &mut egui::Ui) -> Option<Rect> {
        let top = ui.cursor().top();
        let (notice, now) = (self.agents().notice().cloned(), self.sign_in_now());
        if let Some(notice) = notice {
            banner(ui, colors().info_bg, colors().focus, |ui| {
                ui.label(RichText::new(icon::INFO).size(crate::theme::look().text_label).color(colors().info));
                ui.label(RichText::new(&notice).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        if let Some(err) = self.error() {
            banner(ui, colors().error_bg, colors().error, |ui| {
                ui.label(RichText::new(icon::X_CIRCLE).size(crate::theme::look().text_label).color(colors().error));
                ui.label(RichText::new(&err).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        let instance = self.instance().map(|s| s.clone());
        if let Some(why) = now.as_ref().and_then(|n| n.pasted.as_ref()).and_then(|p| p.refused.clone()) {
            banner(ui, colors().error_bg, colors().error, |ui| {
                ui.label(RichText::new(icon::X_CIRCLE).size(crate::theme::look().text_label).color(colors().error));
                ui.label(RichText::new(format!("Not taken: {why}")).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        if self.thread().is_none() {
            let said = match &instance {
                Some(s) => format!("No session yet with {}: type below to start one.", s.display_name),
                None => "Waiting for the server…".to_owned(),
            };
            banner(ui, colors().info_bg, colors().focus, |ui| {
                ui.label(RichText::new(said).size(crate::theme::look().text_label).color(colors().text));
            });
        }
        if self.signing_in_said()
            && let Some(snapshot) = instance
        {
            let look = crate::theme::look();
            banner(ui, colors().info_bg, colors().focus, |ui| match now {
                None => {
                    let instance = &snapshot.instance_id;
                    let name = &snapshot.display_name;
                    let ended = self.agents().sign_in_ended(instance).map(|e| e.how.clone());
                    let said = match ended {
                        _ if self.agents().signing_in(instance) => format!("{} Starting the sign-in to {name}…", icon::HOURGLASS),
                        Some(harness_contracts::SignInEnd::RanOut) => format!("{} The sign-in to {name} stopped waiting: its time ran out. Sign in again for a new link.", icon::CLOCK),
                        Some(harness_contracts::SignInEnd::Failed { reason }) => format!("{} The sign-in to {name} failed: {reason}. Sign in again.", icon::X_CIRCLE),
                        _ => format!("{} Sign in to {name}", icon::KEY),
                    };
                    ui.label(RichText::new(said).strong().size(look.text_label));
                }
                Some(now) => {
                    ui.vertical(|ui| {
                        let said = |ui: &mut egui::Ui, text: &str, color: egui::Color32| {
                            ui.label(RichText::new(text).size(look.text_label).color(color));
                        };
                        if now.ran_out {
                            said(ui, &format!("{} The sign-in stopped waiting. Sign in again for a new link.", icon::CLOCK), colors().text);
                            return;
                        }
                        said(ui, &now.waiting.instructions, colors().text);
                        if let Some(url) = &now.waiting.url {
                            // Its address as text (all of it on hover): Copy link and
                            // Open take it from here.
                            ui.add(egui::Label::new(RichText::new(url).monospace().size(look.text_small).color(colors().muted)).truncate());
                        }
                        if let Some(left) = now.left_s {
                            said(ui, &format!("{} {}:{:02} left", icon::TIMER, left / 60, left % 60), colors().muted);
                            // Its time left, to the second, while it's shown.
                            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                        }
                        if now.pasted.as_ref().is_some_and(|p| p.finishing) {
                            said(ui, "Checking what you pasted…", colors().muted);
                        }
                    });
                }
            });
        }
        let bottom = ui.cursor().top();
        (bottom > top).then(|| Rect::from_min_max(Pos2::new(ui.min_rect().left(), top), Pos2::new(ui.max_rect().right(), bottom)))
    }

    /// How its last sign-in ended, when that's worth saying (it ran out or
    /// failed, and it isn't signed in since) and it isn't put away.
    pub(super) fn sign_in_end_to_say(&self) -> Option<harness_contracts::SignInEnded> {
        let instance = self.active_instance()?;
        if self.instance().is_some_and(|s| s.auth.status == harness_contracts::AuthStatus::SignedIn) {
            return None;
        }
        let ended = self.agents().sign_in_ended(&instance).cloned()?;
        matches!(ended.how, harness_contracts::SignInEnd::RanOut | harness_contracts::SignInEnd::Failed { .. }).then_some(ended)
    }

    /// The sign-in has a banner: one is needed, under way, starting, or
    /// its end is worth saying.
    pub(super) fn signing_in_said(&self) -> bool {
        let starting = self.instance().is_some_and(|s| self.agents().signing_in(&s.instance_id));
        self.needs_login() || self.sign_in_now().is_some() || starting || self.sign_in_end_to_say().is_some()
    }

    /// A banner shows (what `render_banners` draws).
    pub(super) fn banners_shown(&self) -> bool {
        let refused = self.sign_in_now().and_then(|n| n.pasted).is_some_and(|p| p.refused.is_some());
        self.agents().notice().is_some() || self.error().is_some() || refused || self.thread().is_none() || (self.signing_in_said() && self.instance().is_some())
    }

    /// The banners are where you are (their ways out are under them).
    pub(super) fn on_banners(&self) -> bool {
        crate::hosted::Hosted::here(self).as_deref() == Some(BANNERS)
    }

    /// The harness offers sign-in and isn't signed in (a session refused
    /// for it says so too).
    pub(super) fn needs_login(&self) -> bool {
        self.instance().is_some_and(|s| {
            !s.auth.methods.is_empty() && s.auth.status == harness_contracts::AuthStatus::SignedOut
        })
    }

    pub(super) fn render_plan(&mut self, ui: &mut egui::Ui) {
        let plan = self.thread().and_then(|t| t.plan.clone());
        let Some(PlanBody::TodoList { steps }) = plan.map(|p| p.body) else {
            return;
        };
        let done = steps.iter().filter(|s| s.status == StepStatus::Completed).count();
        egui::CollapsingHeader::new(RichText::new(format!("{} Plan ({done}/{})", icon::CLIPBOARD_TEXT, steps.len())).size(crate::theme::look().text_body))
            .default_open(true)
            .show(ui, |ui| render_steps(ui, &steps));
    }

    pub(super) fn render_stream(&mut self, ui: &mut egui::Ui) {
        let view = self.thread().map(|t| t.clone());
        // No session yet: its banner says so.
        let Some(view) = view else { return };
        let items: Vec<&TurnItem> = view.turns.iter().flat_map(|t| view.items_of(&t.id)).collect();
        let rows = rows::rows(&items, &self.ui.folds);
        let mut turn: Option<&harness_contracts::TurnId> = None;
        for row in &rows {
            let item = items[row.first];
            // Each turn under its line (what it is, how it went).
            if turn != Some(&item.turn_id) {
                if turn.is_some() {
                    ui.add(egui::Separator::default().spacing(8.0));
                }
                turn = Some(&item.turn_id);
                if let Some(t) = view.turns.iter().find(|t| t.id == item.turn_id) {
                    self.render_turn_line(ui, t);
                }
            }
            let rect = self.item_frame(ui, Some(row.first), |chat, ui| match &row.run {
                Some(words) => render_run(ui, words, row.last - row.first + 1),
                None => chat.render_item(ui, &view, item, row.first),
            });
            self.ui.item_rects.push(rect);
            // A request waiting on you is a place where it's kept (its
            // card); here, the line saying where it was asked.
            let waiting = matches!(&item.body, ItemBody::Request { request_id } if view.request(request_id).is_some_and(|r| self.agents().waits(r)));
            if !waiting {
                self.ui.drawing.push(Spot { key: row.first.to_string(), rect });
            }
            ui.add_space(crate::theme::look().space_xs);
        }
        if turn.is_some() {
            ui.add(egui::Separator::default().spacing(8.0));
        }
    }

    /// A turn's line: its number, and whether it's queued, running (its
    /// live line), failed, and what it changed.
    fn render_turn_line(&mut self, ui: &mut egui::Ui, turn: &harness_contracts::Turn) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Turn {}", turn.ordinal)).monospace().size(crate::theme::look().text_small).color(colors().faint));
            if turn.status == TurnStatus::Queued {
                ui.label(RichText::new("QUEUED").size(crate::theme::look().text_tiny).color(colors().muted));
            } else if !turn.status.is_terminal() {
                self.render_live(ui);
            } else if let Some(f) = &turn.failure {
                ui.label(RichText::new(format!("failed: {}", f.message)).size(crate::theme::look().text_tiny).color(colors().error));
            }
            if let Some(files) = self.turn_files.get(turn.id.as_str()).filter(|n| **n > 0) {
                let words = if *files == 1 { "1 file changed".to_owned() } else { format!("{files} files changed") };
                ui.label(RichText::new(format!("{} {words}", egui_phosphor::regular::GIT_DIFF)).size(crate::theme::look().text_tiny).color(colors().ok));
            }
        });
    }

    /// The item at `index` in the stream (only it read out).
    pub(super) fn item_at(&self, index: usize) -> Option<TurnItem> {
        let view = self.thread()?;
        view.turns.iter().flat_map(|t| view.items_of(&t.id)).nth(index).cloned()
    }

    /// Every item in the active thread, in the order shown.
    pub(super) fn stream(&self) -> Vec<TurnItem> {
        let Some(view) = self.thread() else { return Vec::new() };
        view.turns.iter().flat_map(|t| view.items_of(&t.id).cloned()).collect()
    }

    /// The cursor on item `index` (a folded run holding it opened, so it's
    /// a row of its own; its run's place when it's the first), shown.
    pub(super) fn put_cursor(&mut self, index: usize, show: crate::viewport::Show) {
        let Some(last) = self.rows().last().map(|r| r.last) else { return };
        let index = index.min(last);
        if let Some(run) = self.rows().into_iter().find(|r| r.run.is_some() && r.holds(index) && r.first != index)
            && let Some(first) = self.item_at(run.first)
        {
            self.ui.folds.toggle_run(&first.id);
        }
        let rows = self.rows();
        let Some(row) = rows.iter().position(|r| r.holds(index)) else { return };
        self.ui.cursor = Some(rows[row].first);
        self.ui.link_pick = None;
        self.ui.scrolling.show(row, show);
    }

    /// Moves by the shared rules over the rows (following the latest: from
    /// the newest).
    pub(super) fn go(&mut self, go: crate::viewport::Go) {
        let rows = self.rows();
        let from = self.ui.cursor.and_then(|c| rows.iter().position(|r| r.holds(c))).or(rows.len().checked_sub(1));
        if let Some(at) = self.ui.scrolling.go(rows.len(), from, go) {
            self.ui.cursor = Some(rows[at].first);
            self.ui.link_pick = None;
        }
    }


    pub(super) fn render_item(&mut self, ui: &mut egui::Ui, view: &ThreadView, item: &TurnItem, index: usize) {
        match &item.body {
            // Your messages and the agent's: as every conversation draws them.
            ItemBody::UserMessage { text } => crate::conversation::message(ui, &mut self.ui.markdown_cache, &crate::conversation::Message::new("user", text)),
            ItemBody::AssistantMessage { content, .. } => {
                let text = item.body.said().unwrap_or_default();
                let by = self.slot().map(|s| s.name.clone()).unwrap_or_else(|| "the agent".into());
                let under = content.iter().filter(|b| !matches!(b, ContentBlock::Text { .. })).map(block_summary).collect();
                crate::conversation::message(ui, &mut self.ui.markdown_cache, &crate::conversation::Message { markdown: true, under, ..crate::conversation::Message::new(&by, &text) });
            }
            // Reasoning, a call's output: folded unless opened (the fold
            // keys, or its control under it while you're on it).
            ItemBody::Reasoning { text, streaming } => {
                let open = self.ui.folds.item_open(item);
                let label = if *streaming { format!("{} Thinking… ({} chars)", icon::BRAIN, text.len()) } else { format!("{} Reasoning ({} chars)", icon::BRAIN, text.len()) };
                ui.label(RichText::new(format!("{} {label}", fold_mark(open))).size(crate::theme::look().text_body).color(colors().special));
                if open {
                    ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(colors().muted));
                }
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
                    let open = self.ui.folds.item_open(item);
                    ui.label(RichText::new(format!("{} output", fold_mark(open))).size(crate::theme::look().text_small).color(colors().muted));
                    if open {
                        for block in output {
                            ui.label(RichText::new(block_detail(block)).monospace().size(crate::theme::look().text_small).color(colors().muted));
                        }
                    }
                }
            }
            ItemBody::FileChange { path, .. } => {
                let link = ui.label(RichText::new(format!("{} {path}", icon::PENCIL_SIMPLE)).monospace().size(crate::theme::look().text_body).color(colors().info));
                self.ui.drawing.push(Spot { key: format!("{index}:0"), rect: link.rect });
            }
            ItemBody::CommandExecution { command, output, exit_code } => {
                ui.label(RichText::new(format!("$ {command}")).monospace().size(crate::theme::look().text_body));
                if let Some(out) = output {
                    let open = self.ui.folds.item_open(item);
                    ui.label(RichText::new(format!("{} output", fold_mark(open))).size(crate::theme::look().text_small).color(colors().muted));
                    if open {
                        ui.label(RichText::new(out).monospace().size(crate::theme::look().text_small).color(colors().muted));
                    }
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
            // Waiting on you: kept in sight under the stream (this says it
            // was asked here); answered: what was asked, and the answer.
            ItemBody::Request { request_id } => {
                if let Some(request) = view.request(request_id) {
                    if self.agents().waits(request) {
                        let line = format!("{} {} · waiting on you, below", icon::HOURGLASS_MEDIUM, request.title);
                        ui.add(egui::Label::new(RichText::new(line).size(crate::theme::look().text_body).color(colors().warn)).truncate());
                    } else {
                        render_request(ui, request, &mut self.drafts);
                    }
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

    /// Its text field: it has the keyboard while `typing` (its host says).
    pub(super) fn render_input(&mut self, ui: &mut egui::Ui, typing: bool, cx: &mut RunCx<'_>) {
        let at = cx.at;
        let input_mode = &mut *cx.input_mode;

        ui.horizontal(|ui| {
            let inserting = typing;
            let steerable = self.steerable();
            let hint = if let Some(takes) = self.sign_in_box() {
                // Its key is the key bar's to say (views say no keys).
                format!("Paste {} here and send it: it goes to the sign-in", takes.waiting.expects.unwrap_or_default())
            } else if inserting && steerable {
                "While it works: sending queues it next; adding puts it into what it's doing".to_owned()
            } else if inserting {
                "Instructions… (/ for commands)".to_owned()
            } else {
                "Type a prompt…".to_owned()
            };
            let typed = self.ui.input.draw(ui, egui::Id::new(("chat_persistent_input_text_edit", at)), 2, &hint, inserting);
            if typed.clicked {
                *input_mode = crate::keymap::InputMode::Insert { at };
            }
            // A trigger where a word starts: its picker, for what follows.
            if let Some(c) = typed.trigger {
                self.asks.push(Ask::Trigger(c));
            }
            // Its Send (and Add) are actions, under it while it's typed in.
            self.ui.typing = inserting.then_some(typed.rect);
            if let Some(sent) = typed.sent {
                self.send_typed(sent == crate::keymap::Sent::Add, cx.note);
            }
        });
    }
}

impl crate::hosted::Hosted for ChatStreamState {
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
        if self.ui.cursor.is_some() {
            return None;
        }
        self.pending_item().map(|i| i.to_string()).or_else(|| self.banners_shown().then(|| BANNERS.to_owned()))
    }

    fn drawn_at(&self, place: &str) -> Option<Rect> {
        self.ui.banners.filter(|_| place == BANNERS)
    }

    fn busy(&self) -> Option<String> {
        (self.active_thread().is_none() && self.opening()).then(|| "Connecting".into())
    }

    /// Its harness, who it's signed in as, what of its configuration
    /// couldn't be read, its session, its tool servers and how much of its
    /// context is used.
    fn status(&self) -> Vec<Status> {
        let Some(s) = self.instance() else { return vec![Status::new("No harness", Tone::Plain)] };
        let mut out = vec![Status::new(s.display_name.clone(), Tone::Plain)];
        if let Some(account) = &s.auth.account {
            let detail = [account.email.clone(), account.organization.clone(), account.plan.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let status = Status::new(format!("{} {}", icon::USER_CIRCLE, account.label), Tone::Muted);
            out.push(if detail.is_empty() { status } else { status.detail(detail) });
        }
        match s.auth.status {
            harness_contracts::AuthStatus::SignedOut if !s.auth.methods.is_empty() => out.push(Status::new(format!("{} Not signed in", icon::KEY), Tone::Warn)),
            harness_contracts::AuthStatus::SignedIn if s.auth.account.is_none() => out.push(Status::new(format!("{} Signed in", icon::KEY), Tone::Muted)),
            _ => {}
        }
        if let Some(home) = &s.home {
            let whose = if home.kept { "set for this project" } else { "its own" };
            out.push(Status::new(icon::HOUSE, Tone::Muted).detail(format!("Its home (sign-in, settings, sessions), {whose}: {}", home.path)));
        }
        if !s.problems.is_empty() {
            out.push(Status::new(icon::WARNING, Tone::Warn).detail(s.problems.join("\n")));
        }
        drop(s);
        if let Some(thread) = &self.active_thread() {
            out.push(Status::new(format!("[{}]", thread.as_str().chars().take(8).collect::<String>()), Tone::Info));
        }
        if let Some(servers) = self.thread().map(|v| v.thread.mcp_servers.clone()).filter(|s| !s.is_empty()) {
            let names: Vec<String> = servers.iter().map(mcp_label).collect();
            out.push(Status::new(format!("{} MCP {}", icon::PLUGS_CONNECTED, servers.len()), Tone::Muted).detail(names.join("\n")));
        }
        if let Some(usage) = self.thread().and_then(|t| t.usage.clone())
            && let (Some(used), Some(window)) = (usage.context_used_tokens, usage.context_window_tokens)
        {
            out.push(Status::new(format!("ctx {:.1}k/{:.0}k", used as f64 / 1000.0, window as f64 / 1000.0), Tone::Muted));
        }
        out
    }
}
