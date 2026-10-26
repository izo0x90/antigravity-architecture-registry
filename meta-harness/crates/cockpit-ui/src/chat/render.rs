//! Drawing the chat: controls, banners, the stream, items, the input.

use super::*;
use crate::hosted::{Status, Tone};
use crate::comparer::Compared;

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

/// A call's output block (not a change: that's drawn as its hunks), as
/// text.
fn block_detail(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Resource { uri, text: Some(text) } => format!("{uri}\n{}", text.chars().take(600).collect::<String>()),
        other => block_summary(other),
    }
}

/// Where the running turn's line was drawn this frame: its turn, the line,
/// the indicator's spot on it, how far down its turn's rows go.
pub(crate) struct LiveAt {
    pub turn: harness_contracts::TurnId,
    pub line: Rect,
    pub spot: Option<Rect>,
    pub turn_bottom: f32,
    /// The agent at work, as drawn there.
    pub activity: crate::indicator::Activity,
}

/// What a row was drawn as: how many items it covers, whether folded,
/// whether its own fold is open, the latest change to its items, how its
/// request stands (a request's row), the width, the text's size. Drawn as
/// the same, it's as tall as it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Measure {
    span: usize,
    folded: bool,
    open: bool,
    changed: u64,
    asks: Option<(harness_contracts::RequestStatus, bool)>,
    width: f32,
    text: f32,
}

/// What the cursor going to an item shows: the row it starts (a folded
/// run, for its first call), or the item itself (a run holding it opened,
/// its first's too: a search's hit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reveal {
    Row,
    Item,
}

/// A folded run's row as a place: its key after its first item's.
pub(super) const RUN: &str = "run";

/// A file a call changed, as it's shown: compared (its hunks, what they
/// add and remove), on its way, too big to compare here, or written whole
/// (no old text came with it: there's nothing to compare it with).
pub(super) enum Change {
    Compared(std::rc::Rc<Compared>),
    Comparing,
    TooBig,
    Written,
}

/// What a call changed and gave, folded (its fold's mark first): what its
/// changes add and remove, as the Changes view says it ("+12 −3
/// src/a.rs", "+20 −5 in 3 files"), each file written whole or not
/// compared yet said so; and the rest ("output").
fn draw_folded_output(ui: &mut egui::Ui, open: bool, changes: &[(usize, &harness_contracts::Diff, Change)], blocks: usize) {
    let look = crate::theme::look();
    let words = |ui: &mut egui::Ui, text: String| ui.label(RichText::new(text).size(look.text_small).color(colors().muted));
    ui.horizontal_wrapped(|ui| {
        words(ui, crate::nav::fold_mark(open).to_owned());
        let compared: Vec<&Compared> = changes.iter().filter_map(|(.., c)| if let Change::Compared(c) = c { Some(c.as_ref()) } else { None }).collect();
        match changes {
            [] => {}
            [(_, diff, Change::Written)] => {
                words(ui, format!("{} written whole ({} lines)", diff.path, diff.new_text.lines().count()));
            }
            [(_, diff, Change::Comparing)] => {
                words(ui, format!("{} changed", diff.path));
            }
            [(_, diff, Change::TooBig)] => {
                words(ui, format!("{} changed (too big to compare here)", diff.path));
            }
            _ if compared.len() == changes.len() => {
                let (added, removed) = compared.iter().fold((0, 0), |(a, r), c| (a + c.added, r + c.removed));
                crate::diff::draw_counts(ui, added, removed);
                words(ui, match changes {
                    [(_, diff, _)] => diff.path.clone(),
                    _ => format!("in {} files", changes.len()),
                });
            }
            _ => {
                words(ui, format!("{} files changed", changes.len()));
            }
        }
        if changes.is_empty() {
            words(ui, "output".to_owned());
        } else if blocks > changes.len() {
            words(ui, "· output".to_owned());
        }
    });
}

/// A file's change drawn as the Changes view draws one: its name, its
/// hunks (one column; unnumbered: the texts a call gave may be a part of
/// the file), or its text when it was written whole.
fn draw_change(ui: &mut egui::Ui, diff: &harness_contracts::Diff, change: &Change) {
    let look = crate::theme::look();
    ui.label(RichText::new(&diff.path).monospace().size(look.text_small).color(colors().info));
    match change {
        Change::Compared(compared) => {
            for hunk in &compared.hunks {
                crate::diff::draw_hunk(ui, hunk, crate::diff::Numbers::Left);
            }
        }
        Change::Comparing => {
            ui.label(RichText::new("Comparing…").size(look.text_small).color(colors().muted));
        }
        Change::TooBig => {
            ui.label(RichText::new("Too big to compare here: the Changes view shows the turn's changes.").size(look.text_small).color(colors().muted));
        }
        Change::Written => {
            ui.label(RichText::new(&diff.new_text).monospace().size(look.text_small).color(colors().muted));
        }
    }
}

/// The most of the chat's height what waits on you takes (it scrolls past
/// that).
const WAITING_SHARE: f32 = 0.4;

impl ChatStreamState {
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, typing: bool, cx: &mut RunCx<'_>) {
        let host = cx.at.name();
        let ctx = ui.ctx().clone();
        let panel = ui.clip_rect();

        ui.vertical(|ui| {
            let banners = self.render_banners(ui);
            self.render_plan(ui);
            // The stream in what's left above what's under it (what waits
            // on you, the typing box), as tall as that was last frame.
            let whole = ui.available_height();
            let scroll_height = (whole - self.ui.below).max(50.0);
            self.ui.drawing.clear();
            self.ui.item_rects.clear();
            self.ui.live_at = None;
            let area = egui::ScrollArea::vertical().id_salt(("chat_stream", cx.at)).auto_shrink([false, false]).stick_to_bottom(self.ui.cursor.is_none()).max_height(scroll_height);
            // Where the shared rules say (following the latest, nothing's
            // asked: the newest stays in view); each frame's messages are
            // measured either way, for the first move from it.
            let area = self.ui.scrolling.area(area, ui.input(|i| i.time));
            let stream = area.show(ui, |ui| self.render_stream(ui));
            // The colours its code blocks need, asked for.
            self.asks.extend(self.ui.messages.colours_wanted());
            if !self.changes_wanted.is_empty() {
                self.asks.push(Ask::Changes(std::mem::take(&mut self.changes_wanted)));
            }
            // The agent at work, where it shows; each item's stop, in the
            // stream's order.
            let rects = std::mem::take(&mut self.ui.item_rects);
            self.show_live(ui, stream.inner_rect, panel, &rects);
            for rect in rects {
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
                // (At least a card's title and its row of answers.)
                let most = (whole * WAITING_SHARE).max(4.0 * ui.spacing().interact_size.y);
                egui::ScrollArea::vertical().id_salt(("chat_waiting", cx.at)).max_height(most).min_scrolled_height(most).auto_shrink([false, true]).show(ui, |ui| {
                    for (index, request) in &pending {
                        let on = index.is_some() && self.ui.cursor == *index;
                        let rect = self.item_frame(ui, on, 0, |chat, ui| {
                            crate::request_card::render_request(ui, request, &mut chat.drafts);
                        });
                        if let Some(index) = index {
                            self.ui.spots.push(Spot { key: index.to_string(), rect });
                        }
                    }
                });
            }
            // Its sub-agents at work, in sight (each opens over the panels).
            self.render_subagents(ui);
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

    /// What it keeps of the thread it shows, kept true to it (as it's
    /// shared, each frame, before keys): another thread shown, the folds
    /// start afresh; a new view of it (a resync), the rows' heights and
    /// the changes worked out go; items put in before the cursor's (a
    /// sub-agent's), the cursor stays on its item.
    pub(super) fn tend(&mut self) {
        let agents = self.agents.clone();
        let agents = agents.get();
        let Some(view) = agents.thread_in(&self.ui.slot) else { return };
        if self.ui.folds_for.as_ref() != Some(&view.thread.id) {
            if self.ui.folds_for.is_some() {
                self.ui.folds.afresh();
            }
            self.ui.folds_for = Some(view.thread.id.clone());
        }
        let epoch = view.epoch();
        if self.ui.epoch != Some(epoch) {
            self.ui.heights.clear();
            self.ui.epoch = Some(epoch);
        }
        if let (Some(at), Some(id)) = (self.ui.cursor, &self.ui.cursor_on)
            && view.in_stream(at).is_none_or(|item| item.id != *id)
            && let Some(now) = view.shown_at(id)
        {
            self.ui.cursor = Some(now);
        }
        // (Put there before its item came, a place restored: on what's
        // there once the thread's all here, shown.)
        if self.ui.cursor_on.is_none() && agents.store().is_live() {
            self.ui.cursor_on = self.ui.cursor.and_then(|at| Some(view.in_stream(at)?.id.clone()));
            if std::mem::take(&mut self.ui.placing) {
                drop(agents);
                if let Some(row) = self.ui.cursor.and_then(|at| self.shape().row_of(at)) {
                    self.ui.scrolling.show(row, crate::viewport::Show::Middle);
                }
            }
        }
    }

    /// The cursor put at `at` (in the order shown), on the item there;
    /// none: following the latest.
    pub(super) fn cursor_to(&mut self, at: Option<usize>) {
        self.ui.cursor = at;
        self.ui.cursor_on = at.and_then(|at| Some(self.thread()?.in_stream(at)?.id.clone()));
    }

    /// An item as the chat draws it, in the stream or where what waits is
    /// kept: in the column's width (less a step in for each sub-agent it's
    /// within, `depth`), filled while the cursor's on it (`on`). Where it
    /// was drawn.
    fn item_frame(&mut self, ui: &mut egui::Ui, on: bool, depth: usize, draw: impl FnOnce(&mut Self, &mut egui::Ui)) -> Rect {
        let fill = if on { colors().selection } else { Color32::TRANSPARENT };
        let look = crate::theme::look();
        Frame::NONE
            .outer_margin(egui::Margin { left: (depth as f32 * look.space_l).min(f32::from(i8::MAX)) as i8, ..Default::default() })
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

    /// The stream, row by row, each turn under its line. Only the rows in
    /// sight or near it are drawn (following the latest, the last window's
    /// too, wherever the scroll is yet): one out of sight takes the room it
    /// took when it was last drawn (as it was then, or before it changed:
    /// it's drawn again as it comes near), or a line's when it never was.
    /// A row drawn at a new height asks for the next frame (what's in
    /// sight moves with it).
    pub(super) fn render_stream(&mut self, ui: &mut egui::Ui) {
        let agents = self.agents.clone();
        let agents = agents.get();
        // No session yet: its banner says so.
        let Some(view) = agents.thread_in(&self.ui.slot) else { return };
        let shape = self.shape();
        let visible = ui.clip_rect();
        let near = visible.expand2(Vec2::new(0.0, visible.height() / 4.0));
        let (width, text) = (ui.available_width(), crate::theme::look().text_body);
        let unknown = ui.spacing().interact_size.y;
        let height_of = |row: &rows::Row| view.in_stream(row.first).and_then(|i| self.ui.heights.get(&i.id)).map_or(unknown, |(_, h)| *h);
        // (Following the latest: the rows of the last window, from the end.)
        let mut tail = shape.rows.len();
        if self.ui.cursor.is_none() {
            let mut room = visible.height() * 1.25;
            while tail > 0 && room > 0.0 {
                tail -= 1;
                room -= height_of(&shape.rows[tail]);
            }
        }
        let mut moved = false;
        // The running turn's: the agent at work, as of this frame.
        let mut live = view.running().map(|t| (t.id.clone(), self.activity(view, ui.input(|i| i.time))));
        let mut turn: Option<&harness_contracts::TurnId> = None;
        for (at, row) in shape.rows.iter().enumerate() {
            let Some(item) = view.in_stream(row.first) else { continue };
            // Each turn under its line (what it is, how it went).
            if turn != Some(&item.turn_id) {
                if turn.is_some() {
                    ui.add(egui::Separator::default().spacing(crate::theme::look().space_m));
                }
                turn = Some(&item.turn_id);
                if let Some(t) = view.turn(&item.turn_id) {
                    let activity = live.take_if(|(id, _)| *id == t.id).map(|(_, activity)| activity);
                    let (line, spot) = self.render_turn_line(ui, t, activity.as_ref());
                    if let Some(activity) = activity {
                        self.ui.live_at = Some(LiveAt { turn: t.id.clone(), line, spot, turn_bottom: line.bottom(), activity });
                    }
                }
            }
            let opens = !row.folded && (rows::folds(item) || row.nested > 0);
            let asks = match &item.body {
                ItemBody::Request { request_id } => view.request(request_id).map(|r| (r.status, self.agents().waits(r))),
                _ => None,
            };
            let measure = Measure { span: row.last - row.first, folded: row.folded, open: opens && self.ui.folds.item_open(item), changed: (row.first..=row.last).map(|i| view.changed(i)).max().unwrap_or(0), asks, width, text };
            // (As tall as when it was last drawn, as it is now or not.)
            let (height, current) = self.ui.heights.get(&item.id).map_or((unknown, false), |(was, height)| (*height, *was == measure));
            let far = at < tail && !near.intersects(Rect::from_min_size(ui.cursor().min, Vec2::new(width, height)));
            let rect = if far {
                ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover()).0
            } else {
                let on = self.ui.cursor.is_some_and(|c| row.holds(c));
                let rect = self.item_frame(ui, on, row.depth, |chat, ui| if row.folded { chat.render_run(ui, view, row) } else { chat.render_item(ui, view, item, row) });
                if !current || rect.height() != height {
                    moved |= rect.height() != height;
                    self.ui.heights.insert(item.id.clone(), (measure, rect.height()));
                }
                rect
            };
            self.ui.item_rects.push(rect);
            // A request waiting on you is a place where it's kept (its
            // card); here, the line saying where it was asked. (Places only
            // for what's in sight: none else can be labelled.)
            let waiting = asks.is_some_and(|(_, waits)| waits);
            if !waiting && rect.intersects(visible) {
                let key = if row.folded { format!("{}:{RUN}", row.first) } else { row.first.to_string() };
                self.ui.drawing.push(Spot { key, rect });
            }
            if let Some(live) = self.ui.live_at.as_mut().filter(|l| l.turn == item.turn_id) {
                live.turn_bottom = rect.bottom();
            }
            ui.add_space(crate::theme::look().space_xs);
        }
        if moved {
            ui.ctx().request_repaint();
        }
        if turn.is_some() {
            ui.add(egui::Separator::default().spacing(crate::theme::look().space_m));
        }
    }

    /// A folded run: one line saying what it did.
    fn render_run(&self, ui: &mut egui::Ui, view: &ThreadView, row: &rows::Row) {
        let run: Vec<&TurnItem> = (row.first..=row.last).filter_map(|i| view.in_stream(i)).collect();
        let line = format!("{} {}", crate::nav::fold_mark(false), rows::summary(&run));
        let what = rows::Folding::of_run(&run).map(|f| f.words()).unwrap_or_default();
        ui.add(egui::Label::new(RichText::new(line).size(crate::theme::look().text_body).color(colors().muted)).truncate())
            .on_hover_text(format!("{}, folded", crate::nav::capitalized(&what)));
    }

    /// A turn's line: its number, what it changed, and whether it's queued,
    /// failed, or running (`live`: the running one's, its live line;
    /// another not ended, RUNNING), in the column's width. Where it was
    /// drawn; the live line's spot.
    pub(super) fn render_turn_line(&self, ui: &mut egui::Ui, turn: &harness_contracts::Turn, live: Option<&crate::indicator::Activity>) -> (Rect, Option<Rect>) {
        let look = crate::theme::look();
        let mut spot = None;
        let line = ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Turn {}", turn.ordinal)).monospace().size(look.text_small).color(colors().faint));
            if let Some(files) = self.turn_files.get(turn.id.as_str()).filter(|n| **n > 0) {
                let words = if *files == 1 { "1 file changed".to_owned() } else { format!("{files} files changed") };
                ui.label(RichText::new(format!("{} {words}", egui_phosphor::regular::GIT_DIFF)).size(look.text_tiny).color(colors().ok));
            }
            match (turn.status, live) {
                (TurnStatus::Queued, _) => {
                    ui.label(RichText::new("QUEUED").size(look.text_tiny).color(colors().muted));
                }
                (TurnStatus::Running, Some(activity)) => spot = self.render_live(ui, activity),
                (TurnStatus::Running, None) => {
                    ui.label(RichText::new("RUNNING").size(look.text_tiny).color(colors().muted));
                }
                _ => {
                    if let Some(f) = &turn.failure {
                        ui.add(egui::Label::new(RichText::new(format!("failed: {}", f.message)).size(look.text_tiny).color(colors().error)).truncate());
                    }
                }
            }
        });
        (line.response.rect, spot)
    }

    /// The cursor on item `index` (what hides it opened: a folded run
    /// holding it, and the calls whose sub-agents it's within; a run's
    /// first stays its run's row unless `reveal` is the item), shown.
    pub(super) fn put_cursor(&mut self, index: usize, show: crate::viewport::Show, reveal: Reveal) {
        let Some(last) = self.shape().rows.last().map(|r| r.last) else { return };
        let index = index.min(last);
        // (Each pass opens one; one that doesn't open ends it.)
        let mut opened = None;
        loop {
            let shape = self.shape();
            let Some(row) = shape.row_of(index).map(|r| &shape.rows[r]) else { return };
            let shown = row.first == index && (reveal == Reveal::Row || !row.folded);
            if shown || opened == Some((row.first, row.last)) {
                break;
            }
            opened = Some((row.first, row.last));
            let Some(first) = self.thread().and_then(|v| v.in_stream(row.first).map(|i| i.id.clone())) else { return };
            if row.folded {
                self.ui.folds.set_run(&first, rows::Set::Open);
            } else {
                self.ui.folds.set(&rows::Folding::Item { id: first, what: rows::Own::Work }, rows::Set::Open);
            }
            // Laid out anew: shown once it's measured so.
            self.ui.scrolling.relaid();
        }
        let Some(row) = self.shape().row_of(index) else { return };
        self.cursor_to(Some(index));
        self.ui.pick = None;
        self.ui.scrolling.show(row, show);
    }

    /// Moves by the shared rules over the rows (following the latest: from
    /// the newest).
    pub(super) fn go(&mut self, go: crate::viewport::Go) {
        let shape = self.shape();
        let from = self.ui.cursor.and_then(|c| shape.row_of(c)).or(shape.rows.len().checked_sub(1));
        if let Some(at) = self.ui.scrolling.go(shape.rows.len(), from, go) {
            self.cursor_to(Some(shape.rows[at].first));
            self.ui.pick = None;
        }
    }

    /// A file a call changed, as the server compared its two texts (asked
    /// for once it's drawn, when it isn't known yet).
    fn change_of(&mut self, diff: &harness_contracts::Diff) -> Change {
        let Some(old) = diff.old_text.as_deref() else { return Change::Written };
        let context = self.context as u32;
        let found = self.comparer.get().compared(old, &diff.new_text, context);
        match found {
            crate::asked::Found::Known(compared) => Change::Compared(compared),
            crate::asked::Found::TooBig => Change::TooBig,
            crate::asked::Found::Unknown { asked } => {
                if !asked {
                    self.changes_wanted.push(crate::comparer::Pair { old: old.to_owned(), new: diff.new_text.clone(), context });
                }
                Change::Comparing
            }
        }
    }

    /// Item `index`'s files (`places`, its links in order), each a place
    /// of its own (`3:1`): the cursor goes on it (a click), and it opens
    /// (Enter, a double click).
    fn places(&mut self, ui: &mut egui::Ui, index: usize, places: &[(String, Option<usize>)]) {
        for (n, (path, line)) in places.iter().enumerate() {
            let label = match line {
                Some(line) => format!("{} {path}:{line}", icon::FOLDER_OPEN),
                None => format!("{} {path}", icon::FOLDER_OPEN),
            };
            let link = ui.label(RichText::new(label).monospace().size(crate::theme::look().text_small).color(colors().info));
            self.ui.drawing.push(Spot { key: Pick::Link(n).key(index), rect: link.rect });
        }
    }

    /// Item `index`'s code blocks, where they were drawn: places of their
    /// own (`3:c0`).
    fn code_spots(&mut self, index: usize, drawn: &[Rect]) {
        for (n, rect) in drawn.iter().enumerate() {
            self.ui.drawing.push(Spot { key: Pick::Code(n).key(index), rect: *rect });
        }
    }

    /// The item `row` starts with, as its kind draws it.
    pub(super) fn render_item(&mut self, ui: &mut egui::Ui, view: &ThreadView, item: &TurnItem, row: &rows::Row) {
        let index = row.first;
        match &item.body {
            // Your messages and the agent's: as every conversation draws them.
            // Yours with its links as chips (places of their own, Enter goes
            // to what each is), what they handed on with it folded under it.
            ItemBody::UserMessage { text, pieces } => {
                let links = harness_contracts::links_of(pieces);
                let drawn = self.ui.messages.draw(ui, egui::Id::new(&item.id), &crate::conversation::Message { links: &links, ..crate::conversation::Message::new("user", text) });
                for (n, rect) in drawn.chips.iter().enumerate() {
                    self.ui.drawing.push(Spot { key: Pick::Chip(n).key(index), rect: *rect });
                }
                if !pieces.is_empty() {
                    let open = self.ui.folds.item_open(item);
                    ui.label(RichText::new(format!("{} what went with it", crate::nav::fold_mark(open))).size(crate::theme::look().text_small).color(colors().muted));
                    if open {
                        for piece in pieces {
                            ui.label(RichText::new(&piece.text).monospace().size(crate::theme::look().text_small).color(colors().muted));
                        }
                    }
                }
            }
            // Its code blocks are places of their own (a hint picks one; Copy
            // copies it).
            ItemBody::AssistantMessage { content, .. } => {
                let text = item.body.said().unwrap_or_default();
                let by = self.slot().map(|s| s.name.clone()).unwrap_or_else(|| "the agent".into());
                let under = content.iter().filter(|b| !matches!(b, ContentBlock::Text { .. })).map(block_summary).collect();
                let drawn = self.ui.messages.draw(ui, egui::Id::new(&item.id), &crate::conversation::Message { markdown: true, under, ..crate::conversation::Message::new(&by, &text) });
                self.code_spots(index, &drawn.codes);
                // The files its words name, as a call's places are.
                let places = nav::links_of(item);
                if !places.is_empty() {
                    ui.horizontal_wrapped(|ui| self.places(ui, index, &places));
                }
            }
            // Reasoning, a call's output: folded unless opened (the fold
            // keys, or its control under it while you're on it).
            ItemBody::Reasoning { text, streaming } => {
                let open = self.ui.folds.item_open(item);
                let label = if *streaming { format!("{} Thinking… ({} chars)", icon::BRAIN, text.len()) } else { format!("{} Reasoning ({} chars)", icon::BRAIN, text.len()) };
                ui.label(RichText::new(format!("{} {label}", crate::nav::fold_mark(open))).size(crate::theme::look().text_body).color(colors().special));
                if open {
                    ui.label(RichText::new(text).monospace().size(crate::theme::look().text_body).color(colors().muted));
                }
            }
            ItemBody::ToolCall { name, output, .. } => {
                let (tag, color) = status_tag(item.status);
                let title = item.title.clone().unwrap_or_else(|| name.clone());
                let icon = words::of(&item.body).map_or("", |s| s.icon);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format!("{tag} {icon} {title}")).monospace().size(crate::theme::look().text_body).color(color));
                    self.places(ui, index, &nav::links_of(item));
                });
                // Its output (a file it changed, as "+12 −3"), or what its
                // sub-agent did (its items follow, open): folded unless
                // opened.
                let open = self.ui.folds.item_open(item);
                // (Each change: its block's place in the output, its file, as compared.)
                let changes: Vec<(usize, &harness_contracts::Diff, Change)> =
                    output.iter().enumerate().filter_map(|(n, b)| if let ContentBlock::Diff(diff) = b { Some((n, diff)) } else { None }).map(|(n, diff)| (n, diff, self.change_of(diff))).collect();
                match (row.nested, output.is_empty()) {
                    (0, true) => {}
                    (0, false) => draw_folded_output(ui, open, &changes, output.len()),
                    (n, _) => {
                        let said = if n == 1 { "1 step by its sub-agent".to_owned() } else { format!("{n} steps by its sub-agent") };
                        ui.label(RichText::new(format!("{} {said}", crate::nav::fold_mark(open))).size(crate::theme::look().text_small).color(colors().muted));
                    }
                }
                if open {
                    for (n, block) in output.iter().enumerate() {
                        match changes.iter().find(|(at, ..)| *at == n) {
                            Some((_, diff, change)) => draw_change(ui, diff, change),
                            None => {
                                ui.label(RichText::new(block_detail(block)).monospace().size(crate::theme::look().text_small).color(colors().muted));
                            }
                        }
                    }
                }
            }
            // A file changed: its place; "+12 −3", folded unless opened.
            ItemBody::FileChange { diff, .. } => {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(icon::PENCIL_SIMPLE).size(crate::theme::look().text_body).color(colors().info));
                    self.places(ui, index, &nav::links_of(item));
                });
                if let Some(diff) = diff {
                    let change = self.change_of(diff);
                    let open = self.ui.folds.item_open(item);
                    let changes = [(0, diff, change)];
                    draw_folded_output(ui, open, &changes, 1);
                    if open {
                        draw_change(ui, diff, &changes[0].2);
                    }
                }
            }
            ItemBody::CommandExecution { command, output, exit_code } => {
                ui.label(RichText::new(format!("$ {command}")).monospace().size(crate::theme::look().text_body));
                if let Some(out) = output {
                    let open = self.ui.folds.item_open(item);
                    ui.label(RichText::new(format!("{} output", crate::nav::fold_mark(open))).size(crate::theme::look().text_small).color(colors().muted));
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
                    let drawn = self.ui.messages.markdown(ui, egui::Id::new(&item.id), markdown);
                    self.code_spots(index, &drawn);
                    });
            }
            ItemBody::TodoList { steps, .. } => render_steps(ui, steps),
            // Waiting on you: kept in sight under the stream (this says it
            // was asked here); answered: what was asked, and the answer.
            ItemBody::Request { request_id } => {
                if let Some(request) = view.request(request_id) {
                    if self.waits_on_you(view, request_id) {
                        let line = format!("{} {} · waiting on you, below", icon::HOURGLASS_MEDIUM, request.title);
                        ui.add(egui::Label::new(RichText::new(line).size(crate::theme::look().text_body).color(colors().warn)).truncate());
                    } else {
                        render_request(ui, request, &mut self.drafts);
                    }
                }
            }
            // Where a sub-agent started: its name, how it's doing, how many
            // steps of its own (folded under it); Enter opens it.
            ItemBody::Subagent { subagent_id } => {
                let look = crate::theme::look();
                let (title, state, colour) = match view.subagent(subagent_id) {
                    Some(seen) => {
                        let (state, colour) = subagents::state_words(seen.subagent.status);
                        (seen.subagent.title.clone(), state, colour)
                    }
                    None => (item.title.clone().unwrap_or_else(|| "sub-agent".into()), "", colors().muted),
                };
                let steps = view.under(&item.id).count();
                let steps = match steps {
                    0 => String::new(),
                    1 => " · 1 step".to_owned(),
                    n => format!(" · {n} steps"),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} {title}", icon::ROBOT)).size(look.text_body).color(colors().text_strong));
                    ui.label(RichText::new(format!("{} {state}{steps}", icon::CIRCLE)).size(look.text_small).color(colour));
                });
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
        // Its sub-agents at work: seen here while it isn't focused.
        let working = match self.subagents_working() {
            0 => None,
            n => Some(Status::new(format!("{} {n}", icon::ROBOT), Tone::Info).detail(format!("Sub-agents working: {n}"))),
        };
        let Some(s) = self.instance() else { return [Status::new("No harness", Tone::Plain)].into_iter().chain(working).collect() };
        let mut out = vec![Status::new(s.display_name.clone(), Tone::Plain)];
        out.extend(working);
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
