//! The shell's one stack of what's over the panels (`crate::overlay`):
//! which are open (each one's own state, read), closing one, and its rules
//! applied: a tap outside the top one, before the frame takes it; the way
//! back out; keys kept from what's under a modal one.

use super::*;
use crate::overlay::Overlay;

impl CockpitApp {
    /// Whether `overlay` is up.
    pub(super) fn overlay_open(&self, overlay: Overlay) -> bool {
        match overlay {
            Overlay::QuickLook => self.quick_look.is_some(),
            Overlay::Inspector => self.feedback.is_inspecting,
            Overlay::Feedback => self.feedback.is_modal_open,
            Overlay::Issues => self.feedback.is_issues_window_open,
            Overlay::Hints => matches!(self.capture, Some(Capture::Hint(_))),
            Overlay::Keys => !self.pending_keys.is_empty() || self.pending_count.is_some(),
            Overlay::Menu => self.input_mode == InputMode::Leader,
            Overlay::Palette => self.palette.is_open(),
        }
    }

    /// Closes `overlay`, as its own way out does; the keyboard goes back
    /// where it was.
    pub(super) fn close_overlay(&mut self, overlay: Overlay, ctx: &egui::Context) {
        match overlay {
            Overlay::QuickLook => self.close_quick_look(),
            Overlay::Inspector => self.feedback.is_inspecting = false,
            Overlay::Feedback => self.feedback.is_modal_open = false,
            Overlay::Issues => self.feedback.is_issues_window_open = false,
            Overlay::Hints => {
                self.on_capture(&[], true, ctx);
                self.capture = None;
                self.input_mode = InputMode::Normal;
            }
            Overlay::Keys => (self.pending_keys, self.pending_count) = (Vec::new(), None),
            Overlay::Menu => self.leave_menu(),
            Overlay::Palette => {
                self.palette.close();
                self.palette_closed(ctx);
            }
        }
        self.forget_frame();
    }

    /// The one on top, of those up.
    pub(super) fn top_overlay(&self) -> Option<Overlay> {
        self.overlays.top(|o| self.overlay_open(o))
    }

    /// Before a frame takes its input: a tap outside what's on top closes
    /// it, and goes no further unless it lets taps through.
    pub(crate) fn before_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // A setting's list of choices, open over one: it closes itself on
        // a tap outside it (egui's own).
        if matches!(self.input_mode, InputMode::Choosing { .. }) {
            return;
        }
        let open: Vec<Overlay> = crate::overlay::ALL.into_iter().filter(|o| self.overlay_open(*o)).collect();
        if let Some(overlay) = self.overlays.outside_tap(raw, |o| open.contains(&o)) {
            self.close_overlay(overlay, ctx);
        }
    }

    /// The way back out, pressed: the top one closes when it doesn't step
    /// back through its own first (true: it was taken). And while a modal
    /// one is up, what's under it takes no keys (true: none go on).
    pub(super) fn overlay_keys(&mut self, back: bool, ctx: &egui::Context) -> bool {
        let Some(top) = self.top_overlay() else { return false };
        let rules = top.rules();
        if rules.own_back {
            return false;
        }
        if back {
            self.close_overlay(top, ctx);
            return true;
        }
        rules.modal
    }

    /// The menus opened: from where (given back when they're left).
    pub(super) fn menus_from(&mut self, from: InputMode) {
        self.menu_from = match from {
            InputMode::Leader => self.menu_from,
            InputMode::Insert { .. } => from,
            _ => InputMode::Normal,
        };
    }

    /// The menus left without running anything: the keyboard back where it
    /// was when they opened.
    pub(super) fn leave_menu(&mut self) {
        self.leader_path.clear();
        self.input_mode = std::mem::replace(&mut self.menu_from, InputMode::Normal);
    }
}
