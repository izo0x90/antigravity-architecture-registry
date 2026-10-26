//! Hosts: a panel and the quick look hold a view and draw it the same
//! way: its top line (the panel's number, the menus about where it is,
//! its title and status, from `Hosted`; its settings under it), then its
//! content. A view draws no controls. Every control comes from the one
//! bound list of where it is (`shown_at`): its settings on the top line,
//! what you're on's under it (or in its row). A control shows the key its
//! binding has, and used, asks for its action: the app runs it once the
//! frame is drawn, the way its key runs it.
//!
//! Pointer and touch, the same in every view: a click or tap on an item
//! moves the cursor there (the panel is focused by it); a double click or
//! tap opens it (Enter); a long press lists what can be done (the action
//! list, with the cursor on it). The view only says where its items are.

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
        let shown = self.shown_at(here);
        // A panel not focused says no keys (they don't run it until it is).
        let groups: Vec<_> = self.groups(&shown, here).into_iter().map(|g| if is_focused { g } else { g.unkeyed() }).collect();
        let mine: Vec<Bound> = shown.into_iter().filter(|b| b.layer == Layer::Panel).map(|b| if is_focused { b } else { b.unkeyed() }).collect();
        let buttons = self.item_buttons_now();
        let tagged = self.notes_named(here);
        let number = match here.at {
            Host::Panel(panel) => Some(panel),
            Host::Look => None,
        };
        let top = crate::hosted::TopLine { number, focused: is_focused, groups: &groups };
        let mut cx = RunCx { net: &self.desk.net, ctx: &ctx, input_mode: &mut self.input_mode, at: here.at, note: &mut self.note, asked: &mut self.asked };
        let line = crate::hosted::header(ui, self.desk.view(kind), &top, &mine, &tagged, &mut cx);
        self.feedback.register_inspectable(ui, &format!("{}:top_line", here.at.name()), line);
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
        self.contents.push((here.at, content));
        // It has the keyboard while typing is in its host and nothing over
        // it takes the keys.
        let typing = self.input_mode == (InputMode::Insert { at: here.at }) && self.keys_reach(here.at);
        let room_under_waiting = match buttons {
            ItemButtons::Floating => ui.spacing().interact_size.y + crate::theme::look().space_s * 3.0,
            ItemButtons::Bar => 0.0,
        };
        let mut drawing = crate::buffer::Drawing { feedback: &mut self.feedback, focused: is_focused, leader_held, typing, fingers: self.fingers, room_under_waiting };
        let drawn = self.desk.in_use(kind);
        self.desk.on_screen(drawn);
        let (buffer, net) = self.desk.view_with_net(kind);
        let mut cx = RunCx { net, ctx: &ctx, input_mode: &mut self.input_mode, at: here.at, note: &mut self.note, asked: &mut self.asked };
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            // What it draws stays in its place, however wide (never over
            // its host's lines, or another panel).
            ui.set_clip_rect(content.intersect(ui.clip_rect()));
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
        let (here_key, item) = {
            let view = self.desk.view(kind);
            let key = view.here().or_else(|| view.cursor_key());
            let rect = key.as_deref().and_then(|key| view.spot_at(key));
            (key, rect)
        };
        let typing = matches!(self.input_mode, InputMode::Insert { .. });
        let where_ = self.here();
        let bound: Vec<Bound> = self.bound_actions().into_iter().filter(|b| b.layer == Layer::On && b.action.at().is_some_and(At::under_something) && self.applies(b, where_)).collect();
        // What waits, each under its own place (none said: what you're on).
        let mine = |b: &Bound| b.action.control.as_ref().is_none_or(|c| c.under.is_none() || c.under == here_key);
        let (yours, waiting): (Vec<Bound>, Vec<Bound>) = bound.into_iter().partition(|b| b.action.at() != Some(At::Waiting) || mine(b));
        let mut groups: Vec<(egui::Id, Option<egui::Rect>, Vec<Bound>)> = match typing {
            // Typing: the box's own under it, what waits on you where you
            // are still under that.
            true => {
                let (box_, here): (Vec<Bound>, Vec<Bound>) = yours.into_iter().filter(|b| b.action.at() != Some(At::Item)).partition(|b| b.action.at() == Some(At::Typing));
                vec![(egui::Id::new("item_controls"), self.desk.view(kind).typing_rect(), box_), (egui::Id::new("waiting_controls"), item, here)]
            }
            false => vec![(egui::Id::new("item_controls"), item, yours.into_iter().filter(|b| b.action.at() != Some(At::Typing)).collect())],
        };
        // The rest of what waits: each under its own. In a bar (one row
        // says what it's for), what you're on's; you're on none: the
        // first's, so what waits is always answerable.
        if self.item_bars.iter().all(|(h, _)| *h != host) {
            let view = self.desk.view(kind);
            for b in waiting {
                let Some(key) = b.action.control.as_ref().and_then(|c| c.under.clone()) else { continue };
                match groups.iter_mut().find(|(id, ..)| *id == egui::Id::new(("waiting_controls", &key))) {
                    Some((.., group)) => group.push(b),
                    None => groups.push((egui::Id::new(("waiting_controls", &key)), view.spot_at(&key), vec![b])),
                }
            }
        } else if !groups.iter().flat_map(|(.., g)| g).any(|b| b.action.at() == Some(At::Waiting)) {
            let first = waiting.first().and_then(|b| b.action.control.as_ref()?.under.clone());
            let first: Vec<Bound> = waiting.into_iter().filter(|b| b.action.control.as_ref().is_some_and(|c| c.under == first)).collect();
            if let Some((.., group)) = groups.last_mut() {
                group.extend(first);
            }
        }
        let look = crate::theme::look();
        let mut cx = RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, at: host, note: &mut self.note, asked: &mut self.asked };
        // In a bar: its host's row, the buttons then what they act on
        // (whether or not the item is on screen), all in the one row.
        if let Some(bar) = self.item_bars.iter().find(|(h, _)| *h == host).map(|(_, rect)| *rect) {
            let here: Vec<Bound> = groups.into_iter().flat_map(|(_, _, here)| here).collect();
            if here.is_empty() {
                return;
            }
            let what = self.desk.view(kind).target().map(|t| t.in_words());
            let shown = egui::Area::new(egui::Id::new("item_controls")).order(egui::Order::Foreground).fixed_pos(bar.min).show(ctx, |ui| {
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
            self.part_of_the_look(host, shown.response.rect);
            return;
        }
        // Floating: under what they're for (above it when there's no
        // room); it needs to be on screen.
        let mut drawn = Vec::new();
        for (id, anchor, here) in groups {
            let Some(anchor) = anchor.map(|r| r.intersect(bounds)).filter(|r| r.is_positive() && !here.is_empty()) else { continue };
            let size = ctx.memory(|m| m.area_rect(id)).map_or(egui::vec2(anchor.width(), look.text_title * 2.0), |r| r.size());
            let below = anchor.bottom() + look.space_s;
            let y = if below + size.y <= bounds.bottom() { below } else { (anchor.top() - look.space_s - size.y).max(bounds.top()) };
            let shown = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(egui::pos2(anchor.left(), y)).show(ctx, |ui| {
                Frame::NONE.fill(colors().overlay_raised).stroke(Stroke::new(look.line_thin, colors().border)).corner_radius(look.radius_m).inner_margin(crate::theme::margin(look.space_s)).show(ui, |ui| {
                    ui.set_max_width(anchor.width().max(look.quick_look_width / 3.0).min(bounds.width()));
                    ui.horizontal_wrapped(|ui| crate::actions::controls(ui, &here, &mut cx));
                });
            });
            drawn.push(shown.response.rect);
        }
        for rect in drawn {
            self.part_of_the_look(host, rect);
        }
    }

    /// Controls drawn for the quick look are part of it (a tap on one isn't
    /// outside it).
    fn part_of_the_look(&mut self, host: Host, rect: egui::Rect) {
        if host == Host::Look {
            self.overlays.drawn_at(crate::overlay::Overlay::QuickLook, rect);
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

    /// The view (what holds it, and its panel) at `pos`: the quick look's,
    /// over its panel; else the panel's. None over anything else.
    fn view_at(&self, ctx: &egui::Context, pos: egui::Pos2) -> Option<(Host, usize, BufferKind)> {
        if let Some(look) = &self.quick_look
            && ctx.memory(|m| m.area_rect(egui::Id::new("quick_look"))).is_some_and(|r| r.contains(pos))
        {
            return (ctx.layer_id_at(pos).map(|l| l.id) == Some(egui::Id::new("quick_look"))).then_some((Host::Look, look.panel, look.kind()));
        }
        if ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background) {
            return None;
        }
        let panel = self.panel_rects.iter().position(|r| r.contains(pos))?;
        Some((Host::Panel(panel), panel, self.desk.panels.kind(panel)))
    }

    fn on_gesture(&mut self, gesture: crate::touch::Gesture, ctx: &egui::Context) {
        use crate::touch::Gesture;
        let at = match gesture {
            Gesture::LongPress(p) | Gesture::Swipe { from: p, .. } => p,
        };
        let Some((host, panel, kind)) = self.view_at(ctx, at) else { return };
        // On the content of the view it's on, or a note's mark beside it (on
        // a top line, a finger held on a button says what it does, and
        // that's all).
        let on_mark = self.note_marks.iter().any(|(_, mark, _)| mark.contains(at));
        if !on_mark && !self.contents.iter().any(|(h, content)| *h == host && content.contains(at)) {
            return;
        }
        if self.quick_look.is_none() && self.desk.panels.focused() != panel {
            self.desk.panels.focus(panel);
            // Where you are moved: the list is gathered again.
            self.forget_frame();
        }
        let own = self.desk.view(kind).own_pointer();
        match gesture {
            Gesture::Swipe { way, .. } => {
                let bound = self.bound_actions();
                let here = self.here();
                // Where the view moves sideways, the finger moved it.
                let sideways = [Move::Motion(Motion::HalfLeft), Move::Motion(Motion::HalfRight)].map(|m| m.id());
                if bound.iter().any(|b| b.origin == Origin::View(kind) && sideways.contains(&b.action.id.as_str()) && self.applies(b, here)) {
                    return;
                }
                let runs = |b: &&Bound| b.runs_on(way.name()) && self.applies(b, here);
                // The closest one to where you are.
                let found = bound.iter().filter(runs).min_by_key(|b| b.layer);
                if let Some(b) = found {
                    self.asked = Some((b.action.id.clone(), None));
                }
            }
            // On a mark in the gutter: its notes, said.
            Gesture::LongPress(p) if self.note_marks.iter().any(|(_, mark, _)| mark.contains(p)) => {
                let said: Vec<&str> = self.note_marks.iter().filter(|(_, mark, _)| mark.contains(p)).map(|(_, _, said)| said.as_str()).collect();
                self.note = Some(said.join("\n"));
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
