//! Hosts: a panel and the quick look hold a view and draw it the same
//! way: the view's top line (its title and status, from `Panel`), then its
//! content. A view draws no controls. Every control comes from the one
//! bound list of where it is (`shown_at`): the panel's layer on its top
//! line, what you're on under it, the host's own (tabs, closing, sending
//! the quick look's card to a panel) on the host's line. A control shows
//! the key its binding has, and used, asks for its action: the app runs it
//! once the frame is drawn, the way its key runs it.
//!
//! Pointer and touch, the same in every view: a click or tap on an item
//! moves the cursor there (the panel is focused by it); a double click or
//! tap opens it (Enter); a long press lists what can be done (the palette,
//! with the cursor on it). The view only says where its items are.

use super::super::*;
use crate::keymap::Host;
use crate::actions::At;
use crate::hosted::ItemButtons;

impl CockpitApp {
    /// Draws the view where `here` is (the one in use): its top line, then
    /// its content.
    pub(in crate::app) fn render_view(&mut self, here: Here, ui: &mut egui::Ui, is_focused: bool) {
        let kind = here.kind;
        let ctx = ui.ctx().clone();
        let leader_held = ui.input(|i| self.config.keymap.leader.key_down(i));
        // A card over a panel: what the other buffers say about what it shows.
        let says = if here.at == Host::Look { self.others_say(kind) } else { Vec::new() };
        let mine: Vec<Bound> = self.shown_at(here).into_iter().filter(|b| b.layer == Layer::Panel).collect();
        let buttons = self.item_buttons_now();
        let tagged = self.notes_named(here);
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, at: here.at, note: &mut self.note, asked: &mut self.asked };
        crate::hosted::header(ui, self.desk.view(kind), &mine, &tagged, &mut cx);
        ui.separator();
        // The buttons of what you're on in a bar: their row kept along the
        // bottom (drawn there once everything is laid out), the buffer above.
        let mut whole = ui.available_rect_before_wrap();
        // The gutter of notes, down the left of a view that draws things
        // notes can be on (always, so its content doesn't move as notes
        // come and go).
        let gutter = self.desk.view(kind).draws_things().is_some_and(|d| !d.in_place()).then(|| {
            let gutter = egui::Rect::from_min_max(whole.min, egui::pos2(whole.left() + crate::app::notes::gutter_width(), whole.bottom()));
            whole.min.x = gutter.right();
            gutter
        });
        let content = match buttons {
            ItemButtons::Floating => whole,
            ItemButtons::Bar => {
                let height = ui.spacing().interact_size.y + crate::theme::look().space_s * 2.0;
                let bar = egui::Rect::from_min_max(egui::pos2(whole.left(), whole.bottom() - height), whole.right_bottom());
                self.item_bars.push((here.at, bar));
                egui::Rect::from_min_max(whole.min, egui::pos2(whole.right(), bar.top()))
            }
        };
        // It has the keyboard while typing is in its host (and no
        // developer window has it).
        let typing = self.input_mode == (InputMode::Insert { at: here.at }) && !self.feedback.is_modal_open && !self.feedback.is_issues_window_open && !self.feedback.is_inspecting;
        let mut drawing = crate::buffer::Drawing { feedback: &mut self.feedback, focused: is_focused, leader_held, typing, fingers: self.fingers };
        let drawn = self.desk.in_use(kind);
        self.desk.on_screen(drawn);
        let (buffer, net) = self.desk.view_with_net(kind);
        let mut cx = RunCx { net, ctx: &ctx, input_mode: &mut self.input_mode, at: here.at, note: &mut self.note, asked: &mut self.asked };
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            if !says.is_empty() {
                crate::hosted::says(ui, &says);
                ui.separator();
            }
            buffer.tick(net, &ctx);
            buffer.render(ui, &mut cx, &mut drawing);
        });
        if content != whole {
            ui.painter().hline(whole.x_range(), content.bottom(), egui::Stroke::new(crate::theme::look().line_thin, colors().border));
            ui.allocate_rect(whole, egui::Sense::hover());
        }
        self.draw_notes(here, ui, gutter.map(|g| g.intersect(egui::Rect::everything_above(content.bottom()))), content);
        self.pointer_on_content(kind, ui, content);
    }

    /// What the buffers in use, but the one of `kind`, say about what it
    /// shows (a component).
    fn others_say(&self, kind: BufferKind) -> Vec<crate::hosted::Status> {
        let Some(id) = self.desk.view(kind).target().and_then(|t| t.component().cloned()) else { return Vec::new() };
        BufferKind::ALL.into_iter().filter(|k| *k != kind).filter_map(|k| self.desk.view(k).knows(&id)).flat_map(|known| known.says).collect()
    }

    /// Where the buttons of what you're on show now: as changed here, else
    /// as the UI config says.
    pub(in crate::app) fn item_buttons_now(&self) -> ItemButtons {
        self.item_buttons.unwrap_or(self.config.panels.item_buttons)
    }

    /// A click or double click on a view's content (not on something over
    /// it): on one of its items, the cursor goes there; a double opens it.
    fn pointer_on_content(&mut self, kind: BufferKind, ui: &egui::Ui, content: egui::Rect) {
        if self.desk.view(kind).own_pointer() {
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
    pub(in crate::app) fn host_controls(&mut self, ui: &mut egui::Ui, bound: &[Bound], host: Host) {
        let ctx = ui.ctx().clone();
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, at: host, note: &mut self.note, asked: &mut self.asked };
        crate::actions::controls(ui, bound, &mut cx);
    }

    /// Where you are in the focused buffer (the cursor's item, or the box
    /// being typed in): what you're on's controls, from the one list. In
    /// their bar when they're in one; else floating below it, or above when
    /// there's no room.
    pub(in crate::app) fn render_item_controls(&mut self, ctx: &egui::Context) {
        let kind = self.focused_kind();
        let (host, bounds) = match &self.quick_look {
            Some(_) => (Host::Look, ctx.memory(|m| m.area_rect(egui::Id::new("quick_look")))),
            None => (Host::Panel(self.desk.panels.focused()), self.panel_rects.get(self.desk.panels.focused()).copied()),
        };
        let Some(bounds) = bounds else { return };
        let hosted = self.desk.view(kind);
        let (at, anchor) = match self.input_mode {
            InputMode::Insert { .. } => (At::Typing, hosted.typing_rect()),
            _ => {
                let view = self.desk.view(kind);
                let here = hosted.here().or_else(|| view.cursor_key());
                (At::Item, view.spots().into_iter().find(|s| Some(&s.key) == here.as_ref()).map(|s| s.rect))
            }
        };
        let anchor = anchor.map(|r| r.intersect(bounds)).filter(|r| r.is_positive());
        let situation = self.situation();
        let here: Vec<Bound> = self.bound_actions().into_iter().filter(|b| b.layer == Layer::On && b.action.at() == Some(at) && b.action.when.holds(&situation)).collect();
        if here.is_empty() {
            return;
        }
        let id = egui::Id::new("item_controls");
        let look = crate::theme::look();
        let mut cx = RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, at: host, note: &mut self.note, asked: &mut self.asked };
        // In a bar: its host's row, the buttons then what they act on
        // (whether or not the item is on screen). Floating: under the item (above it
        // when there's no room).
        match self.item_bars.iter().find(|(h, _)| *h == host).map(|(_, rect)| *rect) {
            Some(bar) => {
                let what = self.desk.view(kind).target().map(|t| t.in_words());
                egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(bar.min).show(ctx, |ui| {
                    ui.set_clip_rect(bar);
                    Frame::NONE.inner_margin(crate::theme::margin2(look.space_s, look.space_s)).show(ui, |ui| {
                        ui.set_width(bar.width() - look.space_s * 2.0);
                        // The buttons first, as on every line: what they act
                        // on after them, never pushing them.
                        ui.horizontal(|ui| {
                            crate::actions::controls(ui, &here, &mut cx);
                            if let Some(what) = what {
                                ui.label(RichText::new(what).size(look.text_small).color(colors().muted));
                            }
                        });
                    });
                });
            }
            // Floating needs the item on screen.
            None => {
                let Some(anchor) = anchor else { return };
                let size = ctx.memory(|m| m.area_rect(id)).map_or(egui::vec2(anchor.width(), look.text_title * 2.0), |r| r.size());
                let below = anchor.bottom() + look.space_s;
                let y = if below + size.y <= bounds.bottom() { below } else { (anchor.top() - look.space_s - size.y).max(bounds.top()) };
                egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(egui::pos2(anchor.left(), y)).show(ctx, |ui| {
                    Frame::NONE.fill(colors().overlay_raised).stroke(Stroke::new(look.line_thin, colors().border)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_s)).show(ui, |ui| {
                        ui.set_max_width(anchor.width().max(look.quick_look_width / 3.0).min(bounds.width()));
                        ui.horizontal_wrapped(|ui| crate::actions::controls(ui, &here, &mut cx));
                    });
                });
            }
        }
    }

    /// This frame's fingers, as gestures, on the view each started on: a
    /// swipe runs what the keymap binds to it there (the view's own first),
    /// unless the view moves sideways (the finger moves it); a long press
    /// puts the cursor on the item under it and lists what can be done (the
    /// palette), or is the view's when it reads its canvas itself (the
    /// graph). Taps, drags and two fingers are the pointer's.
    pub(in crate::app) fn touch_tick(&mut self, ctx: &egui::Context) {
        let (events, pointed, time) = ctx.input(|i| {
            let touches: Vec<(u64, crate::touch::Phase, egui::Pos2)> = i.events.iter().filter_map(|e| match e {
                egui::Event::Touch { id, phase, pos, .. } => Some((id.0, crate::touch::Phase::from(*phase), *pos)),
                _ => None,
            }).collect();
            (touches, i.events.iter().any(|e| matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. })), i.time)
        });
        // A finger's pointer comes with its touches; a mouse's alone.
        if !events.is_empty() {
            self.fingers = true;
        } else if pointed {
            self.fingers = false;
        }
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

    /// The view (and its panel) at `pos`: the quick look's, over its
    /// panel; else the panel's. None over anything else.
    fn view_at(&self, ctx: &egui::Context, pos: egui::Pos2) -> Option<(usize, BufferKind)> {
        if let Some(look) = &self.quick_look
            && ctx.memory(|m| m.area_rect(egui::Id::new("quick_look"))).is_some_and(|r| r.contains(pos))
        {
            return (ctx.layer_id_at(pos).map(|l| l.id) == Some(egui::Id::new("quick_look"))).then_some((look.panel, look.kind()));
        }
        if ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background) {
            return None;
        }
        let panel = self.panel_rects.iter().position(|r| r.contains(pos))?;
        Some((panel, self.desk.panels.kind(panel)))
    }

    fn on_gesture(&mut self, gesture: crate::touch::Gesture, ctx: &egui::Context) {
        use crate::touch::Gesture;
        let at = match gesture {
            Gesture::LongPress(p) | Gesture::Swipe { from: p, .. } => p,
        };
        let Some((panel, kind)) = self.view_at(ctx, at) else { return };
        if self.quick_look.is_none() && self.desk.panels.focused() != panel {
            self.desk.panels.focus(panel);
            // Where you are moved: the list is gathered again.
            self.frame_actions.borrow_mut().take();
        }
        let own = self.desk.view(kind).own_pointer();
        match gesture {
            Gesture::Swipe { way, .. } => {
                let bound = self.bound_actions();
                let situation = self.situation();
                // Where the view moves sideways, the finger moved it.
                let sideways = [Move::Motion(Motion::HalfLeft), Move::Motion(Motion::HalfRight)].map(|m| m.id());
                if bound.iter().any(|b| b.origin == Origin::View(kind) && sideways.contains(&b.action.id.as_str()) && b.action.when.holds(&situation)) {
                    return;
                }
                let runs = |b: &&Bound| b.runs_on(way.name()) && b.action.when.holds(&situation);
                // The closest one to where you are.
                let found = bound.iter().filter(runs).min_by_key(|b| b.layer);
                if let Some(b) = found {
                    self.asked = Some((b.action.id.clone(), None));
                }
            }
            // On a mark in the gutter: its notes, said.
            Gesture::LongPress(p) if self.note_marks.iter().any(|(_, mark, _)| mark.contains(p)) => {
                self.note = self.note_marks.iter().find(|(_, mark, _)| mark.contains(p)).map(|(_, _, said)| said.clone());
            }
            Gesture::LongPress(p) if !own => {
                if let Some(key) = self.desk.view(kind).spots().into_iter().find(|s| s.rect.contains(p)).map(|s| s.key) {
                    self.desk.view_mut(kind).move_to(&key);
                    self.asked = Some(("palette.open".to_owned(), None));
                }
            }
            Gesture::LongPress(_) => self.desk.view_mut(kind).gesture(gesture),
        }
    }

    /// Runs the action a control asked for this frame.
    pub(in crate::app) fn run_asked(&mut self, ctx: &egui::Context) {
        if let Some((id, value)) = self.asked.take() {
            self.run_by_id(&id, value, ctx);
        }
    }
}
