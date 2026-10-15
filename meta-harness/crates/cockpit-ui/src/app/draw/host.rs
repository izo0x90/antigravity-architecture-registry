//! Hosts: a column and the quick look hold a view and draw it the same
//! way: the view's top line (its title and status, from `Panel`), then its
//! content. A view draws no controls. Every control comes from the one
//! bound list of where it is (`shown_at`): the panel's layer on its top
//! line, what you're on under it, the host's own (tabs, closing, sending
//! the quick look's card to a column) on the host's line. A control shows
//! the key its binding has, and used, asks for its action: the app runs it
//! once the frame is drawn, the way its key runs it.
//!
//! Pointer and touch, the same in every view: a click or tap on an item
//! moves the cursor there (the column is focused by it); a double click or
//! tap opens it (Enter); a long press lists what can be done (the palette,
//! with the cursor on it). The view only says where its items are.

use super::super::*;
use crate::actions::At;

impl CockpitApp {
    /// Draws the view where `here` is (the one in use): its top line, then
    /// its content.
    pub(in crate::app) fn render_view(&mut self, here: Here, ui: &mut egui::Ui, is_focused: bool) {
        let (kind, slot) = (here.kind, here.slot);
        let ctx = ui.ctx().clone();
        let pan_held = ui.input(|i| self.config.keymap.leader.key_down(i));
        let panel: Vec<Bound> = self.shown_at(here).into_iter().filter(|b| b.layer == Layer::Panel).collect();
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, asked: &mut self.asked };
        crate::panel::header(ui, self.desk.panel(kind), &panel, &mut cx);
        ui.separator();
        let content = ui.available_rect_before_wrap();
        match kind {
            BufferKind::Chat => self.desk.chat.render(ui, &mut self.feedback, is_focused, &mut cx),
            BufferKind::Code => self.desk.code.render(ui, &mut self.feedback, &mut cx),
            BufferKind::Graph => self.desk.graph.render(ui, &mut self.feedback, slot, pan_held, &mut cx),
            BufferKind::Tree => self.desk.tree.render(ui, &mut self.feedback, &mut cx),
            BufferKind::Spec => {
                // A card over a column: what the graph says about it first.
                if self.desk.spec.floating
                    && let Some(id) = self.desk.spec.target.clone()
                {
                    self.desk.graph.render_card_context(ui, &id);
                    ui.separator();
                }
                self.desk.spec.render(ui, &mut cx);
            }
            BufferKind::Queue => self.desk.queue.render(ui, &mut cx),
            BufferKind::Inbox => self.desk.inbox.render(ui, &mut cx, &mut self.desk.chat),
        }
        self.pointer_on_content(kind, ui, content);
    }

    /// A click or double click on a view's content (not on something over
    /// it): on one of its items, the cursor goes there; a double opens it.
    fn pointer_on_content(&mut self, kind: BufferKind, ui: &egui::Ui, content: egui::Rect) {
        if self.desk.panel(kind).own_pointer() {
            return;
        }
        let (clicked, double, at) = ui.input(|i| (i.pointer.primary_clicked(), i.pointer.button_double_clicked(egui::PointerButton::Primary), i.pointer.interact_pos()));
        let Some(at) = at.filter(|at| (clicked || double) && content.contains(*at) && ui.ctx().layer_id_at(*at) == Some(ui.layer_id())) else { return };
        let Some(key) = self.desk.view(kind).spots().into_iter().find(|s| s.rect.contains(at)).map(|s| s.key) else { return };
        self.desk.view_mut(kind).move_to(&key);
        if double {
            self.asked = Some(("nav.enter".to_owned(), None));
        }
    }

    /// A host's own controls: those of `bound` that have one.
    pub(in crate::app) fn host_controls(&mut self, ui: &mut egui::Ui, bound: &[Bound], slot: usize) {
        let ctx = ui.ctx().clone();
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, asked: &mut self.asked };
        crate::actions::controls(ui, bound, &mut cx);
    }

    /// Under where you are in the focused view (the cursor's item, or the
    /// box being typed in): what you're on's controls, from the one list.
    /// Below it, or above when there's no room.
    pub(in crate::app) fn render_item_controls(&mut self, ctx: &egui::Context) {
        let kind = self.focused_kind();
        let (slot, bounds) = match &self.quick_look {
            Some(look) => (look.slot, ctx.memory(|m| m.area_rect(egui::Id::new("quick_look")))),
            None => (self.desk.columns.focused(), self.slot_rects.get(self.desk.columns.focused()).copied()),
        };
        let Some(bounds) = bounds else { return };
        let panel = self.desk.panel(kind);
        let (at, anchor) = match self.input_mode {
            InputMode::Insert { .. } => (At::Typing, panel.typing_rect()),
            _ => {
                let view = self.desk.view(kind);
                let here = panel.here().or_else(|| view.cursor_key());
                (At::Item, view.spots().into_iter().find(|s| Some(&s.key) == here.as_ref()).map(|s| s.rect))
            }
        };
        let Some(anchor) = anchor.map(|r| r.intersect(bounds)).filter(|r| r.is_positive()) else { return };
        let situation = self.situation();
        let here: Vec<Bound> = self.bound_actions().into_iter().filter(|b| b.layer == Layer::On && b.action.at() == Some(at) && b.action.when.holds(&situation)).collect();
        if here.is_empty() {
            return;
        }
        let id = egui::Id::new("item_controls");
        let look = crate::theme::look();
        let size = ctx.memory(|m| m.area_rect(id)).map_or(egui::vec2(anchor.width(), look.text_title * 2.0), |r| r.size());
        let below = anchor.bottom() + look.space_s;
        let y = if below + size.y <= bounds.bottom() { below } else { (anchor.top() - look.space_s - size.y).max(bounds.top()) };
        let mut cx = RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, asked: &mut self.asked };
        egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(egui::pos2(anchor.left(), y)).show(ctx, |ui| {
            Frame::NONE.fill(colors().overlay_raised).stroke(Stroke::new(look.line_thin, colors().border)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_s)).show(ui, |ui| {
                ui.set_max_width(anchor.width().max(look.quick_look_width / 3.0).min(bounds.width()));
                ui.horizontal_wrapped(|ui| crate::actions::controls(ui, &here, &mut cx));
            });
        });
    }

    /// This frame's fingers, as gestures, on the view each started on: a
    /// swipe runs what the keymap binds to it there (the view's own first);
    /// a long press puts the cursor on the item under it and lists what can
    /// be done (the palette); a view that reads its canvas itself (the
    /// graph) gets the rest. Taps are the pointer's (a click).
    pub(in crate::app) fn touch_tick(&mut self, ctx: &egui::Context) {
        let (events, time) = ctx.input(|i| {
            let touches: Vec<(u64, crate::touch::Phase, egui::Pos2)> = i.events.iter().filter_map(|e| match e {
                egui::Event::Touch { id, phase, pos, .. } => Some((id.0, crate::touch::Phase::from(*phase), *pos)),
                _ => None,
            }).collect();
            (touches, i.time)
        });
        let cfg = self.config.touch.clone();
        let mut gestures = Vec::new();
        for (id, phase, pos) in events {
            // A finger that starts on a control (over the views) is the control's.
            if phase == crate::touch::Phase::Start && self.view_at(ctx, pos).is_none() {
                continue;
            }
            gestures.extend(self.touch.feed(id, phase, pos, time, &cfg));
        }
        gestures.extend(self.touch.tick(time, &cfg));
        if let Some(due) = self.touch.long_press_due(&cfg) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - time).max(0.0)));
        }
        for gesture in gestures {
            self.on_gesture(gesture, ctx);
        }
    }

    /// The view (and its column) at `pos`: the quick look's, over its
    /// column; else the column's. None over anything else.
    fn view_at(&self, ctx: &egui::Context, pos: egui::Pos2) -> Option<(usize, BufferKind)> {
        if let Some(look) = &self.quick_look
            && ctx.memory(|m| m.area_rect(egui::Id::new("quick_look"))).is_some_and(|r| r.contains(pos))
        {
            return (ctx.layer_id_at(pos).map(|l| l.id) == Some(egui::Id::new("quick_look"))).then_some((look.slot, look.kind));
        }
        if ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background) {
            return None;
        }
        let slot = self.slot_rects.iter().position(|r| r.contains(pos))?;
        Some((slot, self.desk.columns.kind(slot)))
    }

    fn on_gesture(&mut self, gesture: crate::touch::Gesture, ctx: &egui::Context) {
        use crate::touch::Gesture;
        let at = match gesture {
            Gesture::Tap(p) | Gesture::DoubleTap(p) | Gesture::LongPress(p) | Gesture::Pinch { center: p, .. } | Gesture::Swipe { from: p, .. } => Some(p),
            Gesture::Pan(_) => None,
        };
        let Some((slot, kind)) = at.map_or_else(|| Some((self.desk.columns.focused(), self.focused_kind())), |p| self.view_at(ctx, p)) else { return };
        if self.quick_look.is_none() && self.desk.columns.focused() != slot {
            self.desk.columns.focus(slot);
            // Where you are moved: the list is gathered again.
            self.frame_actions.borrow_mut().take();
        }
        let own = self.desk.panel(kind).own_pointer();
        match gesture {
            Gesture::Swipe { way, .. } => {
                let bound = self.bound_actions();
                let situation = self.situation();
                let runs = |b: &&Bound| b.runs_on(way.name()) && b.action.when.holds(&situation);
                // The closest one to where you are.
                let found = bound.iter().filter(runs).min_by_key(|b| b.layer);
                if let Some(b) = found {
                    self.asked = Some((b.action.id.clone(), None));
                }
            }
            Gesture::LongPress(p) if !own => {
                if let Some(key) = self.desk.view(kind).spots().into_iter().find(|s| s.rect.contains(p)).map(|s| s.key) {
                    self.desk.view_mut(kind).move_to(&key);
                    self.asked = Some(("palette.open".to_owned(), None));
                }
            }
            other if own => self.desk.view_mut(kind).gesture(other),
            _ => {}
        }
    }

    /// Runs the action a control asked for this frame.
    pub(in crate::app) fn run_asked(&mut self, ctx: &egui::Context) {
        if let Some((id, value)) = self.asked.take() {
            self.run_by_id(&id, value, ctx);
        }
    }
}
