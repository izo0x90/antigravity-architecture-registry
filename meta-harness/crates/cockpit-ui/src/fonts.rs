//! The faces the cockpit draws text in: egui's own, led by the font packs
//! the config names, fetched from the UI server (where they're installed)
//! once each time the config's choice changes.

use std::sync::{Arc, Mutex};

use crate::config::FontsUi;
use crate::net::{NetClient, NetResult};
use crate::theme::{Face, Faces};

/// Which family a face leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lead {
    Mono,
    Text,
}

type Inbox = Arc<Mutex<Vec<(Lead, String, NetResult<Vec<u8>>)>>>;

#[derive(Default)]
pub struct Fonts {
    /// The choice asked for (none: not yet).
    asked: Option<FontsUi>,
    faces: Faces,
    inbox: Inbox,
}

impl Fonts {
    /// Fetches what `wanted` names when it changed, and puts faces in as
    /// they come. Returns what to tell the user (a face not to be had).
    pub fn tend(&mut self, wanted: &FontsUi, net: &NetClient, ctx: &egui::Context) -> Option<String> {
        if self.asked.as_ref() != Some(wanted) {
            self.asked = Some(wanted.clone());
            self.faces = Faces::default();
            ctx.set_fonts(crate::theme::font_definitions(&self.faces));
            for (lead, name) in [(Lead::Mono, &wanted.mono), (Lead::Text, &wanted.text)] {
                if name.is_empty() {
                    continue;
                }
                let (inbox, named) = (self.inbox.clone(), name.clone());
                net.font_face(name, "regular", ctx.clone(), move |r| {
                    if let Ok(mut i) = inbox.lock() {
                        i.push((lead, named, r));
                    }
                });
            }
        }
        let arrived = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        let mut note = None;
        let mut changed = false;
        for (lead, name, result) in arrived {
            // An answer for a choice since changed: passed over.
            let still = self.asked.as_ref().is_some_and(|a| match lead {
                Lead::Mono => a.mono == name,
                Lead::Text => a.text == name,
            });
            if !still {
                continue;
            }
            match result {
                Ok(bytes) => {
                    let face = Some(Face { name: name.clone(), bytes: Arc::new(bytes) });
                    match lead {
                        Lead::Mono => self.faces.mono = face,
                        Lead::Text => self.faces.text = face,
                    }
                    changed = true;
                }
                Err(e) => note = Some(format!("The font {name} can't be had, so egui's own is used: {}", String::from(e))),
            }
        }
        if changed {
            ctx.set_fonts(crate::theme::font_definitions(&self.faces));
        }
        note
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configs_face_is_fetched_once_put_first_and_a_missing_one_said() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let mut fonts = Fonts::default();
        let wanted = FontsUi { mono: "jetbrains-mono".into(), text: String::new() };
        assert_eq!(fonts.tend(&wanted, &net, &ctx), None);
        assert_eq!(net.sent.lock().unwrap().as_slice(), ["GET /api/ui/fonts/jetbrains-mono/regular"], "asked once, the text face left egui's");
        fonts.tend(&wanted, &net, &ctx);
        assert_eq!(net.sent.lock().unwrap().len(), 1, "not again while the choice is the same");
        // A real face's bytes (egui's own, standing in for the pack's).
        let hack = egui::FontDefinitions::default().font_data["Hack"].font.to_vec();
        net.answer_bytes("GET /api/ui/fonts/jetbrains-mono/regular", 200, hack);
        fonts.tend(&wanted, &net, &ctx);
        let defs = crate::theme::font_definitions(&fonts.faces);
        assert_eq!(defs.families[&egui::FontFamily::Monospace][0], "jetbrains-mono", "it leads monospace");
        assert!(defs.families[&egui::FontFamily::Monospace].iter().any(|f| f == "phosphor"), "icons still there");
        // Another choice, not installed: said, and egui's own is used.
        let other = FontsUi { mono: "maple-mono".into(), text: String::new() };
        fonts.tend(&other, &net, &ctx);
        net.answer("GET /api/ui/fonts/maple-mono/regular", 404, &serde_json::json!({ "error": "maple-mono isn't installed (meta-harness pack install maple-mono)" }));
        let note = fonts.tend(&other, &net, &ctx).unwrap();
        assert!(note.contains("pack install maple-mono"), "{note}");
        assert!(fonts.faces.mono.is_none());
    }
}
