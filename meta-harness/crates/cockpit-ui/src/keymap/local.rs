//! The keys outside the views' bindings, from the same config: moving and
//! picking in a list (the action list, its pickers), sending from a typing box
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

/// Every list: the action list and its pickers, a setting's choices.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListKeys {
    pub next: Vec<Chord>,
    pub prev: Vec<Chord>,
    /// Half a page down and up, and to the first and last row: the moves
    /// every view has, here with keys a filter box doesn't type.
    pub half_down: Vec<Chord>,
    pub half_up: Vec<Chord>,
    pub first: Vec<Chord>,
    pub last: Vec<Chord>,
    pub pick: Vec<Chord>,
    /// In the action list: everything, or what applies here. While a value
    /// is picked: what it's for, the next of its kinds (what a tag goes on:
    /// a line, its command).
    pub cycle: Vec<Chord>,
}

impl Default for ListKeys {
    fn default() -> Self {
        Self { next: chords(&["ArrowDown"]), prev: chords(&["ArrowUp"]), half_down: chords(&["PageDown"]), half_up: chords(&["PageUp"]), first: chords(&["ctrl+Home"]), last: chords(&["ctrl+End"]), pick: chords(&["Enter"]), cycle: chords(&["Tab"]) }
    }
}

impl ListKeys {
    /// The move asked for, its key taken so a list's filter box doesn't act
    /// on it too (egui's text boxes use Ctrl+K and others themselves).
    pub fn moved(&self, input: &mut egui::InputState) -> Option<crate::nav::Motion> {
        use crate::nav::Motion;
        let took = |chords: &[Chord], input: &mut egui::InputState| chords.iter().fold(false, |took, c| c.consume(input) || took);
        [(&self.next, Motion::Down), (&self.prev, Motion::Up), (&self.half_down, Motion::HalfDown), (&self.half_up, Motion::HalfUp), (&self.first, Motion::First), (&self.last, Motion::Last)]
            .into_iter()
            .fold(None, |moved, (chords, m)| if took(chords, input) { Some(m) } else { moved })
    }

    pub fn picked(&self, input: &egui::InputState) -> bool {
        any(&self.pick, input)
    }

    /// The cycle key, taken (a text box would move focus on Tab).
    pub fn cycled(&self, input: &mut egui::InputState) -> bool {
        self.cycle.iter().fold(false, |took, c| c.consume(input) || took)
    }

    pub fn cycle_label(&self) -> String {
        self.cycle.first().map(Chord::label).unwrap_or_default()
    }

    pub fn pick_label(&self) -> String {
        self.pick.first().map(Chord::label).unwrap_or_default()
    }

    /// What a list's keys do (`leave`: what the way back out does), each
    /// key with its words.
    pub fn keys(&self, back: &Chord, leave: &'static str) -> Vec<(String, &'static str)> {
        let first = |chords: &[Chord]| chords.first().map(Chord::label).unwrap_or_default();
        let pair = |a: &[Chord], b: &[Chord]| format!("{}/{}", first(a), first(b));
        vec![
            (self.pick_label(), "picks"),
            (pair(&self.next, &self.prev), "choose"),
            (pair(&self.half_down, &self.half_up), "half a page"),
            (pair(&self.first, &self.last), "first, last"),
            (back.label(), leave),
        ]
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
    /// What leaves typing in a field that takes every key (a terminal,
    /// where Escape is the program's): each a sequence of keys.
    #[serde(default = "default_leave")]
    pub leave: Vec<super::Sequence>,
    /// How long a sequence's first key waits for the next: past it, the
    /// key goes into the field after all.
    #[serde(default = "default_leave_wait")]
    pub leave_wait_ms: u64,
    /// The link at the caret: whole, a pointer, or as it fits (in turn).
    #[serde(default = "default_link")]
    pub link: Chord,
}

fn default_link() -> Chord {
    Chord::try_from("ctrl+i".to_owned()).expect("a built-in key")
}

fn default_leave() -> Vec<super::Sequence> {
    ["Escape Escape", "ctrl+\\ n"].into_iter().map(|s| super::Sequence::try_from(s.to_owned()).expect("a sequence")).collect()
}

fn default_leave_wait() -> u64 {
    300
}

impl Default for InputKeys {
    fn default() -> Self {
        let [newline] = <[Chord; 1]>::try_from(chords(&["shift+Enter"])).expect("one key");
        Self { send: chords(&["Enter"]), add: chords(&["ctrl+Enter"]), newline, leave: default_leave(), leave_wait_ms: default_leave_wait(), link: default_link() }
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

    /// The first key that does `sent`, in words.
    pub fn label(&self, sent: Sent) -> String {
        let keys = match sent {
            Sent::Send => &self.send,
            Sent::Add => &self.add,
        };
        keys.first().map(Chord::label).unwrap_or_default()
    }

    /// What a typing box's keys do (`leave`: what the way back out does),
    /// each key with its words.
    pub fn keys(&self, back: &Chord, leave: &'static str) -> Vec<(String, &'static str)> {
        vec![(self.label(Sent::Send), "sends"), (self.newline.label(), "new line"), (self.link.label(), "a link: whole, pointer, as it fits"), (back.label(), leave)]
    }

    /// What a field that takes every key says about its keys: how to leave.
    pub fn leave_keys(&self) -> Vec<(String, &'static str)> {
        self.leave.iter().map(|s| (s.label(), "stops typing")).collect()
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
    /// Held while turning the wheel: zoom, where a view zooms (the graph).
    pub zoom: Modifier,
}

impl Default for PointerKeys {
    fn default() -> Self {
        Self { go_to: Modifier::Ctrl, zoom: Modifier::Ctrl }
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
    /// The keys with ctrl the cockpit binds, as a browser names them: the
    /// page keeps them from the browser.
    pub browser_keys: std::collections::BTreeSet<String>,
}

impl Default for LocalKeys {
    fn default() -> Self {
        let [leader, back] = <[Chord; 2]>::try_from(chords(&["Space", "Escape"])).expect("two keys");
        Self { list: ListKeys::default(), input: InputKeys::default(), pointer: PointerKeys::default(), leader, back, browser_keys: Default::default() }
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
        use crate::nav::Motion;
        let list = ListKeys { next: chords(&["ctrl+j", "ArrowDown"]), prev: chords(&["ctrl+k"]), half_down: chords(&["ctrl+d"]), half_up: chords(&["ctrl+u"]), first: chords(&["ctrl+Home"]), last: chords(&["ctrl+End"]), pick: chords(&["Enter"]), cycle: chords(&["Tab"]) };
        assert_eq!(list.moved(&mut pressed(egui::Key::J, ctrl)), Some(Motion::Down));
        assert_eq!(list.moved(&mut pressed(egui::Key::K, ctrl)), Some(Motion::Up));
        assert_eq!(list.moved(&mut pressed(egui::Key::D, ctrl)), Some(Motion::HalfDown));
        assert_eq!(list.moved(&mut pressed(egui::Key::U, ctrl)), Some(Motion::HalfUp));
        assert_eq!(list.moved(&mut pressed(egui::Key::End, ctrl)), Some(Motion::Last));
        assert_eq!(list.moved(&mut pressed(egui::Key::Home, ctrl)), Some(Motion::First));
        assert_eq!(list.moved(&mut pressed(egui::Key::J, egui::Modifiers::default())), None, "a plain j is typed, not a move");
        let input = InputKeys::default();
        assert_eq!(input.sent(&pressed(egui::Key::Enter, egui::Modifiers::default())), Some(Sent::Send));
        assert_eq!(input.sent(&pressed(egui::Key::Enter, ctrl)), Some(Sent::Add));
        assert_eq!(input.sent(&pressed(egui::Key::Enter, egui::Modifiers { shift: true, ..Default::default() })), None, "Shift+Enter: a new line");
    }
}
