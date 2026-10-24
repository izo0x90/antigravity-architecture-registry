//! The faces the cockpit draws text in: egui's own, led by the font packs
//! the config names, fetched from the UI server (where they're installed).
//! The one owner of the context's fonts: set at start (egui's own and the
//! icons), then again once every face the config names has answered, and
//! only when what leads changed.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use crate::config::FontsUi;
use crate::net::{NetClient, NetResult};
use crate::theme::{Face, Faces};

type Inbox = Arc<Mutex<Vec<(String, NetResult<Vec<u8>>)>>>;

/// Which pack leads monospace, and which the rest.
type Leads = (Option<String>, Option<String>);

#[derive(Default)]
pub struct Fonts {
    /// What the config names, as last applied.
    wanted: FontsUi,
    /// The faces fetched, by pack: only those named now, each once
    /// whichever families it leads.
    have: BTreeMap<String, Arc<egui::FontData>>,
    /// Asked for, not answered yet.
    waiting: BTreeSet<String>,
    /// What the context's fonts were last built from (none: never).
    built: Option<Leads>,
    inbox: Inbox,
}

impl Fonts {
    /// Before the first frame: egui's own and the icons.
    pub fn start(&mut self, ctx: &egui::Context) {
        self.settle(ctx);
    }

    /// The config's choice, when the config changed: what it names and
    /// isn't had is fetched; what's had is kept.
    pub fn want(&mut self, wanted: &FontsUi, net: &NetClient, ctx: &egui::Context) {
        self.wanted = wanted.clone();
        let named: BTreeSet<String> = [&wanted.mono, &wanted.text].into_iter().filter(|n| !n.is_empty()).cloned().collect();
        self.have.retain(|name, _| named.contains(name));
        self.waiting.retain(|name| named.contains(name));
        for name in named {
            if self.have.contains_key(&name) || !self.waiting.insert(name.clone()) {
                continue;
            }
            let inbox = self.inbox.clone();
            net.font_face(&name.clone(), ctx.clone(), move |r| {
                if let Ok(mut i) = inbox.lock() {
                    i.push((name, r));
                }
            });
        }
        self.settle(ctx);
    }

    /// Takes the faces that came. Returns what to tell the user (a face
    /// not to be had).
    pub fn tend(&mut self, ctx: &egui::Context) -> Option<String> {
        let arrived = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        if arrived.is_empty() {
            return None;
        }
        let mut note = None;
        for (name, result) in arrived {
            // Not named any more (or answered already): passed over.
            if !self.waiting.remove(&name) {
                continue;
            }
            match result {
                Ok(bytes) => {
                    self.have.insert(name, Arc::new(egui::FontData::from_owned(bytes)));
                }
                Err(e) => note = Some(format!("The font {name} can't be had, so egui's own is used: {}", String::from(e))),
            }
        }
        self.settle(ctx);
        note
    }

    /// Once nothing named is still coming: the fonts built from what's
    /// had, if that's not what they're built from already (one rebuild
    /// for all the faces, none when nothing changed).
    fn settle(&mut self, ctx: &egui::Context) {
        if !self.waiting.is_empty() {
            return;
        }
        let lead = |name: &String| self.have.get(name).map(|data| Face { name: name.clone(), data: data.clone() });
        let faces = Faces { mono: lead(&self.wanted.mono), text: lead(&self.wanted.text) };
        let leads = (faces.mono.as_ref().map(|f| f.name.clone()), faces.text.as_ref().map(|f| f.name.clone()));
        if self.built.as_ref() != Some(&leads) {
            ctx.set_fonts(crate::theme::font_definitions(&faces));
            self.built = Some(leads);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whose face leads each family in the fonts the context has now.
    fn leading(ctx: &egui::Context) -> (String, String) {
        // The fonts set are taken at the start of the next pass.
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let first = |family| ctx.fonts(|f| f.definitions().families[&family][0].clone());
        (first(egui::FontFamily::Monospace), first(egui::FontFamily::Proportional))
    }

    #[test]
    fn the_configs_faces_are_fetched_once_each_put_first_together_and_a_missing_one_said() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let sent = || net.sent.lock().unwrap().clone();
        let mut fonts = Fonts::default();
        fonts.start(&ctx);
        let defaults = leading(&ctx);
        assert_eq!(defaults.0, "Hack", "egui's own to start with");
        // One pack for both families: asked once.
        let both = FontsUi { mono: "jetbrains-mono".into(), text: "jetbrains-mono".into() };
        fonts.want(&both, &net, &ctx);
        assert_eq!(sent(), ["GET /api/ui/fonts/jetbrains-mono"], "one pack, one fetch");
        // A real face's bytes (egui's own, standing in for the pack's).
        let hack = egui::FontDefinitions::default().font_data["Hack"].font.to_vec();
        net.answer_bytes("GET /api/ui/fonts/jetbrains-mono", 200, hack.clone());
        assert_eq!(fonts.tend(&ctx), None);
        assert_eq!(leading(&ctx), ("jetbrains-mono".into(), "jetbrains-mono".into()), "it leads both");
        assert!(ctx.fonts(|f| f.definitions().families[&egui::FontFamily::Monospace].iter().any(|f| f == "phosphor")), "icons still there");

        // Another text face: the mono one kept (not fetched again), the
        // fonts built once the new one's in, not before.
        let other = FontsUi { mono: "jetbrains-mono".into(), text: "geist-mono".into() };
        fonts.want(&other, &net, &ctx);
        assert_eq!(sent().len(), 2, "only the new face is fetched: {:?}", sent());
        assert_eq!(leading(&ctx).1, "jetbrains-mono", "nothing rebuilt while a face is coming");
        net.answer_bytes("GET /api/ui/fonts/geist-mono", 200, hack);
        fonts.tend(&ctx);
        assert_eq!(leading(&ctx), ("jetbrains-mono".into(), "geist-mono".into()));

        // One not installed: said, and egui's own leads where it was named.
        let missing = FontsUi { mono: "maple-mono".into(), text: String::new() };
        fonts.want(&missing, &net, &ctx);
        net.answer("GET /api/ui/fonts/maple-mono", 404, &serde_json::json!({ "error": "pack 'maple-mono' isn't installed (meta-harness pack install maple-mono)" }));
        let note = fonts.tend(&ctx).unwrap();
        assert!(note.contains("pack install maple-mono"), "{note}");
        assert_eq!(leading(&ctx), defaults);
    }
}
