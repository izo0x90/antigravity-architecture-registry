//! What a tag can be on in a chat: a message (what you said, or an
//! answer) and the turn it's in, named as the server names them: the
//! harness's id for the session, the turn's number in it, the message's
//! number in its turn (`harness_contracts::message_number`). The chat says
//! what's at an item and where each is drawn; the shell does the tagging.

use harness_contracts::{NativeId, Subject, TurnItem, message_number};

use super::ChatStreamState;
use super::nav::item_text;
use crate::nav::{Thing, Things};

/// The start of a text, on a line (for a thing's words).
fn first_words(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    match line.char_indices().nth(60) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

/// Where an item is as a tag names it: its session (the harness's id for
/// it), its turn's number, its number among the turn's messages (none: it
/// isn't a message).
pub(super) type Place = (NativeId, u32, Option<u32>);

/// The stream's places, for the thread (its id) at a change of it (its
/// activity count).
pub(crate) type Places = (harness_contracts::ThreadId, u64, std::rc::Rc<Vec<Option<Place>>>);

impl ChatStreamState {
    /// Where each item of the stream is (in the stream's order), as a tag
    /// names it; none for every item when the harness hasn't said the
    /// session's id. Worked out once per change of the thread.
    fn tag_places(&self) -> std::rc::Rc<Vec<Option<Place>>> {
        let Some(view) = self.thread() else { return std::rc::Rc::default() };
        let key = (view.thread.id.clone(), view.activity);
        if let Some((id, activity, places)) = self.tag_places.borrow().as_ref()
            && (id, *activity) == (&key.0, key.1)
        {
            return places.clone();
        }
        let mut places = Vec::new();
        for turn in &view.turns {
            let of: Vec<&TurnItem> = view.items_of(&turn.id).collect();
            for item in &of {
                places.push(view.thread.native_id.clone().map(|session| (session, turn.ordinal, message_number(&of, &item.id))));
            }
        }
        let places = std::rc::Rc::new(places);
        *self.tag_places.borrow_mut() = Some((key.0, key.1, places.clone()));
        places
    }

    /// The stream's items a subject is (a turn: all of its), when it's of
    /// this chat's session.
    pub(super) fn tagged_indexes(&self, subject: &Subject) -> Vec<usize> {
        let (session, turn, message) = match subject {
            Subject::ChatTurn { session, turn } => (session, *turn, None),
            Subject::ChatMessage { session, turn, message } => (session, *turn, Some(*message)),
            _ => return Vec::new(),
        };
        let places = self.tag_places();
        places.iter().enumerate().filter(|(_, p)| p.as_ref().is_some_and(|(s, t, m)| s == session && *t == turn && (message.is_none() || *m == message))).map(|(i, _)| i).collect()
    }

    /// At a message: it, then its turn (the unit: the message); at another
    /// item (a tool call): its turn.
    pub(super) fn tag_things(&self, key: &str) -> Things {
        let Some(index) = key.split(':').next().and_then(|i| i.parse::<usize>().ok()) else { return Things::default() };
        let places = self.tag_places();
        let Some(Some((session, turn, message))) = places.get(index).cloned() else { return Things::default() };
        let stream = self.stream();
        let said = |i: usize| stream.get(i).map(|item| first_words(&item_text(item))).unwrap_or_default();
        let opening = places.iter().position(|p| p.as_ref().is_some_and(|(_, t, m)| *t == turn && *m == Some(1))).map(said).unwrap_or_default();
        let turn_thing = Thing { subject: Subject::ChatTurn { session: session.clone(), turn }, words: format!("turn {turn}: {opening}") };
        match message {
            Some(m) => Things { at: vec![Thing { subject: Subject::ChatMessage { session, turn, message: m }, words: format!("message {m} of turn {turn}: {}", said(index)) }, turn_thing], unit: 0 },
            None => Things { at: vec![turn_thing], unit: 0 },
        }
    }

    /// The agent slot whose open session is `session` (the harness's id).
    pub(super) fn slot_with(&self, session: &NativeId) -> Option<harness_contracts::SlotId> {
        let agents = self.agents();
        agents.store().slots.values().find(|s| s.current_session().and_then(|x| x.native_id.as_ref()) == Some(session)).map(|s| s.id.clone())
    }

    /// Where a subject is drawn: all of its items that show.
    pub(super) fn tag_drawn_at(&self, subject: &Subject) -> Option<egui::Rect> {
        let indexes = self.tagged_indexes(subject);
        self.ui.spots.iter().filter(|s| indexes.iter().any(|i| s.key == i.to_string())).map(|s| s.rect).reduce(|a, b| a.union(b))
    }
}
