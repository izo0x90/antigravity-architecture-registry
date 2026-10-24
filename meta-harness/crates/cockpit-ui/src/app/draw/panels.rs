//! Each panel's frame, and the palette (the action list and the pickers
//! for an action's value).

use super::super::*;
use crate::keymap::Host;

impl CockpitApp {
    pub fn render_panel(&mut self, panel: usize, ui: &mut egui::Ui) {
        let is_focused = self.desk.panels.focused() == panel;
        let border_color = if is_focused { colors().focus } else { colors().border };

        let panel_id = format!("panel_{}", panel + 1);
        self.feedback.register_inspectable(ui, &panel_id, ui.available_rect_before_wrap());

        Frame::NONE.fill(colors().bg).stroke(Stroke::new(crate::theme::look().line_thin, border_color)).corner_radius(crate::theme::look().radius_m).inner_margin(crate::theme::margin(crate::theme::look().space_m)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(ui.available_height());

            ui.vertical(|ui| {
                let active_kind = self.desk.panels.kind(panel);
                self.render_view(Here { at: Host::Panel(panel), kind: active_kind }, ui, is_focused);
            });
        });

        if let Some(rect) = self.panel_rects.get_mut(panel) {
            *rect = ui.min_rect();
        }
        // A click on it focuses it (one on what's over it, a menu or the
        // action list, is theirs); where you are moved, what was gathered
        // for the frame is gathered again.
        if ui.input(|i| i.pointer.primary_clicked())
            && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            && ui.min_rect().contains(pos)
            && ui.ctx().layer_id_at(pos) == Some(ui.layer_id())
            && self.desk.panels.focused() != panel
        {
            self.desk.panels.focus(panel);
            self.forget_frame();
        }
    }

    /// The palette overlay (opened by its key, or its control on the top
    /// bar).
    pub fn render_palette(&mut self, ctx: &egui::Context) {
        // A search left (picked, gone back from, closed): its owner is told,
        // so the next opens with nothing of it.
        let opening = self.palette.opening();
        if self.searched.as_ref().map(|(origin, id, at)| (*origin, id.as_str(), *at)) != self.palette.searching().map(|(origin, id)| (origin, id, opening)) {
            let now = self.palette.searching().map(|(origin, id)| (origin, id.to_owned(), opening));
            if let Some((origin, id, _)) = std::mem::replace(&mut self.searched, now) {
                self.search_done(origin, &id);
            }
        }
        // A view asked for an action's picker (e.g. Resume once sessions loaded).
        let requests: Vec<String> = BufferKind::ALL.into_iter().filter_map(|kind| self.desk.view_mut(kind).take_palette_request()).collect();
        // Closed and not asked for: nothing to list (what it lists isn't
        // gathered, or judged).
        if !self.palette.is_open() && requests.is_empty() && self.input_mode != InputMode::Palette {
            return;
        }
        let bound = self.palette_actions();
        // (Only one that's offered there now: none of them, nothing opens.)
        for id in requests {
            if let Some(action) = bound.iter().find(|b| b.action.id == id && b.action.when != crate::actions::When::Unavailable) {
                self.open_palette(action);
            }
        }
        let (offered, sheet) = self.sheet_of(bound);
        let picked = self.palette.render(ctx, &offered, &sheet, &self.config.keymap);
        // A trigger in what's written there: its picker, what it picks put after it.
        if let Some(pick) = self.palette.take_trigger().and_then(|c| self.triggered(c)) {
            self.palette.hold_for(&pick);
            self.palette.give_back_typing();
        }
        if let Some(ask) = self.palette.take_search() {
            match ask.origin {
                Origin::View(kind) => self.desk.view_mut(kind).search(&ask.action, &ask.query),
                Origin::Shell => {
                    self.desk.shell_searches.insert(ask.action, ask.query);
                }
            }
        }
        // A view searched from here and not drawn (not ticked): its search
        // alone is tended while it's open.
        if let Some((Origin::View(kind), _)) = self.palette.searching()
            && !self.desk.shown_now(kind)
        {
            let (buffer, net) = self.desk.view_with_net(kind);
            buffer.tend_search(net, ctx);
        }
        if let Some(invocation) = picked {
            // Still open: the picker changed what it acts on (the cycle key).
            if !self.palette.is_open() {
                self.input_mode = InputMode::Normal;
                release_keyboard(ctx);
            }
            self.invoke(invocation, ctx);
        }
        // Closed without a pick: back to typing, when that's where it was;
        // a step-by-step start or edit left there is gone.
        if self.input_mode == InputMode::Palette && !self.palette.is_open() {
            // A trigger's picker left: what was typed in it goes after the trigger.
            if let Some(typed) = self.palette.take_given_back()
                && let InputMode::Insert { at } = self.palette_from
                && let Some(buffer) = self.buffer_at(at)
                && let Some(writing) = self.desk.buffer_mut(buffer).and_then(|b| b.typing_box())
            {
                writing.put(&typed);
            }
            (self.desk.discussing, self.desk.tag_editing) = (None, None);
            self.desk.discussions_here.clear();
            self.input_mode = match self.palette_from {
                typing @ InputMode::Insert { .. } => typing,
                _ => {
                    release_keyboard(ctx);
                    InputMode::Normal
                }
            };
        }
    }

    /// The search of `action` was left: what it looked for is done with
    /// (a view's, or the shell's).
    fn search_done(&mut self, origin: Origin, action: &str) {
        match origin {
            Origin::View(kind) => self.desk.view_mut(kind).search_done(action),
            Origin::Shell => {
                self.desk.shell_searches.remove(action);
                if action == crate::app::linking::LINK_THING {
                    self.desk.finding.clear();
                }
            }
        }
    }
}
