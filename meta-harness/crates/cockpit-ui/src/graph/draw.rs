//! The canvas: the tree as the server laid it out, through a camera that
//! only moves and zooms. Only what's on screen is drawn. Cards show their
//! lines of text (the same the server sized them for); lines show their
//! plan mark; the selection's lines show their pills; what isn't tied to
//! the selection dims.

use std::collections::BTreeMap;

use egui::{Align2, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Vec2};

use arch_graph::tree::{Card, Line as TreeLine, LineKind, Rect as WorldRect, ViewKind, Way, card_lines, count_text};

use super::{Camera, GraphState, Load, NavChange};
use crate::actions::{At, Control, RunCx, Style};
use crate::hosted::{Status, Tone};
use crate::feedback::FeedbackInspectorState;
use crate::graph_config::ChipMode;
use crate::graph_style::{LegendEntry, Swatch, style};
use crate::theme::colors;
use crate::touch::Gesture;
use crate::viewport::Show;

/// How far in the camera zooms: twice the normal text size.
const MAX_ZOOM: f32 = 2.0;

/// "A little" sideways, in points.
pub(super) const FEW: f32 = 48.0;

/// One notch of a mouse wheel, as browsers send it: in points, in lines.
const NOTCH_POINTS: f32 = 100.0;
const NOTCH_LINES: f32 = 3.0;

/// A flick slows as a list's does (egui's): by this many points a second,
/// each second, and stops below this speed.
const FRICTION: f32 = 1000.0;
const STOP_SPEED: f32 = 20.0;

impl GraphState {
    pub(crate) fn to_screen(&self, r: &WorldRect) -> Rect {
        let z = self.camera.zoom;
        Rect::from_min_size(self.canvas.min + self.camera.pan + Vec2::new(r.x, r.y) * z, Vec2::new(r.w, r.h) * z)
    }

    fn world_to_screen(&self, (x, y): (f32, f32)) -> Pos2 {
        self.canvas.min + self.camera.pan + Vec2::new(x, y) * self.camera.zoom
    }

    /// How far it zooms: out to the whole tree on the screen (or the
    /// normal size, if that already shows it all), in to twice the normal
    /// text size.
    pub(crate) fn zoom_limits(&self) -> (f32, f32) {
        (self.whole_tree_zoom().min(1.0), MAX_ZOOM)
    }

    /// The zoom that shows the whole tree, with room around it.
    fn whole_tree_zoom(&self) -> f32 {
        let Some(tree) = &self.tree else { return 1.0 };
        let pad = crate::theme::look().space_xxxl;
        let (w, h) = (tree.size.0.max(1.0), tree.size.1.max(1.0));
        ((self.canvas.width() - pad * 2.0) / w).min((self.canvas.height() - pad * 2.0) / h).max(f32::MIN_POSITIVE)
    }

    /// `camera` as the tree allows: zoomed within its limits, and never
    /// past the tree's edges (a tree smaller than the screen stays whole
    /// on it; a bigger one leaves no more than the room around it empty).
    pub(crate) fn clamped(&self, camera: Camera) -> Camera {
        let Some(tree) = &self.tree else { return camera };
        let (min, max) = self.zoom_limits();
        let zoom = camera.zoom.clamp(min, max);
        let pad = crate::theme::look().space_xxxl;
        let size = Vec2::new(tree.size.0, tree.size.1) * zoom;
        let window = self.canvas.size();
        let axis = |pan: f32, size: f32, window: f32| {
            let (a, b) = (pad, window - size - pad);
            pan.clamp(a.min(b), a.max(b))
        };
        Camera { pan: Vec2::new(axis(camera.pan.x, size.x, window.x), axis(camera.pan.y, size.y, window.y)), zoom }
    }

    /// Looks where `camera` says (as the tree allows): a key's move glides
    /// there; anything else is there at once.
    pub(crate) fn aim(&mut self, camera: Camera, glide: bool) {
        self.fling = Vec2::ZERO;
        let to = self.clamped(camera);
        self.glide = (glide && to != self.camera).then_some((self.camera, to, None));
        if !glide {
            self.camera = to;
        }
    }

    /// This frame's camera while a key's move glides (none: it's there).
    fn glide_on(&mut self, now: f64) -> bool {
        let Some((from, to, since)) = self.glide else { return false };
        let since = since.unwrap_or(now);
        let t = ((now - since) / crate::viewport::GLIDE_S).clamp(0.0, 1.0) as f32;
        let eased = crate::viewport::eased(t);
        self.camera = Camera { pan: from.pan + (to.pan - from.pan) * eased, zoom: from.zoom + (to.zoom - from.zoom) * eased };
        self.glide = (t < 1.0).then_some((from, to, Some(since)));
        self.glide.is_some()
    }

    /// Where it's going (where it is, if it isn't moving).
    pub(crate) fn aimed(&self) -> Camera {
        self.glide.map_or(self.camera, |(_, to, _)| to)
    }

    /// `camera` zoomed by `factor` around `around` (a point on the screen).
    fn zoomed(&self, camera: Camera, factor: f32, around: Pos2) -> Camera {
        let (min, max) = self.zoom_limits();
        let zoom = (camera.zoom * factor).clamp(min, max);
        let world = (around - self.canvas.min - camera.pan) / camera.zoom;
        Camera { pan: around - self.canvas.min - world * zoom, zoom }
    }

    /// Zooms by hand (the wheel, a pinch): there at once.
    pub(crate) fn zoom_by(&mut self, factor: f32, around: Pos2) {
        let camera = self.zoomed(self.camera, factor, around);
        self.moved_by_hand(camera);
    }

    /// A key's zoom: around the selected card if it's on the screen, else
    /// around the middle.
    pub(crate) fn zoom_key(&mut self, factor: f32) {
        let selected = self.selected_card().map(|c| self.to_screen(&c.rect).center()).filter(|p| self.canvas.contains(*p));
        let camera = self.zoomed(self.aimed(), factor, selected.unwrap_or(self.canvas.center()));
        self.aim(camera, true);
    }

    /// Moved by hand to `camera`: there at once, a glide stops, and the
    /// next key starts from what's on the screen.
    fn moved_by_hand(&mut self, camera: Camera) {
        self.glide = None;
        self.camera = self.clamped(camera);
        self.by_hand = true;
    }

    /// Where a new tree is first shown: the whole of it, unless that's too
    /// small to read; then readable, from what's selected below the roots,
    /// or the first root of a tidy tree, or the top.
    pub(crate) fn place_tree(&mut self) {
        let Some(tree) = &self.tree else { return };
        let area = self.canvas;
        let pad = crate::theme::look().space_xxxl;
        let zoom = self.whole_tree_zoom().min(1.0);
        if zoom >= self.ui.readable_zoom {
            self.fit_all(false);
            return;
        }
        // In a tidy tree its first card is centred on what it holds; boxes
        // in boxes read from the top.
        let first_root = (tree.request.view != ViewKind::Contains).then(|| tree.roots.first().and_then(|k| tree.card(k))).flatten();
        let zoom = self.ui.readable_zoom;
        let camera = match self.selected_card().filter(|c| c.level > 0).or(first_root).map(|c| c.rect) {
            // Its left edge a little in from the left, centred up and down.
            Some(focus) => Camera { pan: Vec2::new(pad, area.height() / 2.0) - Vec2::new(focus.x, focus.y + focus.h / 2.0) * zoom, zoom },
            None => Camera { pan: Vec2::splat(pad), zoom },
        };
        self.aim(camera, false);
    }

    /// The whole tree on the screen (`z z`).
    pub(crate) fn fit_all(&mut self, glide: bool) {
        let Some(tree) = &self.tree else { return };
        let zoom = self.whole_tree_zoom().min(1.0);
        let size = Vec2::new(tree.size.0, tree.size.1) * zoom;
        self.aim(Camera { pan: (self.canvas.size() - size) / 2.0, zoom }, glide);
    }

    /// The selected card filling the screen, as near as the zoom goes (`z f`).
    pub(crate) fn fit_card(&mut self) -> Result<(), String> {
        let card = self.selected_card().ok_or("Select a card first.")?;
        let pad = crate::theme::look().space_xxxl;
        let r = card.rect;
        let (min, max) = self.zoom_limits();
        let zoom = ((self.canvas.width() - pad * 2.0) / r.w.max(1.0)).min((self.canvas.height() - pad * 2.0) / r.h.max(1.0)).clamp(min, max);
        let middle = Vec2::new(r.x + r.w / 2.0, r.y + r.h / 2.0) * zoom;
        self.aim(Camera { pan: self.canvas.size() / 2.0 - middle, zoom }, true);
        Ok(())
    }

    /// Shows card `key` as asked: in view, moving only as far as needed
    /// (keeping room around it; a card bigger than the screen, its top-left
    /// corner), gliding; or in the middle, at once (a jump).
    pub(crate) fn show_card(&mut self, key: &str, show: Show) {
        let Some(card) = self.card(key) else { return };
        let camera = self.aimed();
        let z = camera.zoom;
        let rect = Rect::from_min_size(self.canvas.min + camera.pan + Vec2::new(card.rect.x, card.rect.y) * z, Vec2::new(card.rect.w, card.rect.h) * z);
        let room = crate::theme::look().space_xxxl.min(self.canvas.width().min(self.canvas.height()) / 4.0);
        let inside = self.canvas.shrink(room);
        let axis = |lo: f32, hi: f32, min: f32, max: f32| {
            // Bigger than the room: its start shows.
            if hi - lo > max - min || lo < min {
                min - lo
            } else if hi > max {
                max - hi
            } else {
                0.0
            }
        };
        match show {
            Show::InView => {
                let by = Vec2::new(axis(rect.left(), rect.right(), inside.left(), inside.right()), axis(rect.top(), rect.bottom(), inside.top(), inside.bottom()));
                if by != Vec2::ZERO {
                    self.aim(Camera { pan: camera.pan + by, zoom: z }, true);
                }
            }
            Show::Middle => {
                let by = if rect.width() > inside.width() || rect.height() > inside.height() { inside.min - rect.min } else { self.canvas.center() - rect.center() };
                self.aim(Camera { pan: camera.pan + by, zoom: z }, false);
            }
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

    pub(super) fn draw(&mut self, ui: &mut egui::Ui, feedback: &mut FeedbackInspectorState, pan_held: bool, cx: &mut RunCx<'_>) {
        self.render_where(ui);
        let canvas = ui.available_rect_before_wrap();
        self.canvas = canvas;
        feedback.register_inspectable(ui, &format!("{}:graph:canvas", cx.at.name()), canvas);
        let response = ui.allocate_rect(canvas, Sense::click_and_drag());
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 0.0, colors().bg);
        self.draw_grid(&painter);
        let time = ui.input(|i| i.time);
        if self.fit && self.tree.is_some() {
            self.fit = false;
            self.place_tree();
        }
        if let Some((key, show)) = self.reveal_after_fit.take() {
            self.show_card(&key, show);
        }
        if self.glide_on(time) {
            ui.ctx().request_repaint();
        }
        if pan_held && response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        self.hand(ui, &response);
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
                    _ if tree.request.only.is_some() => "Nothing the changes touch is in this view. Show everything, or compare something else.".to_owned(),
                    (_, arch_graph::Source::Plan(p)) if !tree.request.full_code => format!("Plan {p} has nothing for this view yet. Show it in the full code, or pick another view."),
                    _ => "Nothing to show in this view yet. Read the code in, or pick another view.".to_owned(),
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
        // Click or tap: a count opens its card; a card selects; a double
        // one opens it (on nothing: the whole tree fits).
        if (response.clicked() || response.double_clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let Some(key) = self.hit_more(pos) {
                self.navigate(NavChange::Open(key));
            } else {
                let hit = self.hit(pos);
                match &hit {
                    // A double click opens it, as everywhere: what Enter does.
                    Some(_) if response.double_clicked() => *cx.asked = Some((crate::nav::Move::Enter.id().to_owned(), None)),
                    None if response.double_clicked() => self.fit_all(true),
                    _ => {}
                }
                self.select(hit);
            }
        }
        if self.legend_open {
            self.draw_legend(&painter);
        }
    }

    /// Moved by hand, as every view is: a drag (the mouse, a finger) moves
    /// it and a flick glides on; the wheel and two fingers on a trackpad
    /// move it (Shift: sideways); the wheel with the zoom key held
    /// (`[keymap.pointer] zoom`) zooms a step a notch, as the zoom keys do,
    /// and pinching (a trackpad, two fingers) zooms, around the pointer or
    /// the fingers. Two fingers on a screen also pan.
    fn hand(&mut self, ui: &egui::Ui, response: &egui::Response) {
        let zoom_key = crate::keymap::local().pointer.zoom;
        let (touch, touching, scroll, notches, pinch, pointer, velocity, dt) = ui.input(|i| {
            let (mut notches, mut pinch) = (0.0, 1.0);
            for event in &i.events {
                match event {
                    egui::Event::MouseWheel { unit, delta, modifiers } if zoom_key.held(*modifiers) => {
                        notches += match unit {
                            egui::MouseWheelUnit::Point => delta.y / NOTCH_POINTS,
                            egui::MouseWheelUnit::Line => delta.y / NOTCH_LINES,
                            egui::MouseWheelUnit::Page => delta.y,
                        };
                    }
                    egui::Event::Zoom(factor) => pinch *= factor,
                    _ => {}
                }
            }
            (i.multi_touch(), i.any_touches(), i.smooth_scroll_delta, notches, pinch, i.pointer.hover_pos(), i.pointer.velocity(), i.stable_dt.min(0.1))
        });
        if let Some(touch) = touch.filter(|t| response.rect.contains(t.center_pos)) {
            (self.fling, self.pinching) = (Vec2::ZERO, true);
            let panned = Camera { pan: self.camera.pan + touch.translation_delta, zoom: self.camera.zoom };
            self.moved_by_hand(self.zoomed(panned, touch.zoom_delta, touch.center_pos));
            return;
        }
        if response.dragged() {
            self.fling = Vec2::ZERO;
            self.moved_by_hand(Camera { pan: self.camera.pan + response.drag_delta(), zoom: self.camera.zoom });
        }
        if response.drag_stopped() && !self.pinching {
            self.fling = velocity;
        }
        if !touching {
            self.pinching = false;
        }
        if response.hovered() {
            if scroll != Vec2::ZERO && notches == 0.0 {
                self.fling = Vec2::ZERO;
                self.moved_by_hand(Camera { pan: self.camera.pan + scroll, zoom: self.camera.zoom });
            }
            let factor = self.ui.zoom_step.powf(notches) * pinch;
            if factor != 1.0 {
                self.zoom_by(factor, pointer.unwrap_or(self.canvas.center()));
            }
        }
        let speed = self.fling.length();
        if speed > 0.0 {
            let slower = speed - FRICTION * dt;
            if slower < STOP_SPEED {
                self.fling = Vec2::ZERO;
            } else {
                self.fling *= slower / speed;
                let want = self.camera.pan + self.fling * dt;
                self.moved_by_hand(Camera { pan: want, zoom: self.camera.zoom });
                // At an edge: it stops there.
                if self.camera.pan != want {
                    self.fling = Vec2::ZERO;
                }
                ui.ctx().request_repaint();
            }
        }
    }

    /// A key's move of what's shown by `by` (on the screen).
    pub(crate) fn pan_by(&mut self, by: Vec2) {
        let camera = self.aimed();
        self.aim(Camera { pan: camera.pan + by, zoom: camera.zoom }, true);
    }

    /// A long press on its canvas (the host hands it over) selects what's
    /// under it; its controls show under it. Everything else fingers do is
    /// the pointer's.
    pub fn on_gesture(&mut self, gesture: Gesture) {
        if let Gesture::LongPress(pos) = gesture
            && let Some(key) = self.hit(pos)
        {
            self.select(Some(key));
        }
    }

    fn chips_shown(&self) -> bool {
        match self.ui.chips.show {
            ChipMode::Always => true,
            ChipMode::Never => false,
            ChipMode::Auto => self.fingers || self.canvas.width() < self.ui.chips.narrow_below,
        }
    }

    /// Where the graph is: the source, the view and way, the roots gone
    /// through (each a way back), what holds the root, how deep, what was
    /// cut, and how long it took.
    fn render_where(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let look = crate::theme::look();
            let muted = |t: String| RichText::new(t).size(look.text_body).color(colors().muted);
            let source = self.source_label();
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
            // The roots gone through, then the root (back along it: the
            // Path picker on the top line).
            ui.label(muted("start".into()));
            for root in self.trail_roots().iter().flatten() {
                ui.label(muted(format!("› {}", name(root))));
            }
            if let Some(root) = &self.nav.root {
                ui.label(muted("›".into()));
                ui.label(RichText::new(name(root)).size(look.text_body).strong().color(colors().text_strong));
            }
            if let Some(tree) = &self.tree {
                if !tree.path.is_empty() {
                    ui.label(muted("· inside".into()));
                    for crumb in &tree.path {
                        ui.label(muted(crumb.label.clone()));
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
                    // Its fixes are actions: showing the levels that fit (on
                    // the top line), going into the selected card (under it).
                    ui.label(RichText::new(format!("{glyph} {text}")).size(look.text_body).color(color));
                }
                if let Some(ms) = tree.slow_ms {
                    ui.label(RichText::new(format!("· slow: {ms} ms")).size(look.text_body).color(colors().warn))
                        .on_hover_text("The server took longer than its limit to build this. Each stage's time is at /api/perf.");
                }
            }
        });
        ui.separator();
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
    pub(crate) fn visible(&self, rect: Rect) -> bool {
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
                let fitted = self.fitted.borrow_mut().get(text, px, rect.width() - pad * 2.0, z, |t, w| fit_text(painter, t, &font, w));
                if let Some(text) = fitted {
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
            if let Some((added, removed)) = self.changed(id) {
                // What the desk's comparison changed in it, at its bottom left.
                let galley = painter.layout_no_wrap(format!("{} +{added} −{removed}", egui_phosphor::regular::GIT_DIFF), FontId::proportional(look.text_tiny), theme.text);
                let chip = Rect::from_min_size(rect.left_bottom() + Vec2::new(look.space_s, -galley.size().y / 2.0 - look.space_hair), galley.size() + Vec2::new(look.space_s * 2.0, look.space_hair * 2.0));
                painter.rect_filled(chip, look.radius_pill, theme.ok_bg);
                painter.rect_stroke(chip, look.radius_pill, Stroke::new(look.line_thin, theme.ok), StrokeKind::Inside);
                painter.galley(chip.min + Vec2::new(look.space_s, look.space_hair), galley, theme.text);
            }
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
        let st = style().line(&theme, line.relation, line.kind, line.mark, line.evidence.as_ref());
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
    /// (calls), the steps between two data types (data flow); where the
    /// server's layout put them, at the camera's zoom.
    fn draw_pills(&self, painter: &egui::Painter, line: &TreeLine) {
        let Some(layout) = self.config.as_ref().map(|c| &c.layout) else { return };
        let look = crate::theme::look();
        let theme = colors();
        let px = layout.small_size * self.camera.zoom;
        for pill in &line.pills {
            let Some(at) = pill.rect else { continue };
            let rect = Rect::from_min_max(self.world_to_screen((at.x, at.y)), self.world_to_screen((at.right(), at.bottom())));
            if !self.visible(rect) {
                continue;
            }
            let color = style().pill_color(&theme, pill.role);
            painter.rect_filled(rect, look.radius_pill, theme.bg);
            painter.rect_stroke(rect, look.radius_pill, Stroke::new(look.line_thin, color), StrokeKind::Inside);
            if px >= self.ui.min_text_px {
                painter.with_clip_rect(rect).text(rect.center(), Align2::CENTER_CENTER, &pill.text, FontId::proportional(px), theme.text_strong);
            }
        }
    }

    /// The legend: what each color, line and outline means, from the style
    /// table, sized to its text and clear of the chips.
    fn draw_legend(&self, painter: &egui::Painter) {
        let theme = colors();
        let entries: Vec<LegendEntry> = style().legend(&theme);
        let look = crate::theme::look();
        let (pad, swatch_w, gap) = (look.space_ml, look.space_xxl + look.space_s, look.space_m);
        let max_w = (self.canvas.width() - look.space_xl).clamp(self.ui.legend.min_w, self.ui.legend.max_w);
        let text_w = max_w - pad * 2.0 - swatch_w - gap;
        let font = FontId::proportional(look.text_body);
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
        let row_min = look.space_l + look.space_xs;
        let height: f32 = rows.iter().map(|(_, _, g)| g.size().y.max(row_min) + look.space_s).sum::<f32>() + pad * 2.0;
        let top = self.canvas.min.y + look.space_m;
        let left = self.canvas.max.x - width - look.space_m;
                let rect = Rect::from_min_size(Pos2::new(left, top), Vec2::new(width, height)).intersect(self.canvas);
        painter.rect_filled(rect, look.radius_l, theme.overlay);
        let mut y = rect.min.y + pad;
        for (head, entry, galley) in rows {
            let h = galley.size().y.max(row_min);
            match (head, entry) {
                (Some(_), _) => {
                    painter.galley(Pos2::new(rect.min.x + pad, y), galley, theme.muted);
                }
                (None, Some(entry)) => {
                    let swatch = Rect::from_min_size(Pos2::new(rect.min.x + pad, y + look.space_xs), Vec2::new(swatch_w, look.space_ml));
                    match entry.swatch {
                        Swatch::Line(line) => {
                            let points = [swatch.left_center(), swatch.right_center()];
                            if line.dashed {
                                painter.extend(Shape::dashed_line(&points, Stroke::new(line.width, line.color), look.space_sm, look.space_s));
                            } else {
                                painter.line_segment(points, Stroke::new(line.width, line.color));
                            }
                        }
                        Swatch::Outline(line) => {
                            painter.rect_stroke(swatch, look.radius_s, Stroke::new(line.width, line.color), StrokeKind::Inside);
                        }
                        Swatch::Pill(color) => {
                            painter.rect_stroke(swatch, look.radius_m, Stroke::new(look.line_thin, color), StrokeKind::Inside);
                        }
                    }
                    painter.galley(Pos2::new(swatch.max.x + gap, y), galley, theme.text);
                }
                (None, None) => {}
            }
            y += h + look.space_s;
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

/// Card texts as they fit, measured once per text, size and width; kept
/// until the zoom changes (every size and width does then).
#[derive(Default)]
pub(super) struct Fitted {
    zoom: f32,
    texts: std::collections::HashMap<(String, u32, u32), Option<String>>,
}

impl Fitted {
    pub(super) fn get(&mut self, text: &str, px: f32, width: f32, zoom: f32, fit: impl FnOnce(&str, f32) -> Option<String>) -> Option<String> {
        if zoom != self.zoom {
            self.texts.clear();
            self.zoom = zoom;
        }
        self.texts.entry((text.to_owned(), px.to_bits(), width.to_bits())).or_insert_with(|| fit(text, width)).clone()
    }
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

impl crate::hosted::Hosted for GraphState {
    /// What it shows: the code, or a plan.
    fn title(&self) -> Status {
        Status::new(format!("{} {}", egui_phosphor::regular::GRAPH, self.source_label()), Tone::Plain)
    }

    /// Its canvas: a tap selects, a double tap goes in, two fingers move it.
    fn own_pointer(&self) -> bool {
        true
    }

    fn busy(&self) -> Option<String> {
        (self.load == Load::Loading).then(|| "Loading the view".into())
    }

    /// An answer cut short or slow (said on the top bar).
    fn alert(&self) -> Option<Status> {
        self.answer_status().map(|(words, more)| Status::new(words, Tone::Warn).detail(more))
    }

    /// The files changed since they were read, the agent having moved it,
    /// and what it has to say.
    fn status(&self) -> Vec<Status> {
        let mut out = Vec::new();
        if !self.stale_files.is_empty() {
            let (glyph, word, _) = crate::status::look("stale");
            out.push(Status::new(format!("{glyph} {} {word}", self.stale_files.len()), Tone::Warn).detail(self.stale_files.join("\n")));
        }
        if let Some(agent) = &self.agent {
            out.push(Status::new(format!("{} the agent moved the view: {}", egui_phosphor::regular::ROBOT, agent.what), Tone::Accent));
        }
        if let Some(note) = &self.note {
            out.push(Status::new(note.clone(), Tone::Warn));
        }
        out
    }

    /// On its top line: the jump list's back and forward, and (on touch or
    /// narrow screens) the chips (reading changed code again is the top
    /// bar's); under the
    /// selected card, the actions for it (`[graph] context`).
    fn placed(&self) -> Vec<(String, Control)> {
        let header = |icon: Option<&'static str>, caption: &str| Control::new(At::Header, Style::Button, caption, icon);
        let mut out = vec![("nav.back".to_owned(), header(Some(egui_phosphor::regular::ARROW_LEFT), "")), ("nav.forward".to_owned(), header(Some(egui_phosphor::regular::ARROW_RIGHT), ""))];
        if self.chips_shown() {
            out.extend(self.ui.chips.actions.iter().filter(|id| !out.iter().any(|(o, _)| o == *id)).map(|id| (id.clone(), header(None, ""))).collect::<Vec<_>>());
        }
        if self.selection.is_some() {
            out.extend(self.ui.context.iter().map(|id| (id.clone(), Control::new(At::Item, Style::Button, "", None))));
        }
        out
    }

}
