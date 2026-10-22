//! The cockpit's state, kept by the server: the panels and the buffer in
//! each, where every buffer is (the same places the jump list and marks
//! hold) and what each keeps of its own (a card pinned, how changes are
//! laid out), which buffer of each kind was used last, the marks and the
//! jump list. This structure is the one definition of what's kept; the
//! server stores it as written. (A chat's unsent text is not here: it's
//! its own record, `crate::drafts`.)
//!
//! Two versions: `VERSION` is the whole record's (older: nothing of it is
//! read); each buffer kind's `saved_version` is what its place and kept
//! blob are written in (older: that kind's buffers start new, the rest
//! holds).
//!
//! Loaded once when the cockpit opens (nothing is saved before that, so a
//! slow server can't have its state overwritten by defaults), then saved a
//! moment after it last changed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::panels::{Panels, BufferId};
use crate::nav::{Jump, JumpList, Place};
use crate::net::{NetClient, NetResult};

/// Bumped when a change to this structure itself (not a buffer kind's part
/// of it) can't read an older one: an older one is then left unread, and
/// the cockpit opens as new.
pub const VERSION: u32 = 5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedUi {
    pub version: u32,
    pub panels: Panels,
    /// Where each view is (shown or put away).
    pub views: Vec<(BufferId, Place)>,
    /// The view of each kind used last.
    pub used: Vec<BufferId>,
    pub marks: BTreeMap<char, Jump>,
    pub jumps: JumpList,
    /// What each buffer keeps of its own (a card pinned, how changes are
    /// laid out).
    pub kept: Vec<(BufferId, serde_json::Value)>,
    /// The version each kind's places and kept blobs are written in (a kind
    /// missing: 1, the first). A kind at another version is not read.
    #[serde(default)]
    pub versions: BTreeMap<crate::kind::BufferKind, u32>,
    /// The user lets agents drive the cockpit.
    #[serde(default)]
    pub agents: bool,
    /// The desk's comparison in use (git).
    #[serde(default)]
    pub comparison: harness_contracts::changes::Comparison,
    /// Where the buttons of what you're on show, once changed here (else
    /// the UI config says).
    #[serde(default)]
    pub item_buttons: Option<crate::hosted::ItemButtons>,
}

/// What came back from the server.
enum Reply {
    Loaded(NetResult<Option<serde_json::Value>>),
    Saved(NetResult<()>),
}

/// Loading and saving.
#[derive(Default)]
pub struct Keeper {
    /// The state read in (or none to read): saving starts after this.
    loaded: bool,
    asked: bool,
    /// What was last sent, and the state as it is now with when it last
    /// changed (sent once it's been still a moment).
    sent: Option<String>,
    pending: Option<(String, f64)>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

/// What the keeper hands back to apply.
pub enum Event {
    /// The saved state, to put back.
    Restore(Box<SavedUi>),
    /// Nothing saved (or nothing readable): the cockpit opens as new.
    Fresh,
    /// Something to tell the user (a save or load that failed).
    Note(String),
}

impl Keeper {
    /// Asks for the saved state (again, after the server was away and it
    /// wasn't read yet).
    pub fn load(&mut self, net: &NetClient, ctx: &egui::Context) {
        if self.loaded || self.asked {
            return;
        }
        self.asked = true;
        let inbox = self.inbox.clone();
        net.ui_state(ctx.clone(), move |r| push(&inbox, Reply::Loaded(r)));
    }

    /// The server was away: a load that didn't arrive is asked for again.
    pub fn reconnected(&mut self) {
        if !self.loaded {
            self.asked = false;
        }
    }

    /// Takes replies, and saves `now_state` once it's been unchanged for
    /// `after` seconds. Returns what to apply.
    pub fn tick(&mut self, now_state: &SavedUi, now: f64, after: f64, net: &NetClient, ctx: &egui::Context) -> Vec<Event> {
        let mut out = Vec::new();
        for reply in std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                Reply::Loaded(Ok(value)) => {
                    self.loaded = true;
                    // Its version first: one from another version isn't read.
                    let version = value.as_ref().and_then(|v| v.get("version")).and_then(serde_json::Value::as_u64);
                    match (value, version) {
                        (None, _) => out.push(Event::Fresh),
                        (Some(_), Some(v)) if v != u64::from(VERSION) => out.extend([Event::Fresh, Event::Note("The saved layout is from an older version of the cockpit: starting with a new one.".into())]),
                        (Some(value), _) => match serde_json::from_value::<SavedUi>(value) {
                            Ok(saved) => out.push(Event::Restore(Box::new(saved))),
                            Err(e) => out.extend([Event::Fresh, Event::Note(format!("The saved layout couldn't be read ({e}): starting with a new one."))]),
                        },
                    }
                }
                // Not read: nothing is saved over it (asked again on reconnect).
                Reply::Loaded(Err(e)) => {
                    self.asked = false;
                    out.push(Event::Note(format!("Couldn't read the saved layout: {}", String::from(e))));
                }
                Reply::Saved(Ok(())) => {}
                Reply::Saved(Err(e)) => {
                    self.sent = None;
                    out.push(Event::Note(format!("Couldn't save the layout: {}", String::from(e))));
                }
            }
        }
        // Restored this frame: what's on screen next frame is the saved state.
        if !self.loaded || out.iter().any(|e| matches!(e, Event::Restore(_))) {
            return out;
        }
        let text = serde_json::to_string(now_state).unwrap_or_default();
        if self.sent.as_deref() == Some(text.as_str()) {
            self.pending = None;
            return out;
        }
        match &self.pending {
            Some((waiting, since)) if *waiting == text => {
                if now - since >= after {
                    let value = serde_json::to_value(now_state).unwrap_or_default();
                    self.sent = Some(text);
                    self.pending = None;
                    let inbox = self.inbox.clone();
                    net.save_ui_state(value, ctx.clone(), move |r| push(&inbox, Reply::Saved(r)));
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(after - (now - since)));
                }
            }
            _ => {
                self.pending = Some((text, now));
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(after));
            }
        }
        out
    }

    /// The state just put back is what the server has: not sent again.
    pub fn restored(&mut self, state: &SavedUi) {
        self.sent = serde_json::to_string(state).ok();
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

    fn state(draft: &str) -> SavedUi {
        SavedUi { version: VERSION, panels: Panels::default(), views: Vec::new(), kept: vec![(BufferId::new(crate::kind::BufferKind::Spec, 0), serde_json::Value::String(draft.into()))], used: Vec::new(), marks: BTreeMap::new(), jumps: JumpList::default(), agents: false, comparison: Default::default(), item_buttons: None, versions: BTreeMap::new() }
    }

    fn arrive(keeper: &Keeper, value: Option<serde_json::Value>) {
        push(&keeper.inbox, Reply::Loaded(Ok(value)));
    }

    #[test]
    fn nothing_is_saved_before_the_saved_state_is_read_and_then_only_once_still() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let mut keeper = Keeper::default();
        assert!(keeper.tick(&state("a"), 0.0, 0.5, &net, &ctx).is_empty());
        assert_eq!((keeper.sent.as_ref(), keeper.pending.as_ref()), (None, None), "not read yet: nothing saved over it");
        // Read: what was saved comes back to be put on screen.
        let saved = state("saved draft");
        arrive(&keeper, Some(serde_json::to_value(&saved).unwrap()));
        let events = keeper.tick(&state("a"), 1.0, 0.5, &net, &ctx);
        assert!(matches!(events.as_slice(), [Event::Restore(s)] if **s == saved));
        keeper.restored(&saved);
        // Unchanged: nothing to send. Changed: sent once it's been still a moment.
        keeper.tick(&saved, 1.1, 0.5, &net, &ctx);
        assert!(keeper.pending.is_none());
        keeper.tick(&state("typing"), 1.2, 0.5, &net, &ctx);
        keeper.tick(&state("typing more"), 1.5, 0.5, &net, &ctx);
        assert_eq!(keeper.sent, serde_json::to_string(&saved).ok(), "still changing: not sent");
        keeper.tick(&state("typing more"), 2.1, 0.5, &net, &ctx);
        assert_eq!(keeper.sent, serde_json::to_string(&state("typing more")).ok());
    }

    #[test]
    fn a_state_that_cant_be_read_starts_fresh_and_says_so() {
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        let mut keeper = Keeper::default();
        arrive(&keeper, None);
        assert!(matches!(keeper.tick(&state(""), 0.0, 0.5, &net, &ctx).as_slice(), [Event::Fresh]));
        let mut old = Keeper::default();
        arrive(&old, Some(serde_json::json!({ "version": VERSION + 1 })));
        let events = old.tick(&state(""), 0.0, 0.5, &net, &ctx);
        assert!(matches!(events.as_slice(), [Event::Fresh, Event::Note(n)] if n.contains("another version") || n.contains("older version")), "one from another version isn't read");
        // Read: saving can start.
        old.tick(&state("x"), 1.0, 0.5, &net, &ctx);
        assert!(old.pending.is_some());
    }
}
