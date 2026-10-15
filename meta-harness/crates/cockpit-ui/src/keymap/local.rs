//! The keys outside the views' bindings, from the same config: moving and
//! picking in a list (the palette, its pickers), sending from a typing box
//! and its new line, the modifier held while clicking a name to go to it,
//! the leader and the way back out. Views read them through `local()` (set
//! with the rest of the config), never as keys of their own, and say them
//! by their labels.

use std::cell::RefCell;
use std::rc::Rc;

use serde::Deserialize;

use super::Chord;

fn chords(specs: &[&str]) -> Vec<Chord> {
    specs.iter().map(|s| Chord::try_from((*s).to_owned()).expect("a built-in key")).collect()
}

fn any(chords: &[Chord], input: &egui::InputState) -> bool {
    chords.iter().any(|c| c.pressed(input))
}

/// Every list: the palette and its pickers.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListKeys {
    pub next: Vec<Chord>,
    pub prev: Vec<Chord>,
    pub pick: Vec<Chord>,
}

impl Default for ListKeys {
    fn default() -> Self {
        Self { next: chords(&["ArrowDown"]), prev: chords(&["ArrowUp"]), pick: chords(&["Enter"]) }
    }
}

impl ListKeys {
    /// The step asked for (down one, up one, or none), its key taken so a
    /// list's filter box doesn't act on it too (egui's text boxes use
    /// Ctrl+K and others themselves).
    pub fn step(&self, input: &mut egui::InputState) -> i32 {
        let took = |chords: &[Chord], input: &mut egui::InputState| chords.iter().fold(false, |took, c| c.consume(input) || took);
        i32::from(took(&self.next, input)) - i32::from(took(&self.prev, input))
    }

    pub fn picked(&self, input: &egui::InputState) -> bool {
        any(&self.pick, input)
    }

    pub fn pick_label(&self) -> String {
        self.pick.first().map(Chord::label).unwrap_or_default()
    }

    /// What a list's keys do (`leave`: what the way back out does), each
    /// key with its words.
    pub fn keys(&self, back: &Chord, leave: &'static str) -> Vec<(String, &'static str)> {
        let first = |chords: &[Chord]| chords.first().map(Chord::label).unwrap_or_default();
        vec![(self.pick_label(), "runs"), (format!("{}/{}", first(&self.next), first(&self.prev)), "choose"), (back.label(), leave)]
    }
}

/// Every typing box.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputKeys {
    pub send: Vec<Chord>,
    /// In the chat: into the turn it's running.
    pub add: Vec<Chord>,
    /// A new line in the box (the box's own return key).
    pub newline: Chord,
}

impl Default for InputKeys {
    fn default() -> Self {
        let [newline] = <[Chord; 1]>::try_from(chords(&["shift+Enter"])).expect("one key");
        Self { send: chords(&["Enter"]), add: chords(&["ctrl+Enter"]), newline }
    }
}

/// What a typing box's keys asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Send,
    /// Into what it's doing.
    Add,
}

impl InputKeys {
    pub fn sent(&self, input: &egui::InputState) -> Option<Sent> {
        if any(&self.add, input) {
            Some(Sent::Add)
        } else if any(&self.send, input) {
            Some(Sent::Send)
        } else {
            None
        }
    }

    /// The send key in words, for a box's hint.
    pub fn send_label(&self) -> String {
        self.send.first().map(Chord::label).unwrap_or_default()
    }

    pub fn add_label(&self) -> String {
        self.add.first().map(Chord::label).unwrap_or_default()
    }

    /// What a typing box's keys do (`leave`: what the way back out does),
    /// each key with its words.
    pub fn keys(&self, back: &Chord, leave: &'static str) -> Vec<(String, &'static str)> {
        vec![(self.send_label(), "sends"), (self.newline.label(), "new line"), (back.label(), leave)]
    }
}

/// A modifier held with the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modifier {
    /// Ctrl, or Cmd on a Mac.
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    /// Its name, as keys are written ("ctrl").
    pub fn label(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
        }
    }

    pub fn held(self, modifiers: egui::Modifiers) -> bool {
        match self {
            Self::Ctrl => modifiers.ctrl || modifiers.command,
            Self::Alt => modifiers.alt,
            Self::Shift => modifiers.shift,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerKeys {
    /// Held while clicking a name in the code: go to what it names.
    pub go_to: Modifier,
}

impl Default for PointerKeys {
    fn default() -> Self {
        Self { go_to: Modifier::Ctrl }
    }
}

/// The keys outside the views' bindings, the leader and the way back out.
#[derive(Clone, Debug)]
pub struct LocalKeys {
    pub list: ListKeys,
    pub input: InputKeys,
    pub pointer: PointerKeys,
    pub leader: Chord,
    pub back: Chord,
}

impl Default for LocalKeys {
    fn default() -> Self {
        let [leader, back] = <[Chord; 2]>::try_from(chords(&["Space", "Escape"])).expect("two keys");
        Self { list: ListKeys::default(), input: InputKeys::default(), pointer: PointerKeys::default(), leader, back }
    }
}

impl LocalKeys {
    /// The way back out was pressed.
    pub fn back(&self, input: &egui::InputState) -> bool {
        self.back.pressed(input)
    }

    /// A typing box: several lines, a new line on the configured key.
    pub fn text_box<'t>(&self, text: &'t mut dyn egui::TextBuffer) -> egui::TextEdit<'t> {
        egui::TextEdit::multiline(text).return_key(self.input.newline.shortcut())
    }

    /// A leader path in words ("Space s n").
    pub fn leader_hint(&self, path: &[char]) -> String {
        std::iter::once(self.leader.label()).chain(path.iter().map(char::to_string)).collect::<Vec<_>>().join(" ")
    }
}

thread_local! {
    static LOCAL: RefCell<Rc<LocalKeys>> = RefCell::new(Rc::new(LocalKeys::default()));
}

/// The keys as configured (the UI is single-threaded).
pub fn local() -> Rc<LocalKeys> {
    LOCAL.with(|l| l.borrow().clone())
}

pub fn set_local(keys: LocalKeys) {
    LOCAL.with(|l| *l.borrow_mut() = Rc::new(keys));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pressed(key: egui::Key, modifiers: egui::Modifiers) -> egui::InputState {
        let ctx = egui::Context::default();
        let mut out = None;
        let raw = egui::RawInput { events: vec![egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], modifiers, ..Default::default() };
        let _ = ctx.run(raw, |ctx| out = Some(ctx.input(|i| i.clone())));
        out.unwrap()
    }

    #[test]
    fn lists_and_boxes_take_their_keys_from_config() {
        let ctrl = egui::Modifiers { ctrl: true, ..Default::default() };
        let list = ListKeys { next: chords(&["ctrl+j", "ArrowDown"]), prev: chords(&["ctrl+k"]), pick: chords(&["Enter"]) };
        assert_eq!(list.step(&mut pressed(egui::Key::J, ctrl)), 1);
        assert_eq!(list.step(&mut pressed(egui::Key::K, ctrl)), -1);
        assert_eq!(list.step(&mut pressed(egui::Key::J, egui::Modifiers::default())), 0, "a plain j is typed, not a move");
        let input = InputKeys::default();
        assert_eq!(input.sent(&pressed(egui::Key::Enter, egui::Modifiers::default())), Some(Sent::Send));
        assert_eq!(input.sent(&pressed(egui::Key::Enter, ctrl)), Some(Sent::Add));
        assert_eq!(input.sent(&pressed(egui::Key::Enter, egui::Modifiers { shift: true, ..Default::default() })), None, "Shift+Enter: a new line");
    }
}
