//! The quick look: a buffer over a panel (a card, the inbox, the lead's
//! chat).

use super::*;
use crate::keymap::Host;
use crate::quick_look::PANE;

impl CockpitApp {
    /// Shows `buffer` in the quick look, over the focused panel (what it
    /// held is put back first); `key` opened it, if it's down.
    pub(super) fn look_at(&mut self, buffer: BufferId, key: Option<Chord>, ctx: &egui::Context) {
        self.close_quick_look();
        // It holds the keys: typing in a panel under it stops.
        if matches!(self.input_mode, InputMode::Insert { at: Host::Panel(_) }) {
            self.input_mode = InputMode::Normal;
        }
        let now = ctx.input(|i| i.time);
        let before = self.desk.used_buffer(buffer.kind);
        self.quick_look = Some(QuickLook::open(buffer, before, self.desk.panels.focused(), self.desk.panels.focused_kind(), key, now));
        // One of its own is told so before it reads anything.
        if buffer.id == PANE {
            self.desk.ensure(buffer);
            if let Some(own) = self.desk.buffer_mut(buffer) {
                own.as_card(true);
            }
            // (Then it reads what's shared: it may be asked to show
            // something before it's drawn.)
            self.desk.share_with(buffer);
        }
        self.use_buffer(buffer);
    }

    /// Talking to the agent from anywhere: the lead's chat in the quick
    /// look, typing.
    pub(super) fn talk(&mut self, ctx: &egui::Context) {
        let chat = BufferId::new(BufferKind::Chat, PANE);
        if self.quick_look.as_ref().map(|q| q.buffer) != Some(chat) {
            self.look_at(chat, None, ctx);
        }
        // (The lead's, whatever the look's chat showed: a sub-agent alone.)
        let lead = Target::Agent { slot: Some(harness_contracts::SlotId::new(harness_contracts::LEAD)), thread: None };
        let _ = self.desk.view_mut(BufferKind::Chat).show_target(&lead);
        self.input_mode = InputMode::Insert { at: Host::Look };
    }

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
        // Its own card, held on what it shows.
        self.look_at(BufferId::new(BufferKind::Spec, PANE), key, ctx);
        let _ = self.desk.view_mut(BufferKind::Spec).show_target(&Target::Component(id));
    }

    /// The desk's pop-up terminal: its own terminal buffer in the quick
    /// look (focused, it starts typing); shown already, hidden.
    pub(super) fn pop_terminal(&mut self, ctx: &egui::Context) {
        if self.popped_terminal() {
            self.close_quick_look();
        } else {
            self.look_at(BufferId::new(BufferKind::Terminal, PANE), None, ctx);
        }
    }

    /// The quick look shows the pop-up terminal.
    pub(super) fn popped_terminal(&self) -> bool {
        self.quick_look.as_ref().is_some_and(|q| q.buffer == BufferId::new(BufferKind::Terminal, PANE))
    }

    /// Closes the quick look: the buffer of its kind used before is used
    /// again, and typing in it stops.
    pub(super) fn close_quick_look(&mut self) {
        if let Some(look) = self.quick_look.take() {
            self.use_buffer(look.before);
            if matches!(self.input_mode, InputMode::Insert { at: Host::Look } | InputMode::Capture { at: Host::Look }) {
                self.input_mode = InputMode::Normal;
            }
        }
    }

    /// What it holds, in a panel: a buffer of its own becomes a new
    /// buffer of a panel (the nearest of its kind, else beside the focused
    /// one); another is shown where it is, or there. Typing in it goes on
    /// there.
    pub(super) fn quick_look_to_panel(&mut self) {
        let Some(look) = self.quick_look.take() else { return };
        let from = self.jump_here();
        let buffer = if look.own() {
            let buffer = self.desk.panels.new_buffer(look.kind());
            self.desk.rekey(look.buffer, buffer);
            buffer
        } else {
            look.buffer
        };
        self.use_buffer(buffer);
        self.desk.view_mut(buffer.kind).as_card(false);
        let panel = self.desk.panels.ensure_visible(buffer);
        self.desk.panels.focus(panel);
        if self.input_mode == (InputMode::Insert { at: Host::Look }) {
            self.input_mode = InputMode::Insert { at: Host::Panel(panel) };
        }
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
        let (panel, kind) = (look.panel, look.kind());
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
        let area = egui::Area::new(egui::Id::new("quick_look")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            Frame::NONE.fill(colors().panel).stroke(Stroke::new(theme.line_thin, colors().focus)).corner_radius(theme.radius_xl).inner_margin(crate::theme::margin(theme.space_m)).show(ui, |ui| {
                ui.set_min_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                ui.set_max_size(size - egui::vec2(2.0 * theme.space_m, 2.0 * theme.space_m));
                ui.label(RichText::new(format!("{} Quick look", egui_phosphor::regular::EYE)).strong().size(theme.text_label).color(colors().text_strong));
                self.render_view(Here { at: Host::Look, kind }, ui, true);
            });
        });
        // Over the panels: a tap outside closes it (a peek's key closes it
        // when let go).
        self.overlays.drawn_at(crate::overlay::Overlay::QuickLook, area.response.rect);
    }

}
