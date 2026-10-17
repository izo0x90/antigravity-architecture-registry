//! Typing into a field that takes every key (a terminal): which keys leave
//! typing, and what the rest are to the program in it.
//!
//! Leaving: `[keymap.input] leave` lists key sequences ("Escape Escape",
//! "ctrl+\ n"). The first key of one is held from the field until the
//! next key comes: the sequence leaves; another key, or the wait passing,
//! sends the held key in after all. One rule for every sequence.

use harness_contracts::{Mods, TermKey, TerminalInput};

use super::{Chord, InputKeys};

/// What a frame's keys did to typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typing {
    Stays,
    Leaves,
}

/// A sequence's first key, held from the field (its events, the chord it
/// is, and since when).
#[derive(Default)]
pub struct Leaving(Option<(Vec<egui::Event>, Chord, f64)>);

impl Chord {
    /// Whether a key press (with the modifiers it was pressed with) is this chord.
    fn is_press(&self, key: egui::Key, mods: egui::Modifiers) -> bool {
        self.physical() == Some(key) && self.ctrl == (mods.ctrl || mods.command) && self.alt == mods.alt && (self.shift == mods.shift || !self.is_named())
    }

    fn is_named(&self) -> bool {
        matches!(self.key, super::ChordKey::Key(_))
    }
}

impl Leaving {
    /// Before the field reads this frame's input: takes out what leaves
    /// typing, holds a sequence's first key, and puts a held key back when
    /// it turns out not to be one (or waited too long).
    pub fn filter(&mut self, input: &mut egui::InputState, keys: &InputKeys) -> Typing {
        let now = input.time;
        let wait = keys.leave_wait_ms as f64 / 1000.0;
        let mut out = Vec::new();
        if let Some((events, _, since)) = &self.0
            && now - since > wait
        {
            out.extend(events.iter().cloned());
            self.0 = None;
        }
        let events = std::mem::take(&mut input.events);
        let mut events = events.into_iter().peekable();
        let mut leaves = false;
        while let Some(event) = events.next() {
            let egui::Event::Key { key, pressed: true, modifiers, .. } = event else {
                // A held key's release goes with it.
                if let egui::Event::Key { key, pressed: false, .. } = &event
                    && self.0.as_ref().is_some_and(|(held, ..)| matches!(held.first(), Some(egui::Event::Key { key: k, .. }) if k == key))
                {
                    continue;
                }
                if !leaves {
                    out.push(event);
                }
                continue;
            };
            // The press, and the text it typed.
            let mut press = vec![event];
            if let Some(egui::Event::Text(_)) = events.peek() {
                press.extend(events.next());
            }
            if leaves {
                continue;
            }
            if let Some((held, first, _)) = self.0.take() {
                if keys.leave.iter().any(|s| s.0.len() == 2 && s.0[0] == first && s.0[1].is_press(key, modifiers)) {
                    leaves = true;
                    continue;
                }
                out.extend(held);
            }
            if let Some(first) = keys.leave.iter().find(|s| s.0.len() > 1 && s.0[0].is_press(key, modifiers)).map(|s| s.0[0].clone()) {
                self.0 = Some((press, first, now));
                continue;
            }
            if keys.leave.iter().any(|s| s.0.len() == 1 && s.0[0].is_press(key, modifiers)) {
                leaves = true;
                continue;
            }
            out.extend(press);
        }
        input.events = out;
        if leaves {
            self.0 = None;
            Typing::Leaves
        } else {
            Typing::Stays
        }
    }

    /// A key is held, waiting for the next (draw again by then).
    pub fn waiting(&self, keys: &InputKeys, now: f64) -> Option<f64> {
        self.0.as_ref().map(|(_, _, since)| (since + keys.leave_wait_ms as f64 / 1000.0 - now).max(0.0))
    }
}

/// What a frame's events are to a terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    Input(TerminalInput),
    /// The platform's copy (Cmd+C, or Ctrl+C's copy event): copies a
    /// selection, when there's one.
    Copy,
}

/// A key that isn't text, as a terminal knows it.
fn term_key(key: egui::Key) -> Option<TermKey> {
    use egui::Key as K;
    Some(match key {
        K::Enter => TermKey::Enter,
        K::Tab => TermKey::Tab,
        K::Backspace => TermKey::Backspace,
        K::Escape => TermKey::Escape,
        K::ArrowUp => TermKey::Up,
        K::ArrowDown => TermKey::Down,
        K::ArrowLeft => TermKey::Left,
        K::ArrowRight => TermKey::Right,
        K::Home => TermKey::Home,
        K::End => TermKey::End,
        K::PageUp => TermKey::PageUp,
        K::PageDown => TermKey::PageDown,
        K::Insert => TermKey::Insert,
        K::Delete => TermKey::Delete,
        K::F1 => TermKey::F(1),
        K::F2 => TermKey::F(2),
        K::F3 => TermKey::F(3),
        K::F4 => TermKey::F(4),
        K::F5 => TermKey::F(5),
        K::F6 => TermKey::F(6),
        K::F7 => TermKey::F(7),
        K::F8 => TermKey::F(8),
        K::F9 => TermKey::F(9),
        K::F10 => TermKey::F(10),
        K::F11 => TermKey::F(11),
        K::F12 => TermKey::F(12),
        _ => return None,
    })
}

/// A frame's events as what's typed into a terminal: keys that aren't
/// text by name, keys with Ctrl or Alt by their character (the text the
/// browser also gives for them dropped), text as typed, pastes. Cmd's keys
/// are the platform's (its copy and paste come as their own events).
pub fn terminal_input(events: &[egui::Event]) -> Vec<Typed> {
    let mut out = Vec::new();
    let mut skip_text = false;
    let pasting = events.iter().any(|e| matches!(e, egui::Event::Paste(_)));
    for event in events {
        match event {
            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                skip_text = false;
                if m.mac_cmd {
                    continue;
                }
                let mods = Mods { ctrl: m.ctrl, alt: m.alt, shift: m.shift };
                if let Some(named) = term_key(*key) {
                    out.push(Typed::Input(TerminalInput::Key { key: named, mods }));
                } else if m.ctrl || m.alt {
                    // Ctrl+V's paste comes as its own event.
                    if *key == egui::Key::V && m.ctrl && pasting {
                        continue;
                    }
                    let mut chars = key.symbol_or_name().chars();
                    if let (Some(c), None) = (chars.next(), chars.next()) {
                        out.push(Typed::Input(TerminalInput::Key { key: TermKey::Char(c.to_ascii_lowercase()), mods }));
                        skip_text = true;
                    }
                }
            }
            egui::Event::Text(text) if std::mem::take(&mut skip_text) => {
                let _ = text;
            }
            egui::Event::Text(text) => out.push(Typed::Input(TerminalInput::Text { text: text.clone() })),
            egui::Event::Paste(text) => out.push(Typed::Input(TerminalInput::Paste { text: text.clone() })),
            egui::Event::Copy | egui::Event::Cut => out.push(Typed::Copy),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    fn up(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    const CTRL: egui::Modifiers = egui::Modifiers { ctrl: true, command: true, alt: false, shift: false, mac_cmd: false };
    const ALT: egui::Modifiers = egui::Modifiers { alt: true, ctrl: false, shift: false, mac_cmd: false, command: false };
    const NONE: egui::Modifiers = egui::Modifiers::NONE;

    fn input(text: &str) -> TerminalInput {
        TerminalInput::Text { text: text.into() }
    }

    fn named(k: TermKey, ctrl: bool, alt: bool) -> TerminalInput {
        TerminalInput::Key { key: k, mods: Mods { ctrl, alt, shift: false } }
    }

    #[test]
    fn keys_are_what_a_terminal_expects_as_a_browser_gives_them() {
        let typed = |events: Vec<egui::Event>| terminal_input(&events);
        assert_eq!(typed(vec![key(egui::Key::A, NONE), egui::Event::Text("a".into())]), [Typed::Input(input("a"))], "text as typed");
        assert_eq!(typed(vec![key(egui::Key::Enter, NONE), key(egui::Key::Tab, NONE), key(egui::Key::ArrowUp, NONE)]), [named(TermKey::Enter, false, false), named(TermKey::Tab, false, false), named(TermKey::Up, false, false)].map(Typed::Input));
        assert_eq!(typed(vec![key(egui::Key::C, CTRL), egui::Event::Copy]), [Typed::Input(named(TermKey::Char('c'), true, false)), Typed::Copy], "Ctrl+C: ^C (and the copy, for a selection)");
        assert_eq!(typed(vec![key(egui::Key::B, ALT), egui::Event::Text("b".into())]), [Typed::Input(named(TermKey::Char('b'), false, true))], "Alt+B by its key, its text dropped");
        assert_eq!(typed(vec![key(egui::Key::V, CTRL), egui::Event::Paste("ls\n".into())]), [Typed::Input(TerminalInput::Paste { text: "ls\n".into() })], "Ctrl+V: the paste, not ^V");
        assert_eq!(typed(vec![key(egui::Key::Backslash, CTRL)]), [Typed::Input(named(TermKey::Char('\\'), true, false))]);
        let cmd = egui::Modifiers { mac_cmd: true, command: true, ..NONE };
        assert_eq!(typed(vec![key(egui::Key::C, cmd), egui::Event::Copy]), [Typed::Copy], "Cmd+C is the platform's copy only");
    }

    fn frame(leaving: &mut Leaving, keys: &InputKeys, time: f64, events: Vec<egui::Event>) -> (Typing, Vec<egui::Event>) {
        let mut input = egui::InputState::default();
        input.time = time;
        input.events = events;
        let typing = leaving.filter(&mut input, keys);
        (typing, input.events)
    }

    #[test]
    fn escape_twice_leaves_and_escape_once_reaches_the_program_after_the_wait() {
        let keys = InputKeys::default();
        let mut leaving = Leaving::default();
        let (typing, passed) = frame(&mut leaving, &keys, 1.0, vec![key(egui::Key::Escape, NONE), up(egui::Key::Escape)]);
        assert_eq!((typing, passed), (Typing::Stays, vec![]), "held: it may be the first of two");
        assert!(leaving.waiting(&keys, 1.1).is_some());
        let (typing, passed) = frame(&mut leaving, &keys, 1.15, vec![key(egui::Key::Escape, NONE)]);
        assert_eq!((typing, passed), (Typing::Leaves, vec![]), "twice within the wait: typing stops, the program gets neither");
        let (_, passed) = frame(&mut leaving, &keys, 2.0, vec![key(egui::Key::Escape, NONE)]);
        assert!(passed.is_empty());
        let (typing, passed) = frame(&mut leaving, &keys, 2.4, vec![]);
        assert_eq!((typing, passed), (Typing::Stays, vec![key(egui::Key::Escape, NONE)]), "once, and the wait passed: the program gets it");
    }

    #[test]
    fn a_held_first_key_followed_by_another_goes_in_before_it() {
        let keys = InputKeys::default();
        let mut leaving = Leaving::default();
        frame(&mut leaving, &keys, 1.0, vec![key(egui::Key::Escape, NONE)]);
        let (typing, passed) = frame(&mut leaving, &keys, 1.05, vec![key(egui::Key::J, NONE), egui::Event::Text("j".into())]);
        assert_eq!((typing, passed), (Typing::Stays, vec![key(egui::Key::Escape, NONE), key(egui::Key::J, NONE), egui::Event::Text("j".into())]), "vim's Escape then j, in order");
        frame(&mut leaving, &keys, 2.0, vec![key(egui::Key::Backslash, CTRL)]);
        let (typing, passed) = frame(&mut leaving, &keys, 2.05, vec![key(egui::Key::N, NONE), egui::Event::Text("n".into())]);
        assert_eq!((typing, passed), (Typing::Leaves, vec![]), "ctrl+\\ n leaves; neither key reaches the program");
        let (typing, passed) = frame(&mut leaving, &keys, 3.0, vec![key(egui::Key::A, NONE), egui::Event::Text("a".into())]);
        assert_eq!((typing, passed.len()), (Typing::Stays, 2), "anything else passes as it is");
    }
}
