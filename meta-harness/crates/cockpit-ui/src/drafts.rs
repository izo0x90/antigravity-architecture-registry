//! The chats' unsent text, kept by the server as its own record, by slot:
//! not part of the layout, so a layout reset (another version, another
//! browser saving over it) doesn't lose what was typed. Read once when the
//! desk opens; a chat view takes its slot's when it's empty; each slot's
//! text is saved a moment after it last changed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use harness_contracts::SlotId;

use crate::net::{NetClient, NetResult};

enum Reply {
    Loaded(NetResult<BTreeMap<String, String>>),
    Saved(SlotId, NetResult<()>),
}

#[derive(Default)]
pub struct Drafts {
    /// Read from the server (none: not yet).
    loaded: Option<BTreeMap<SlotId, String>>,
    asked: bool,
    /// Each slot's text as last sent, and as it is now with when it last
    /// changed (sent once it's been still a moment).
    sent: BTreeMap<SlotId, String>,
    pending: BTreeMap<SlotId, (String, f64)>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

impl Drafts {
    /// Asks for the saved drafts (again, after the server was away).
    pub fn load(&mut self, net: &NetClient, ctx: &egui::Context) {
        if self.loaded.is_some() || self.asked {
            return;
        }
        self.asked = true;
        let inbox = self.inbox.clone();
        net.drafts(ctx.clone(), move |r| push(&inbox, Reply::Loaded(r)));
    }

    pub fn reconnected(&mut self) {
        if self.loaded.is_none() {
            self.asked = false;
        }
    }

    /// What was saved for `slot` (none: not read yet; some none: nothing
    /// saved for it).
    pub fn saved(&self, slot: &SlotId) -> Option<Option<&str>> {
        Some(self.loaded.as_ref()?.get(slot).map(String::as_str))
    }

    /// A chat view's text for `slot` changed: saved once it's been still a
    /// moment (nothing is saved before what was saved is read).
    pub fn changed(&mut self, slot: SlotId, text: String, time: f64) {
        if self.loaded.is_none() || self.sent.get(&slot).map_or("", String::as_str) == text {
            self.pending.remove(&slot);
            return;
        }
        if self.pending.get(&slot).is_none_or(|(waiting, _)| *waiting != text) {
            self.pending.insert(slot, (text, time));
        }
    }

    /// Takes replies, and sends what's been still for `after` seconds.
    /// Returns what to tell the user.
    pub fn tick(&mut self, time: f64, after: f64, net: &NetClient, ctx: &egui::Context) -> Vec<String> {
        let mut notes = Vec::new();
        for reply in std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                Reply::Loaded(Ok(found)) => {
                    let found: BTreeMap<SlotId, String> = found.into_iter().map(|(slot, text)| (SlotId::new(&slot), text)).collect();
                    self.sent = found.clone();
                    self.loaded = Some(found);
                }
                Reply::Loaded(Err(e)) => {
                    self.asked = false;
                    notes.push(format!("Couldn't read the unsent chat text: {}", String::from(e)));
                }
                Reply::Saved(_, Ok(())) => {}
                // Not saved: tried again after a moment.
                Reply::Saved(slot, Err(e)) => {
                    if let Some(text) = self.sent.remove(&slot) {
                        self.pending.entry(slot).or_insert((text, time));
                    }
                    notes.push(format!("Couldn't save the unsent chat text: {}", String::from(e)));
                }
            }
        }
        let due: Vec<SlotId> = self.pending.iter().filter(|(_, (_, since))| time - since >= after).map(|(slot, _)| slot.clone()).collect();
        for slot in due {
            let Some((text, _)) = self.pending.remove(&slot) else { continue };
            self.sent.insert(slot.clone(), text.clone());
            let (inbox, sent) = (self.inbox.clone(), slot.clone());
            net.save_draft(&slot, &text, ctx.clone(), move |r| push(&inbox, Reply::Saved(sent, r)));
        }
        if let Some(soonest) = self.pending.values().map(|(_, since)| after - (time - since)).reduce(f64::min) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(soonest.max(0.0)));
        }
        notes
    }

    /// What a slot's text is to be saved as (pending, else sent).
    #[cfg(test)]
    pub fn to_save(&self, slot: &SlotId) -> Option<&str> {
        self.pending.get(slot).map(|(t, _)| t.as_str()).or_else(|| self.sent.get(slot).map(String::as_str))
    }

    /// As if the server answered with `found` (tests).
    #[cfg(test)]
    pub fn arrived(&mut self, found: BTreeMap<SlotId, String>) {
        push(&self.inbox, Reply::Loaded(Ok(found.into_iter().map(|(s, t)| (s.to_string(), t)).collect())));
    }
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_is_read_once_and_saved_once_still() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let lead = SlotId::new("lead");
        let mut drafts = Drafts::default();
        assert_eq!(drafts.saved(&lead), None, "not read yet");
        drafts.changed(lead.clone(), "typing".into(), 0.0);
        drafts.tick(0.6, 0.5, &net, &ctx);
        assert!(drafts.pending.is_empty() && drafts.sent.is_empty(), "not read yet: nothing saved over it");
        drafts.arrived([(lead.clone(), "saved".to_owned())].into());
        drafts.tick(1.0, 0.5, &net, &ctx);
        assert_eq!(drafts.saved(&lead), Some(Some("saved")));
        assert_eq!(drafts.saved(&SlotId::new("sub-1")), Some(None), "read: nothing for it");
        // Unchanged: nothing to send. Changed: sent once it's been still a moment.
        drafts.changed(lead.clone(), "saved".into(), 1.0);
        assert!(drafts.pending.is_empty());
        drafts.changed(lead.clone(), "saved more".into(), 1.2);
        drafts.changed(lead.clone(), "saved more!".into(), 1.5);
        drafts.tick(1.6, 0.5, &net, &ctx);
        assert_eq!(drafts.sent.get(&lead).map(String::as_str), Some("saved"), "still changing: not sent");
        drafts.tick(2.1, 0.5, &net, &ctx);
        assert_eq!(drafts.sent.get(&lead).map(String::as_str), Some("saved more!"));
        // Emptied: saved as empty (the server drops it).
        drafts.changed(lead.clone(), String::new(), 3.0);
        drafts.tick(3.6, 0.5, &net, &ctx);
        assert_eq!(drafts.sent.get(&lead).map(String::as_str), Some(""));
    }
}
