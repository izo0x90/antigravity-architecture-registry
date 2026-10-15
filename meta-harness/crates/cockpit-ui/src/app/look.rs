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
        let kind = self.desk.columns.focused_kind();
        if kind == BufferKind::Spec {
            self.note = Some("The spec view shows the full card already.".into());
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
        self.quick_look = Some(QuickLook::open(crate::quick_look::Look::Card, self.desk.columns.focused(), kind, self.desk.used_pane(BufferKind::Spec).id, key, now));
        // Its own spec view, held on the card.
        self.use_pane(Pane::new(BufferKind::Spec, crate::quick_look::PANE));
        self.desk.spec.pinned = true;
        self.desk.spec.floating = true;
        self.desk.spec.show(id);
    }

    /// Closes the quick look: the spec view used before is used again.
    pub(super) fn close_quick_look(&mut self) {
        if let Some(look) = self.quick_look.take() {
            self.use_pane(Pane::new(BufferKind::Spec, look.previous_spec));
        }
    }

    /// Its card in a column: it becomes that column's spec view (the
    /// nearest spec column's, else beside the focused column).
    pub(super) fn quick_look_to_column(&mut self) {
        if self.quick_look.take().is_none() {
            return;
        }
        let from = self.jump_here();
        let pane = self.desk.columns.new_pane(BufferKind::Spec);
        self.desk.spec.floating = false;
        self.desk.active.insert(BufferKind::Spec, pane.id);
        self.desk.last_used.insert(BufferKind::Spec, pane.id);
        let slot = self.desk.columns.ensure_visible(pane);
        self.desk.columns.focus(slot);
        self.after_jump(from);
    }

    /// A peek closes when its key is let go; the quick look closes when
    /// focus moved to a column (a go-to landed there).
    pub(super) fn quick_look_tick(&mut self, ctx: &egui::Context) {
        let Some(look) = &mut self.quick_look else { return };
        let moved = self.desk.columns.focused() != look.slot || self.desk.columns.kind(look.slot) != look.over;
        let hold = self.config.keymap.peek_hold_ms as f64 / 1000.0;
        let (released, now) = ctx.input(|i| (look.key().is_some_and(|k| k.released(i)), i.time));
        if moved || (released && look.let_go(now, hold)) {
            self.close_quick_look();
        }
    }

    /// The quick look, over the column it was opened from: what the graph
    /// says about the card, then the card in full (the spec view).
    pub(super) fn render_quick_look(&mut self, ctx: &egui::Context, shell: &[Action]) {
        let Some(look) = &self.quick_look else { return };
        let (slot, shows) = (look.slot, look.look.clone());
        let over = self.slot_rects.get(slot).copied().filter(|r| r.is_positive()).unwrap_or_else(|| ctx.content_rect());
        let screen = ctx.content_rect();
        let theme = crate::theme::look();
        let size = egui::vec2(over.width().max(screen.width().min(theme.quick_look_width)).min(screen.width() - 2.0 * theme.space_l), screen.height() * theme.quick_look_height);
        let pos = egui::pos2((over.center().x - size.x / 2.0).clamp(screen.left() + theme.space_l, (screen.right() - size.x - theme.space_l).max(screen.left())), screen.center().y - size.y / 2.0);
        let close_hint = self.bound_actions().into_iter().find(|b| b.action.id == "quicklook.toggle").map(|b| b.key_hint()).unwrap_or_default();
        let column_hint = self.bound_actions().into_iter().find(|b| b.action.id == "quicklook.to_column").map(|b| b.key_hint()).unwrap_or_default();
        let (mut to_column, mut close) = (false, false);
        let area = egui::Area::new(egui::Id::new("quick_look")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE.fill(colors().panel).stroke(Stroke::new(theme.line_thin, colors().focus)).corner_radius(theme.radius_xl).inner_margin(crate::theme::margin(theme.space_m)).show(ui, |ui| {
                ui.set_min_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                ui.set_max_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                let crate::quick_look::Look::Attention(id) = shows else {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{} Quick look", egui_phosphor::regular::EYE)).strong().size(theme.text_label).color(colors().text_strong));
                        ui.label(RichText::new(format!("{close_hint} or Esc closes · hold {close_hint} to peek")).size(theme.text_small).color(colors().muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close |= ui.small_button(egui_phosphor::regular::X).on_hover_text("Close").clicked();
                            to_column |= ui.button(format!("{} Open in a column", egui_phosphor::regular::COLUMNS)).on_hover_text(column_hint).clicked();
                        });
                    });
                    if let Some(id) = self.desk.spec.target.clone() {
                        self.desk.graph.render_card_context(ui, &id);
                    }
                    ui.separator();
                    let mut cx = RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell };
                    self.desk.spec.render(ui, &mut cx);
                    return;
                };
                close |= self.render_attention(ui, &id);
            });
        });
        // A click outside closes it (a peek's key closes it when let go).
        let outside = ctx.input(|i| i.pointer.primary_clicked() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
        if to_column {
            self.quick_look_to_column();
        } else if close || outside {
            self.close_quick_look();
        }
    }

    /// The box for talking to the agent: it has the keyboard while open.
    pub(super) fn render_talk(&mut self, ctx: &egui::Context) {
        let Some(talk) = &mut self.talk else { return };
        let close = self.config.keymap.back.label();
        if talk.render(ctx, &mut self.desk.chat, &self.desk.net, &close) == crate::talk::Outcome::Close {
            self.talk = None;
            self.input_mode = InputMode::Normal;
            release_keyboard(ctx);
        }
    }
}
