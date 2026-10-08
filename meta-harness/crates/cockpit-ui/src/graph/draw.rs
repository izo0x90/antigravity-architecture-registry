//! The canvas: the tree as the server laid it out, through a camera that
//! only moves and zooms. Only what's on screen is drawn. Cards show their
//! lines of text (the same the server sized them for); lines show their
//! plan mark; the selection's lines show their pills; what isn't tied to
//! the selection dims.

use std::collections::BTreeMap;

use egui::{Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};

use arch_graph::tree::{Card, Line as TreeLine, LineKind, Rect as WorldRect, ViewKind, Way, card_lines, count_text};

use super::{Camera, GraphState, Load, NavChange};
use crate::actions::{Action, RunCx, ToolbarStyle, ToolbarUse, toolbar_control};
use crate::feedback::FeedbackInspectorState;
use crate::graph_config::ChipMode;
use crate::graph_style::{LegendEntry, Swatch, style};
use crate::nav::Navigable;
use crate::theme::colors;
use crate::touch::{Gesture, Swipe};

/// How far out and in the camera zooms.
const MIN_ZOOM: f32 = 0.03;
const MAX_ZOOM: f32 = 4.0;
/// How much of a wide graph slot the details card takes, beside the canvas.
const DRAWER_SHARE: f32 = 0.3;
/// How much of a narrow canvas the details sheet covers, from the bottom.
const SHEET_SHARE: f32 = 0.34;

impl GraphState {
    pub(crate) fn to_screen(&self, r: &WorldRect) -> Rect {
        let z = self.camera.zoom;
        Rect::from_min_size(self.canvas.min + self.camera.pan + Vec2::new(r.x, r.y) * z, Vec2::new(r.w, r.h) * z)
    }

    fn world_to_screen(&self, (x, y): (f32, f32)) -> Pos2 {
        self.canvas.min + self.camera.pan + Vec2::new(x, y) * self.camera.zoom
    }

    pub(crate) fn zoom_by(&mut self, factor: f32, around: Pos2) {
        let old = self.camera.zoom;
        let new = (old * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        let world = (around - self.canvas.min - self.camera.pan) / old;
        self.camera.zoom = new;
        self.camera.pan = around - self.canvas.min - world * new;
    }

    /// The whole tree in the open part of the canvas, never smaller than
    /// readable: past that, readable size from the selection (or the top).
    pub(crate) fn fit_view(&mut self) {
        let Some(tree) = &self.tree else { return };
        let (w, h) = (tree.size.0.max(1.0), tree.size.1.max(1.0));
        let area = self.open_area;
        let pad = crate::theme::look().space_xxxl;
        let zoom = ((area.width() - pad * 2.0) / w).min((area.height() - pad * 2.0) / h).clamp(MIN_ZOOM, 1.0);
        if zoom >= self.ui.readable_zoom {
            self.camera.zoom = zoom;
            self.camera.pan = (area.min - self.canvas.min) + (area.size() - Vec2::new(w, h) * zoom) / 2.0;
            return;
        }
        // A tree reads from the top: unless something below its roots is
        // selected, start there.
        let Some(focus) = self.selected_card().filter(|c| c.level > 0).map(|c| c.rect) else {
            self.start_at_the_top();
            return;
        };
        let zoom = self.ui.readable_zoom;
        self.camera.zoom = zoom;
        // The selection's left edge a little in from the left, centred up and down.
        let at = Vec2::new(focus.x, focus.y + focus.h / 2.0) * zoom;
        self.camera.pan = (area.min - self.canvas.min) + Vec2::new(pad, area.height() / 2.0) - at;
    }

    /// Readable size, the drawing's top-left corner at the open area's.
    fn start_at_the_top(&mut self) {
        let pad = crate::theme::look().space_xxxl;
        self.camera.zoom = self.ui.readable_zoom;
        self.camera.pan = (self.open_area.min - self.canvas.min) + Vec2::splat(pad);
    }

    /// Pans just enough to show `key` (when it isn't already).
    pub(crate) fn bring_into_view(&mut self, key: &str) {
        let Some(card) = self.card(key) else { return };
        let rect = self.to_screen(&card.rect);
        if self.open_area.contains_rect(rect) {
            return;
        }
        // Bigger than the screen (a box): its top-left corner; else centred.
        if rect.width() > self.open_area.width() || rect.height() > self.open_area.height() {
            let pad = crate::theme::look().space_xxxl;
            self.camera.pan += (self.open_area.min + Vec2::splat(pad)) - rect.min;
        } else {
            self.camera.pan += self.open_area.center() - rect.center();
        }
    }

    /// The card under `pos`: the smallest (a box's part before the box).
    pub(crate) fn hit(&self, pos: Pos2) -> Option<String> {
        let tree = self.tree.as_ref()?;
        tree.cards.iter().map(|c| (self.to_screen(&c.rect), &c.key)).filter(|(r, _)| r.contains(pos)).min_by(|a, b| a.0.area().total_cmp(&b.0.area())).map(|(_, k)| k.clone())
    }

    /// The "+N" count under `pos` (opening its card loads what it counts).
    fn hit_more(&self, pos: Pos2) -> Option<String> {
        self.more_rects.iter().find(|(_, r)| r.contains(pos)).map(|(k, _)| k.clone())
    }

    pub fn render(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, slot_idx: usize, pan_held: bool, cx: &mut RunCx<'_>) {
        self.tick(cx.net, cx.ctx);
        self.render_toolbar(ui, cx);
        let area = ui.available_rect_before_wrap();
        let narrow = area.width() < self.ui.chips.narrow_below;
        let has_drawer = self.drawer_open && self.selected_card().is_some();
        let drawer_w = if has_drawer && !narrow { (area.width() * DRAWER_SHARE).clamp(220.0, 360.0) } else { 0.0 };
        let canvas = Rect::from_min_max(area.min, Pos2::new(area.max.x - drawer_w, area.max.y));
        self.canvas = canvas;
        let sheet_h = if has_drawer && narrow { canvas.height() * SHEET_SHARE } else { 0.0 };
        self.open_area = Rect::from_min_max(canvas.min, Pos2::new(canvas.max.x, canvas.max.y - sheet_h));
        feedback.register_inspectable(ui, &format!("slot_{}:graph:canvas", slot_idx + 1), canvas);
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, colors().bg);
        self.draw_grid(&painter);
        let time = ui.input(|i| i.time);
        if self.fit && self.tree.is_some() {
            self.fit = false;
            self.fit_view();
        }
        if let Some(key) = self.reveal_after_fit.take() {
            self.bring_into_view(&key);
        }
        let touched = self.handle_touch(ui);
        if !touched {
            if response.dragged() {
                self.camera.pan += response.drag_delta();
            }
            if pan_held && response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
            if response.hovered() {
                let (scroll, pinch, pointer) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
                let around = pointer.unwrap_or(canvas.center());
                if pinch != 1.0 {
                    self.zoom_by(pinch, around);
                } else if scroll != 0.0 {
                    self.zoom_by((1.0 + scroll * 0.0015).clamp(0.8, 1.25), around);
                }
            }
        }
        let look = crate::theme::look();
        match (&self.tree, &self.load) {
            (None, Load::Failed(err)) => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, format!("Couldn't load the view: {err}"), FontId::proportional(look.text_title), colors().error);
            }
            (None, _) => {
                painter.text(canvas.center(), Align2::CENTER_CENTER, "Loading the architecture…", FontId::proportional(look.text_title), colors().muted);
            }
            (Some(tree), _) if tree.cards.is_empty() => {
                let why = match (tree.request.view, &tree.request.source) {
                    (_, arch_graph::Source::Plan(p)) if !tree.request.full_code => format!("Plan {p} has nothing for this view yet. Show it in the full code (g F), or pick another view."),
                    _ => "Nothing to show in this view yet. Read the code in (Space I) or pick another view.".to_owned(),
                };
                painter.text(canvas.center(), Align2::CENTER_CENTER, why, FontId::proportional(look.text_title), colors().muted);
            }
            (Some(_), _) => {
                let (moving, more) = self.draw_tree(&painter, time);
                self.more_rects = more;
                if moving {
                    ui.ctx().request_repaint();
                }
            }
        }
        if let Load::Failed(err) = &self.load
            && self.tree.is_some()
        {
            painter.text(canvas.left_bottom() + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, format!("refresh failed: {err}"), FontId::proportional(look.text_body), colors().error);
        }
        // Click: a count opens its card; a card selects; a double click goes in.
        if !touched
            && (response.clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let Some(key) = self.hit_more(pos) {
                self.navigate(NavChange::Open(key));
            } else {
                let hit = self.hit(pos);
                if response.double_clicked()
                    && let Some(key) = hit.clone()
                {
                    self.pointer_jump(NavChange::GoIn(key));
                }
                self.select(hit);
            }
        }
        if self.legend_open {
            self.draw_legend(&painter);
        }
        self.render_chips(ui, cx);
        if has_drawer {
            let drawer = if narrow { Rect::from_min_max(Pos2::new(canvas.min.x, canvas.max.y - sheet_h), canvas.max) } else { Rect::from_min_max(Pos2::new(canvas.max.x, area.min.y), area.max) };
            ui.scope_builder(egui::UiBuilder::new().max_rect(drawer.shrink(6.0)), |ui| self.render_drawer(ui));
        }
    }

    /// Feeds this frame's finger events to the gesture recognizer and acts on
    /// what it sees. Returns whether fingers were involved this frame.
    fn handle_touch(&mut self, ui: &egui::Ui) -> bool {
        let (events, time) = ui.input(|i| {
            let touches: Vec<(u64, crate::touch::Phase, Pos2)> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Touch { id, phase, pos, .. } => Some((id.0, crate::touch::Phase::from(*phase), *pos)),
                    _ => None,
                })
                .collect();
            (touches, i.time)
        });
        let cfg = self.ui.touch.clone();
        let mut gestures = Vec::new();
        for (id, phase, pos) in &events {
            // Fingers that start on a chip belong to the chip.
            if *phase == crate::touch::Phase::Start && (self.chip_rects.iter().any(|(_, r)| r.contains(*pos)) || !self.canvas.contains(*pos)) {
                continue;
            }
            gestures.extend(self.gestures.feed(*id, *phase, *pos, time, &cfg));
        }
        gestures.extend(self.gestures.tick(time, &cfg));
        if let Some(due) = self.gestures.long_press_due(&cfg) {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((due - time).max(0.0)));
        }
        let involved = !events.is_empty() || self.gestures.active();
        if involved {
            self.touch_seen = true;
        }
        for gesture in gestures {
            self.on_gesture(gesture);
        }
        involved
    }

    /// What each gesture does: a tap selects (or opens a count), a double
    /// tap goes in, a long press offers the card's actions, two fingers
    /// pan and zoom, a swipe walks the jump list.
    pub fn on_gesture(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Tap(pos) => {
                self.close_context();
                match self.hit_more(pos) {
                    Some(key) => self.navigate(NavChange::Open(key)),
                    None => {
                        let hit = self.hit(pos);
                        self.select(hit);
                    }
                }
            }
            Gesture::DoubleTap(pos) => match self.hit(pos) {
                Some(key) => {
                    self.select(Some(key.clone()));
                    self.pointer_jump(NavChange::GoIn(key));
                }
                None => self.fit_view(),
            },
            Gesture::LongPress(pos) => {
                if let Some(key) = self.hit(pos) {
                    self.select(Some(key.clone()));
                    self.context = Some((key, pos));
                }
            }
            Gesture::Pan(delta) => self.camera.pan += delta,
            Gesture::Pinch { factor, center } => self.zoom_by(factor, center),
            Gesture::Swipe(Swipe::Right) => self.requests.run = Some(("nav.back".into(), None)),
            Gesture::Swipe(Swipe::Left) => self.requests.run = Some(("nav.forward".into(), None)),
        }
    }

    fn chips_shown(&self) -> bool {
        match self.ui.chips.show {
            ChipMode::Always => true,
            ChipMode::Never => false,
            ChipMode::Auto => self.touch_seen || self.canvas.width() < self.ui.chips.narrow_below,
        }
    }

    /// On-screen chips along the canvas top, and a long-pressed card's
    /// actions next to it: the same actions the keys run.
    fn render_chips(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        use crate::actions::ActionProvider;
        self.chip_rects.clear();
        let mut actions = self.actions();
        actions.extend(cx.shell.iter().cloned());
        let size = self.ui.chips.size;
        let mut run: Option<Action> = None;
        if self.chips_shown() {
            let bar = Rect::from_min_max(self.canvas.min + Vec2::splat(6.0), Pos2::new(self.canvas.max.x - 6.0, self.canvas.min.y + size * 2.0 + 12.0));
            let wanted: Vec<&Action> = self.ui.chips.actions.iter().filter_map(|id| actions.iter().find(|a| &a.id == id)).collect();
            ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                ui.horizontal_wrapped(|ui| {
                    for action in wanted {
                        let text = action.glyph().map_or_else(|| action.label.clone(), str::to_owned);
                        let response = ui.add(egui::Button::new(RichText::new(text).size(size * 0.42)).min_size(Vec2::splat(size)).corner_radius(size / 2.0)).on_hover_text(&action.label);
                        self.chip_rects.push((action.id.clone(), response.rect));
                        if response.clicked() {
                            run = Some(action.clone());
                        }
                    }
                });
            });
        }
        self.context_chips = self.chip_rects.len();
        if let Some((_, at)) = self.context.clone() {
            let wanted: Vec<&Action> = self.ui.context.iter().filter_map(|id| actions.iter().find(|a| &a.id == id)).collect();
            let width = (size + 8.0) * 3.0;
            let origin = Pos2::new(at.x.min(self.canvas.max.x - width).max(self.canvas.min.x), (at.y + 12.0).min(self.canvas.max.y - size * 2.0));
            let panel = Rect::from_min_size(origin, Vec2::new(width, (size + 8.0) * wanted.len().div_ceil(3) as f32));
            ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                egui::Frame::NONE.fill(colors().overlay).corner_radius(crate::theme::look().radius_xl).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for action in wanted {
                            let response = ui.add(egui::Button::new(RichText::new(&action.label).size(crate::theme::look().text_label)).min_size(Vec2::new(size, size)));
                            self.chip_rects.push((action.id.clone(), response.rect));
                            if response.clicked() {
                                run = Some(action.clone());
                            }
                        }
                    });
                });
            });
        }
        if let Some(action) = run {
            self.close_context();
            self.use_action(&action, None, cx);
        }
    }

    fn close_context(&mut self) {
        self.context = None;
        self.chip_rects.truncate(self.context_chips);
    }

    /// The toolbar (the graph's controls) and, under it, where the graph is:
    /// the source, the view and way, the roots gone through (each a way
    /// back), what holds the root, how deep, what was cut, and anything the
    /// graph has to say.
    fn render_toolbar(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        use crate::actions::ActionProvider;
        let mut actions: Vec<Action> = cx
            .shell
            .iter()
            .filter_map(|a| match a.id.as_str() {
                "nav.back" => Some(a.clone().toolbar(ToolbarStyle::Button, "", Some(egui_phosphor::regular::ARROW_LEFT))),
                "nav.forward" => Some(a.clone().toolbar(ToolbarStyle::Button, "", Some(egui_phosphor::regular::ARROW_RIGHT))),
                _ => None,
            })
            .collect();
        actions.extend(self.actions());
        let mut used: Option<(Action, ToolbarUse)> = None;
        ui.horizontal_wrapped(|ui| {
            for action in actions.iter().filter(|a| a.toolbar.is_some()) {
                if let Some(u) = toolbar_control(ui, action) {
                    used = Some((action.clone(), u));
                }
            }
            if self.load == Load::Loading {
                ui.label(RichText::new(egui_phosphor::regular::HOURGLASS).color(colors().warn));
            }
        });
        enum Go {
            Back(usize),
            Into(String),
            Top,
        }
        let mut go: Option<Go> = None;
        ui.horizontal_wrapped(|ui| {
            let look = crate::theme::look();
            let muted = |t: String| RichText::new(t).size(look.text_body).color(colors().muted);
            let source = self.nav.plan.as_ref().map_or("code".to_owned(), |p| if self.nav.full_code { format!("plan {p} in the full code") } else { format!("plan {p}") });
            let way = match (self.nav.view, self.nav.way) {
                (ViewKind::Contains, Way::Down) => "what's inside",
                (ViewKind::Contains, Way::Up) => "what holds it",
                (ViewKind::Calls, Way::Down) => "what it calls",
                (ViewKind::Calls, Way::Up) => "what calls it",
                (ViewKind::DataFlow, Way::Down) => "where it goes",
                (ViewKind::DataFlow, Way::Up) => "where it came from",
            };
            ui.label(muted(format!("{source} · {way} ·")));
            let name = |key: &str| self.card(key).map_or_else(|| key.rsplit([':', '.']).next().unwrap_or(key).to_owned(), |c| c.label.clone());
            // The roots gone through, each a way back; then the root.
            if ui.link(RichText::new("start").size(look.text_body)).on_hover_text("The view's starting points").clicked() {
                go = Some(Go::Top);
            }
            for (i, root) in self.trail_roots().iter().enumerate() {
                if let Some(root) = root {
                    ui.label(muted("›".into()));
                    if ui.link(RichText::new(name(root)).size(look.text_body)).clicked() {
                        go = Some(Go::Back(i));
                    }
                }
            }
            if let Some(root) = &self.nav.root {
                ui.label(muted("›".into()));
                ui.label(RichText::new(name(root)).size(look.text_body).strong().color(colors().text_strong));
            }
            if let Some(tree) = &self.tree {
                if !tree.path.is_empty() {
                    ui.label(muted("· inside".into()));
                    for crumb in &tree.path {
                        if ui.link(RichText::new(&crumb.label).size(look.text_body)).on_hover_text("Show this box").clicked() {
                            go = Some(Go::Into(crumb.key.clone()));
                        }
                    }
                }
                ui.label(muted(format!("· {} levels · {} cards", self.depth(), tree.cards.len())));
                if let Some(cut) = &tree.cut {
                    let mut text = match (cut.shown < cut.asked, cut.starts_left_out) {
                        (true, _) => format!("showing {} of {} levels: {} more", cut.shown, cut.asked, cut.hidden),
                        (false, n) => format!("{} of {} starting points", tree.roots.len(), tree.roots.len() as u32 + n),
                    };
                    if cut.reason == arch_graph::tree::CutReason::Time {
                        text.push_str(" (out of time)");
                    }
                    let (glyph, _, color) = crate::status::look("stale");
                    ui.label(RichText::new(format!("{glyph} {text}")).size(look.text_body).color(color)).on_hover_text("Go into a card, or show fewer levels (-)");
                }
                if let Some(ms) = tree.slow_ms {
                    ui.label(RichText::new(format!("· slow: {ms} ms")).size(look.text_body).color(colors().warn))
                        .on_hover_text("The server took longer than its limit to build this. Each stage's time is at /api/perf.");
                }
            }
            if !self.stale_files.is_empty() {
                let count = self.stale_files.len().to_string();
                crate::status::chip(ui, "stale", Some(&count)).on_hover_text(self.stale_files.join("\n"));
                if ui.small_button("Re-read").on_hover_text("Read the code that changed again (SPC I r)").clicked() {
                    self.requests.reindex = true;
                }
            }
            if let Some(agent) = &self.agent {
                ui.label(RichText::new(format!("{} the agent moved the view: {}", egui_phosphor::regular::ROBOT, agent.what)).size(look.text_body).color(colors().special));
                if let Some(back) = actions.iter().find(|a| a.id == "graph.agent_return")
                    && ui.small_button(format!("{} Return", egui_phosphor::regular::ARROW_U_UP_LEFT)).on_hover_text(&back.label).clicked()
                {
                    used = Some((back.clone(), ToolbarUse::Run(None)));
                }
            }
            if let Some(note) = &self.note {
                ui.label(RichText::new(note).size(look.text_body).color(colors().warn));
            }
        });
        ui.separator();
        match go {
            Some(Go::Back(i)) => {
                if self.jumped.is_none() {
                    self.jumped = Some(self.place());
                }
                self.back_to(i);
            }
            Some(Go::Into(key)) => self.pointer_jump(NavChange::View { view: ViewKind::Contains, root: Some(key) }),
            Some(Go::Top) => self.pointer_jump(NavChange::Top),
            None => {}
        }
        match used {
            Some((action, ToolbarUse::Run(value))) => self.use_action(&action, value, cx),
            Some((action, ToolbarUse::OpenPalette)) => self.palette_request = Some(action.id.clone()),
            None => {}
        }
    }

    /// A faint grid under the graph, moving and scaling with it.
    fn draw_grid(&self, painter: &egui::Painter) {
        let look = crate::theme::look();
        if look.grid_step <= 0.0 {
            return;
        }
        let mut step = look.grid_step * self.camera.zoom;
        while step < look.grid_step / 2.0 {
            step *= 2.0;
        }
        let origin = self.canvas.min + self.camera.pan;
        let stroke = Stroke::new(look.line_thin, colors().grid);
        let mut x = self.canvas.left() + (origin.x - self.canvas.left()).rem_euclid(step);
        while x < self.canvas.right() {
            painter.line_segment([Pos2::new(x, self.canvas.top()), Pos2::new(x, self.canvas.bottom())], stroke);
            x += step;
        }
        let mut y = self.canvas.top() + (origin.y - self.canvas.top()).rem_euclid(step);
        while y < self.canvas.bottom() {
            painter.line_segment([Pos2::new(self.canvas.left(), y), Pos2::new(self.canvas.right(), y)], stroke);
            y += step;
        }
    }

    /// Whether anything of `rect` (screen) is on the canvas.
    fn visible(&self, rect: Rect) -> bool {
        rect.intersects(self.canvas)
    }

    /// The cards (and box frames) on screen, in drawing order (boxes before
    /// what's inside them).
    pub(crate) fn on_screen(&self) -> Vec<&Card> {
        let Some(tree) = &self.tree else { return Vec::new() };
        tree.cards.iter().filter(|c| self.visible(self.to_screen(&c.rect))).collect()
    }

    /// Draws the tree; returns whether something animates (the agent's
    /// pulse) and where the "+N" counts are.
    fn draw_tree(&self, painter: &egui::Painter, time: f64) -> (bool, Vec<(String, Rect)>) {
        let mut more = Vec::new();
        let Some(tree) = &self.tree else { return (false, more) };
        let related = self.related();
        let mut moving = false;
        // Lines under cards; the selection's on top of the rest.
        let (lit, rest): (Vec<&TreeLine>, Vec<&TreeLine>) = tree.lines.iter().partition(|l| self.touches_selection(&l.from, &l.to));
        for line in rest.iter().chain(lit.iter()) {
            self.draw_line(painter, line, tree.request.way, related.is_some());
        }
        // A card's lines to cards drawn further away: one stub, with how many.
        let mut stubs: BTreeMap<&str, (Pos2, usize)> = BTreeMap::new();
        for line in tree.lines.iter().filter(|l| !l.reach.is_empty()) {
            if let Some(end) = line.path.last() {
                stubs.entry(line.from.as_str()).or_insert((self.world_to_screen(*end), 0)).1 += 1;
            }
        }
        for (_, (at, n)) in stubs {
            if self.visible(Rect::from_center_size(at, Vec2::splat(4.0))) && self.camera.zoom * crate::theme::look().text_small >= self.ui.min_text_px {
                count_pill(painter, at + Vec2::new(4.0 + crate::theme::look().space_m * 4.0, 0.0), &format!("{} {n}", egui_phosphor::regular::ARROW_BEND_UP_RIGHT), colors().muted);
            }
        }
        // Boxes (Contains) outermost first: their parts draw over them.
        let mut cards = self.on_screen();
        if tree.request.view == ViewKind::Contains {
            cards.sort_by_key(|c| c.level);
        }
        for card in cards {
            moving |= self.draw_card(painter, card, tree.request.view, time, &mut more);
            if let Some(related) = &related
                && !related.contains(&card.key)
            {
                let look = crate::theme::look();
                let rect = self.to_screen(&card.rect);
                // A box's frame doesn't hide what's in it.
                if card.open && tree.request.view == ViewKind::Contains {
                    continue;
                }
                painter.rect_filled(rect.expand(look.space_xs), look.radius_l, colors().bg.gamma_multiply(look.dim));
            }
        }
        for line in lit {
            self.draw_pills(painter, line);
        }
        (moving, more)
    }

    /// One card: its outline by plan mark, its text (the lines it was
    /// sized for, at the camera's zoom), what it hides as a "+N" to click,
    /// review badges, other plans that change it, the agent's pointer.
    fn draw_card(&self, painter: &egui::Painter, card: &Card, view: ViewKind, time: f64, more: &mut Vec<(String, Rect)>) -> bool {
        let look = crate::theme::look();
        let (theme, st) = (colors(), style());
        let rect = self.to_screen(&card.rect);
        let selected = self.selection.as_deref() == Some(card.key.as_str());
        let fade = if card.outside_plan { st.nodes.outside_plan_alpha } else { 1.0 };
        let is_box = view == ViewKind::Contains && card.open;
        let fill = if is_box { theme.surface.gamma_multiply(0.6) } else { st.node_fill(&theme, selected) };
        if selected && look.glow_width > 0.0 {
            for i in 1..=4 {
                let t = i as f32 / 4.0;
                painter.rect_stroke(rect.expand(look.glow_width * t), look.radius_l + look.glow_width * t, Stroke::new(look.glow_width / 4.0, theme.focus.gamma_multiply(look.glow_alpha * (1.0 - t))), StrokeKind::Outside);
            }
        }
        painter.rect_filled(rect, look.radius_l, fill.gamma_multiply(fade));
        let outline = st.node_outline(&theme, card.mark, selected);
        let stroke = Stroke::new(outline.width, outline.color.gamma_multiply(fade));
        if outline.dashed {
            let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
            painter.extend(Shape::dashed_line(&corners, stroke, look.space_sm, look.space_s));
        } else {
            painter.rect_stroke(rect, look.radius_l, stroke, StrokeKind::Inside);
        }
        // Text at the camera's zoom, clipped to the card; a box's name in its header.
        if let Some(layout) = self.config.as_ref().map(|c| &c.layout) {
            let z = self.camera.zoom;
            let pad = layout.pad * z;
            let mut y = rect.top() + pad;
            let lines = card_lines(card, view, layout);
            let shown = if is_box { &lines[..1] } else { &lines[..] };
            for (k, (text, size)) in shown.iter().enumerate() {
                let px = size * z;
                if px < self.ui.min_text_px {
                    break;
                }
                let font = FontId::proportional(px);
                let color = (if k == 0 { theme.text_strong } else { theme.muted }).gamma_multiply(fade);
                if let Some(text) = fit_text(painter, text, &font, rect.width() - pad * 2.0) {
                    painter.with_clip_rect(rect).text(Pos2::new(rect.left() + pad, y), Align2::LEFT_TOP, text, font, color);
                }
                y += px * layout.line_height;
            }
        }
        // What it hides, as a count to click, inside its right edge.
        if !card.open
            && let Some(count) = count_text(card)
            && self.camera.zoom * look.text_small >= self.ui.min_text_px
        {
            let r = count_pill(painter, Pos2::new(rect.right() - look.space_s, rect.center().y), &count, theme.accent);
            more.push((card.key.clone(), r));
        }
        if card.links_left_out > 0 {
            let at = rect.right_bottom() + Vec2::new(-look.space_s, -look.space_m);
            count_pill(painter, at, &format!("{} {}", egui_phosphor::regular::ARROWS_SPLIT, card.links_left_out), theme.muted);
        }
        if !card.other_plans.is_empty() {
            let names: Vec<&str> = card.other_plans.iter().map(|p| p.0.as_str()).collect();
            let galley = painter.layout_no_wrap(format!("also {}", names.join(", ")), FontId::proportional(look.text_tiny), theme.on_bright);
            let chip = Rect::from_min_size(rect.left_top() + Vec2::new(look.space_s, -galley.size().y / 2.0 - look.space_hair), galley.size() + Vec2::new(look.space_s * 2.0, look.space_hair * 2.0));
            painter.rect_filled(chip, look.radius_pill, theme.special);
            painter.galley(chip.min + Vec2::new(look.space_s, look.space_hair), galley, theme.on_bright);
        }
        if let Some(id) = &card.component {
            self.draw_marks(painter, rect, id);
        }
        if self.card_stale(card) {
            let (glyph, _, color) = crate::status::look("stale");
            painter.text(rect.right_top() + Vec2::new(-look.space_m, look.space_xs), Align2::RIGHT_TOP, glyph, FontId::proportional(look.text_small), color);
        }
        // Where the agent points: a ring, and a pulse going out from it.
        let pointed = self.agent.as_ref().and_then(|a| a.select.as_deref()) == Some(card.key.as_str());
        if pointed {
            let agent = st.agent_color(&theme);
            painter.rect_stroke(rect.expand(look.space_s), look.radius_xl, Stroke::new(look.line_bold, agent), StrokeKind::Outside);
            if look.pulse_seconds > 0.0 {
                let beat = (time as f32 % look.pulse_seconds) / look.pulse_seconds;
                let reach = look.space_s + look.glow_width * 2.0 * beat;
                painter.rect_stroke(rect.expand(reach), look.radius_xl + reach, Stroke::new(look.line_thin, agent.gamma_multiply(1.0 - beat)), StrokeKind::Outside);
            }
        }
        pointed
    }

    /// Whether a card's file changed since it was indexed.
    fn card_stale(&self, card: &Card) -> bool {
        let file = card.detail.split(':').next().unwrap_or_default();
        !file.is_empty() && self.stale_files.iter().any(|s| s == file || file.strip_prefix(s.as_str()).is_some_and(|rest| rest.starts_with('/')))
    }

    /// Review badges at the card's top right: a dot when it changed since the
    /// last review, a count of unaddressed comments.
    fn draw_marks(&self, painter: &egui::Painter, rect: Rect, id: &arch_graph::ComponentId) {
        let Some(mark) = self.marks.get(id) else { return };
        let z = self.camera.zoom.clamp(0.4, 1.0);
        let mut at = rect.right_top() + Vec2::new(-8.0, 8.0) * z;
        if mark.open > 0 {
            let galley = painter.layout_no_wrap(format!("{}", mark.open), FontId::proportional(crate::theme::look().text_small * z), colors().on_bright);
            let chip = Rect::from_center_size(at, galley.size() + Vec2::new(8.0, 2.0) * z);
            painter.rect_filled(chip, chip.height() / 2.0, colors().warn);
            painter.galley(chip.center() - galley.size() / 2.0, galley, colors().on_bright);
            at.x = chip.left() - 6.0 * z;
        }
        if mark.changed {
            painter.circle_filled(at, 4.0 * z, colors().info);
        }
    }

    /// One line: its stroke by plan mark (or as a line back), dimmed when a
    /// selection doesn't touch it, an arrow head where it points.
    fn draw_line(&self, painter: &egui::Painter, line: &TreeLine, way: Way, selecting: bool) {
        let lit = self.touches_selection(&line.from, &line.to);
        // A line to a card drawn further away: its stub, or (an end selected) its whole way.
        let shown = if lit && !line.reach.is_empty() { &line.reach } else { &line.path };
        if shown.len() < 2 {
            return;
        }
        let points: Vec<Pos2> = shown.iter().map(|p| self.world_to_screen(*p)).collect();
        let bounds = Rect::from_points(&points);
        if !self.visible(bounds.expand(4.0)) {
            return;
        }
        let look = crate::theme::look();
        let theme = colors();
        let st = style().line(&theme, line.relation, line.kind, line.mark);
        let color = if selecting && !lit { st.color.gamma_multiply(1.0 - look.dim) } else { st.color };
        let width = st.width * self.camera.zoom.clamp(0.5, 1.5) + if lit { look.line_thin } else { 0.0 };
        let stroke = Stroke::new(width, color);
        if st.dashed {
            painter.extend(Shape::dashed_line(&points, stroke, look.space_sm, look.space_s));
        } else {
            painter.add(Shape::line(points.clone(), stroke));
        }
        // Down, the arrow is at the child (it calls, holds or becomes it);
        // up, the child calls (or makes) the card it hangs from.
        let n = points.len();
        let (tip, before) = match (way, line.kind) {
            (Way::Up, LineKind::Tree) => (points[0], points[1]),
            _ => (points[n - 1], points[n - 2]),
        };
        let dir = (tip - before).normalized();
        let size = look.space_m * self.camera.zoom.clamp(0.5, 1.2);
        let side = Vec2::new(-dir.y, dir.x) * size / 2.0;
        painter.add(Shape::convex_polygon(vec![tip, tip - dir * size + side, tip - dir * size - side], color, Stroke::NONE));
    }

    /// The pills on a selected card's line: what goes in and comes back
    /// (calls), the steps between two data types (data flow).
    fn draw_pills(&self, painter: &egui::Painter, line: &TreeLine) {
        let way = if line.reach.is_empty() { &line.path } else { &line.reach };
        if line.pills.is_empty() || way.len() < 2 {
            return;
        }
        let look = crate::theme::look();
        let theme = colors();
        let points: Vec<Pos2> = way.iter().map(|p| self.world_to_screen(*p)).collect();
        // Stacked just before where the line arrives (each card's own pills
        // beside it, never piled with its siblings').
        let end = points[points.len() - 1];
        let font = FontId::proportional(look.text_tiny.max(look.text_small * self.camera.zoom.min(1.0)));
        let galleys: Vec<_> = line.pills.iter().map(|p| (painter.layout_no_wrap(p.text.clone(), font.clone(), theme.text_strong), style().pill_color(&theme, p.role))).collect();
        let row = galleys.iter().map(|(g, _)| g.size().y).fold(0.0f32, f32::max) + look.space_hair * 2.0 + look.space_xs;
        let mut y = end.y - row * galleys.len() as f32 / 2.0;
        for (galley, color) in galleys {
            let size = galley.size() + Vec2::new(look.space_s * 2.0, look.space_hair * 2.0);
            let rect = Rect::from_min_size(Pos2::new(end.x - look.space_m - size.x, y), size);
            painter.rect_filled(rect, look.radius_pill, theme.bg);
            painter.rect_stroke(rect, look.radius_pill, Stroke::new(look.line_thin, color), StrokeKind::Inside);
            painter.galley(rect.min + Vec2::new(look.space_s, look.space_hair), galley, theme.text_strong);
            y += row;
        }
    }

    /// The legend: what each color, line and outline means, from the style
    /// table, sized to its text and clear of the chips.
    fn draw_legend(&self, painter: &egui::Painter) {
        let theme = colors();
        let entries: Vec<LegendEntry> = style().legend(&theme);
        let (pad, swatch_w, gap) = (10.0, 28.0, 8.0);
        let max_w = (self.canvas.width() - 16.0).clamp(160.0, 400.0);
        let text_w = max_w - pad * 2.0 - swatch_w - gap;
        let font = FontId::proportional(crate::theme::look().text_body);
        let mut rows: Vec<(Option<&str>, Option<&LegendEntry>, std::sync::Arc<egui::Galley>)> = Vec::new();
        let mut section = "";
        for entry in &entries {
            if entry.section != section {
                section = entry.section;
                rows.push((Some(section), None, painter.layout(section.to_owned(), font.clone(), theme.muted, max_w - pad * 2.0)));
            }
            rows.push((None, Some(entry), painter.layout(entry.label.clone(), font.clone(), theme.text, text_w)));
        }
        let width = rows.iter().map(|(head, _, g)| if head.is_some() { g.size().x + pad * 2.0 } else { g.size().x + pad * 2.0 + swatch_w + gap }).fold(0.0f32, f32::max).min(max_w);
        let height: f32 = rows.iter().map(|(_, _, g)| g.size().y.max(14.0) + 4.0).sum::<f32>() + pad * 2.0;
        let mut top = self.canvas.min.y + 8.0;
        let left = self.canvas.max.x - width - 8.0;
        let footprint = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height));
        for (_, chip) in &self.chip_rects {
            if chip.intersects(footprint) {
                top = top.max(chip.max.y + 8.0);
            }
        }
        let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height)).intersect(self.canvas);
        painter.rect_filled(rect, 6.0, theme.overlay);
        let mut y = rect.min.y + pad;
        for (head, entry, galley) in rows {
            let h = galley.size().y.max(14.0);
            match (head, entry) {
                (Some(_), _) => {
                    painter.galley(Pos2::new(rect.min.x + pad, y), galley, theme.muted);
                }
                (None, Some(entry)) => {
                    let swatch = Rect::from_min_size(Pos2::new(rect.min.x + pad, y + 2.0), Vec2::new(swatch_w, 10.0));
                    match entry.swatch {
                        Swatch::Line(line) => {
                            let points = [swatch.left_center(), swatch.right_center()];
                            if line.dashed {
                                painter.extend(Shape::dashed_line(&points, Stroke::new(line.width, line.color), 5.0, 3.0));
                            } else {
                                painter.line_segment(points, Stroke::new(line.width, line.color));
                            }
                        }
                        Swatch::Outline(line) => {
                            painter.rect_stroke(swatch, 3.0, Stroke::new(line.width, line.color), StrokeKind::Inside);
                        }
                        Swatch::Pill(color) => {
                            painter.rect_stroke(swatch, 5.0, Stroke::new(1.0, color), StrokeKind::Inside);
                        }
                    }
                    painter.galley(Pos2::new(swatch.max.x + gap, y), galley, theme.text);
                }
                (None, None) => {}
            }
            y += h + 4.0;
        }
    }

    pub(crate) fn camera(&self) -> Camera {
        self.camera
    }

    pub(crate) fn set_camera(&mut self, camera: Camera) {
        self.camera = camera;
    }
}

/// A small rounded count, its right edge at `right_center`; returns where it is.
fn count_pill(painter: &egui::Painter, right_center: Pos2, text: &str, color: egui::Color32) -> Rect {
    let look = crate::theme::look();
    let theme = colors();
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(look.text_small), color);
    let size = galley.size() + Vec2::new(look.space_s * 2.0, look.space_hair * 2.0);
    let rect = Rect::from_min_size(Pos2::new(right_center.x - size.x, right_center.y - size.y / 2.0), size);
    painter.rect_filled(rect, look.radius_pill, theme.bg);
    painter.rect_stroke(rect, look.radius_pill, Stroke::new(look.line_thin, color), StrokeKind::Inside);
    painter.galley(rect.min + Vec2::new(look.space_s, look.space_hair), galley, color);
    rect
}

/// `text` as it fits in `width`: whole, shortened with an ellipsis, or not
/// at all when not even one letter fits.
fn fit_text(painter: &egui::Painter, text: &str, font: &FontId, width: f32) -> Option<String> {
    let fits = |t: &str| painter.layout_no_wrap(t.to_owned(), font.clone(), colors().text).size().x <= width;
    if fits(text) {
        return Some(text.to_owned());
    }
    // Halving, not one letter at a time: a long name costs a few layouts.
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(&format!("{}…", chars[..mid].iter().collect::<String>())) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lo > 0).then(|| format!("{}…", chars[..lo].iter().collect::<String>()))
}
