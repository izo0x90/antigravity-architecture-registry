//! The quick look and the talk box.

use super::*;

impl CockpitApp {
    /// Opens the quick look on what the cursor is on, or closes it.
    pub(super) fn toggle_quick_look(&mut self, ctx: &egui::Context) {
        if let Some(open) = &self.quick_look {
            // Its own key, still down: repeating, not pressed again.
            if !open.held() {
                self.close_quick_look();
            }
            return;
        }
        let kind = self.desk.panels.focused_kind();
        if kind == BufferKind::Spec {
            self.note = Some("The spec shows the full card already.".into());
            return;
        }
        let Some(id) = self.desk.view(kind).target().and_then(|t| t.component().cloned()) else {
            self.note = Some("Nothing here to look at: put the cursor on a component.".into());
            return;
        };
        // The key that opened it, if it's down (a menu or the palette has none).
        let key = ctx.input(|i| {
            self.bound_actions().into_iter().find(|b| b.action.id == "quicklook.toggle").and_then(|b| b.keys.into_iter().filter(|seq| seq.len() == 1).map(|mut seq| seq.remove(0)).find(|chord| chord.key_down(i)))
        });
        let now = ctx.input(|i| i.time);
        self.quick_look = Some(QuickLook::open(BufferKind::Spec, self.desk.panels.focused(), kind, self.desk.used_buffer(BufferKind::Spec).id, key, now));
        // Its own spec view, held on the card.
        self.use_buffer(Buffer::new(BufferKind::Spec, crate::quick_look::PANE));
        self.desk.spec.pinned = true;
        self.desk.spec.floating = true;
        self.desk.spec.show(id);
    }

    /// Closes the quick look: the spec view used before is used again.
    pub(super) fn close_quick_look(&mut self) {
        if let Some(look) = self.quick_look.take() {
            self.use_buffer(Buffer::new(BufferKind::Spec, look.previous_spec));
        }
    }

    /// Its card in a panel: it becomes that panel's spec view (the
    /// nearest spec panel's, else beside the focused panel).
    pub(super) fn quick_look_to_panel(&mut self) {
        if self.quick_look.take().is_none() {
            return;
        }
        let from = self.jump_here();
        let buffer = self.desk.panels.new_buffer(BufferKind::Spec);
        self.desk.spec.floating = false;
        self.desk.active.insert(BufferKind::Spec, buffer.id);
        self.desk.last_used.insert(BufferKind::Spec, buffer.id);
        let panel = self.desk.panels.ensure_visible(buffer);
        self.desk.panels.focus(panel);
        self.after_jump(from);
    }

    /// A peek closes when its key is let go; the quick look closes when
    /// focus moved to a panel (a go-to landed there).
    pub(super) fn quick_look_tick(&mut self, ctx: &egui::Context) {
        let Some(look) = &mut self.quick_look else { return };
        let moved = self.desk.panels.focused() != look.panel || self.desk.panels.kind(look.panel) != look.over;
        let hold = self.config.keymap.peek_hold_ms as f64 / 1000.0;
        let (released, now) = ctx.input(|i| (look.key().is_some_and(|k| k.released(i)), i.time));
        if moved || (released && look.let_go(now, hold)) {
            self.close_quick_look();
        }
    }

    /// The quick look, over the panel it was opened from: a host like a
    /// panel, holding its view (the card's spec view, or the inbox), its
    /// own controls on its header.
    pub(super) fn render_quick_look(&mut self, ctx: &egui::Context) {
        let Some(look) = &self.quick_look else { return };
        let (panel, kind) = (look.panel, look.kind);
        let over = self.panel_rects.get(panel).copied().filter(|r| r.is_positive()).unwrap_or_else(|| ctx.content_rect());
        let screen = ctx.content_rect();
        let theme = crate::theme::look();
        let size = egui::vec2(over.width().max(screen.width().min(theme.quick_look_width)).min(screen.width() - 2.0 * theme.space_l), screen.height() * theme.quick_look_height);
        // On the side away from the panel it's about, so that panel stays
        // in sight; with one panel there's no other side: over it.
        let (left, right) = (screen.left() + theme.space_l, (screen.right() - size.x - theme.space_l).max(screen.left()));
        let x = match self.desk.panels.len() {
            1 => (over.center().x - size.x / 2.0).clamp(left, right),
            _ if over.center().x > screen.center().x => left,
            _ => right,
        };
        let pos = egui::pos2(x, screen.center().y - size.y / 2.0);
        // Its own controls from the one list (where you are: it).
        let own: Vec<Bound> = self.bound_actions().into_iter().filter(|b| b.layer == Layer::Host).collect();
        let area = egui::Area::new(egui::Id::new("quick_look")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE.fill(colors().panel).stroke(Stroke::new(theme.line_thin, colors().focus)).corner_radius(theme.radius_xl).inner_margin(crate::theme::margin(theme.space_m)).show(ui, |ui| {
                ui.set_min_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                ui.set_max_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} Quick look", egui_phosphor::regular::EYE)).strong().size(theme.text_label).color(colors().text_strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.host_controls(ui, &own, panel));
                });
                self.render_view(Here { panel, kind, look: true }, ui, true);
            });
        });
        // A click outside closes it (a peek's key closes it when let go).
        if ctx.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p))) {
            self.close_quick_look();
        }
    }

    /// The box for talking to the agent: it has the keyboard while open.
    pub(super) fn render_talk(&mut self, ctx: &egui::Context) {
        let Some(talk) = &mut self.talk else { return };
        if talk.render(ctx, &mut self.desk.chat, &self.desk.net) == crate::talk::Outcome::Close {
            self.talk = None;
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
        }
    }
}
