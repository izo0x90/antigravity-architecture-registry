//! What buffers ask of the rest of the cockpit: taken from every buffer
//! the same way, and done here, whichever buffer asked.

use super::*;
use crate::nav::Ask;

impl CockpitApp {
    /// Takes the asks of every buffer in use and does them (each frame, and
    /// after every action). What they changed is drawn (and read by the
    /// buffers) in the next frame, asked for now.
    pub(super) fn take_asks(&mut self, ctx: &egui::Context) {
        for kind in BufferKind::ALL {
            for ask in self.desk.view_mut(kind).take_asks() {
                self.do_ask(kind, ask, ctx);
                ctx.request_repaint();
            }
        }
    }

    fn do_ask(&mut self, from: BufferKind, ask: Ask, ctx: &egui::Context) {
        match ask {
            Ask::Show { target, in_ } => {
                let jump = self.jump_of(from);
                self.land(in_, &target, jump, ctx);
            }
            // The buffers in use (each kind's used last) follow the user's
            // moves; a parked one keeps its own place.
            Ask::Follow(id) => {
                for kind in BufferKind::ALL {
                    self.desk.view_mut(kind).follow(&id);
                }
            }
            Ask::Reread(file) => self.reread(file, ctx),
            // Typing where it is: the quick look, while it holds it; else
            // its panel, focused.
            Ask::Type => {
                let at = match &self.quick_look {
                    Some(look) if look.kind() == from => crate::keymap::Host::Look,
                    _ => {
                        self.focus_kind(from);
                        crate::keymap::Host::Panel(self.desk.panels.focused())
                    }
                };
                self.input_mode = InputMode::Insert { at };
            }
            Ask::Refresh(crate::nav::Refresh::Review) => self.desk.shared.review.invalidate(),
            Ask::Refresh(crate::nav::Refresh::Changes) => self.desk.shared.changes.invalidate(),
            Ask::Compare(comparison) => self.desk.shared.changes.compare(comparison),
            Ask::Source(source) => self.desk.shared.source.set(source),
            Ask::Located { id, anchor } => self.desk.each_buffer(|b| b.located(&id, anchor.as_ref())),
            Ask::Waiting(waiting) => self.do_waiting(waiting, ctx),
            Ask::Agents { slot, act } => self.desk.shared.agents.get_mut().act(slot, act, &self.desk.net, ctx),
            Ask::Terminals { id, act } => self.desk.shared.terminals.get_mut().act(id, act, &self.desk.net, ctx),
            Ask::Tags(act) => self.desk.shared.tags.get_mut().act(act, &self.desk.net, ctx, &self.notes_later),
        }
    }

    /// Reads `file` into the index again (a failure is said with the index's).
    fn reread(&mut self, file: String, ctx: &egui::Context) {
        let notes = self.desk.index_inbox.clone();
        self.desk.net.arch_refresh(&[file], ctx.clone(), move |r| {
            if let Err(e) = r
                && let Ok(mut i) = notes.lock()
            {
                i.push(Err(e));
            }
        });
    }
}
