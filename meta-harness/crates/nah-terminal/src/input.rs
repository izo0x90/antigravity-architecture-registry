//! What's typed, as the bytes a terminal program expects in the modes it's
//! in (application cursor keys, bracketed paste).

use harness_contracts::{Mods, TermKey, TerminalInput};

/// The modes typing depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// Arrows (and Home/End) as `ESC O x` rather than `ESC [ x`.
    pub app_cursor: bool,
    /// Pastes wrapped in `ESC [200~` … `ESC [201~`.
    pub bracketed_paste: bool,
}

pub fn encode(input: &TerminalInput, modes: Modes) -> Vec<u8> {
    match input {
        TerminalInput::Text { text } => text.as_bytes().to_vec(),
        TerminalInput::Paste { text } => paste(text, modes),
        TerminalInput::Key { key, mods } => key_bytes(*key, *mods, modes),
    }
}

/// A paste: bracketed when asked for (anything in it that would end the
/// bracket taken out); else its line ends as Enter sends them.
fn paste(text: &str, modes: Modes) -> Vec<u8> {
    if modes.bracketed_paste {
        let clean = text.replace('\x1b', "");
        [b"\x1b[200~".as_slice(), clean.as_bytes(), b"\x1b[201~"].concat()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier(mods: Mods) -> u8 {
    1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl)
}

fn key_bytes(key: TermKey, mods: Mods, modes: Modes) -> Vec<u8> {
    let any = mods.shift || mods.alt || mods.ctrl;
    // Cursor-style keys: `ESC [ x` (or `ESC O x` in application mode),
    // `ESC [ 1 ; m x` with modifiers.
    let cursor = |x: u8| -> Vec<u8> {
        if any {
            format!("\x1b[1;{}{}", modifier(mods), x as char).into_bytes()
        } else if modes.app_cursor {
            vec![0x1b, b'O', x]
        } else {
            vec![0x1b, b'[', x]
        }
    };
    // Tilde keys: `ESC [ n ~`, `ESC [ n ; m ~` with modifiers.
    let tilde = |n: u8| -> Vec<u8> {
        if any { format!("\x1b[{n};{}~", modifier(mods)).into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let alt = |bytes: Vec<u8>| -> Vec<u8> { if mods.alt { [vec![0x1b], bytes].concat() } else { bytes } };
    match key {
        TermKey::Up => cursor(b'A'),
        TermKey::Down => cursor(b'B'),
        TermKey::Right => cursor(b'C'),
        TermKey::Left => cursor(b'D'),
        TermKey::Home => cursor(b'H'),
        TermKey::End => cursor(b'F'),
        TermKey::Insert => tilde(2),
        TermKey::Delete => tilde(3),
        TermKey::PageUp => tilde(5),
        TermKey::PageDown => tilde(6),
        TermKey::F(n @ 1..=4) => {
            let x = b'P' + (n - 1);
            if any { format!("\x1b[1;{}{}", modifier(mods), x as char).into_bytes() } else { vec![0x1b, b'O', x] }
        }
        TermKey::F(n) => tilde(match n {
            5 => 15,
            6 => 17,
            7 => 18,
            8 => 19,
            9 => 20,
            10 => 21,
            11 => 23,
            _ => 24,
        }),
        TermKey::Enter => alt(vec![b'\r']),
        TermKey::Tab if mods.shift => b"\x1b[Z".to_vec(),
        TermKey::Tab => alt(vec![b'\t']),
        TermKey::Backspace if mods.ctrl => alt(vec![0x08]),
        TermKey::Backspace => alt(vec![0x7f]),
        TermKey::Escape => alt(vec![0x1b]),
        TermKey::Char(c) if mods.ctrl => alt(control(c).map_or_else(|| c.to_string().into_bytes(), |b| vec![b])),
        TermKey::Char(c) => alt(c.to_string().into_bytes()),
    }
}

/// Ctrl with a key: its control byte, when it has one.
fn control(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(c as u8 - b'a' + 1),
        ' ' | '@' | '2' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '/' | '-' | '7' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: TermKey, ctrl: bool, alt: bool, shift: bool) -> TerminalInput {
        TerminalInput::Key { key, mods: Mods { ctrl, alt, shift } }
    }

    #[test]
    fn keys_are_the_bytes_programs_expect() {
        let normal = Modes::default();
        let app = Modes { app_cursor: true, ..Modes::default() };
        assert_eq!(encode(&key(TermKey::Up, false, false, false), normal), b"\x1b[A");
        assert_eq!(encode(&key(TermKey::Up, false, false, false), app), b"\x1bOA", "an editor asked for application keys");
        assert_eq!(encode(&key(TermKey::Right, true, false, false), app), b"\x1b[1;5C", "Ctrl+Right moves a word");
        assert_eq!(encode(&key(TermKey::Char('c'), true, false, false), normal), [3], "Ctrl+C");
        assert_eq!(encode(&key(TermKey::Char('D'), true, false, false), normal), [4], "Ctrl+D, whatever the case");
        assert_eq!(encode(&key(TermKey::Char('\\'), true, false, false), normal), [0x1c]);
        assert_eq!(encode(&key(TermKey::Char('b'), false, true, false), normal), b"\x1bb", "Alt+B: a word back");
        assert_eq!(encode(&key(TermKey::Enter, false, false, false), normal), b"\r");
        assert_eq!(encode(&key(TermKey::Backspace, false, false, false), normal), [0x7f]);
        assert_eq!(encode(&key(TermKey::Tab, false, false, true), normal), b"\x1b[Z");
        assert_eq!(encode(&key(TermKey::Delete, false, false, false), normal), b"\x1b[3~");
        assert_eq!(encode(&key(TermKey::F(1), false, false, false), normal), b"\x1bOP");
        assert_eq!(encode(&key(TermKey::F(5), false, false, false), normal), b"\x1b[15~");
        assert_eq!(encode(&TerminalInput::Text { text: "ls é".into() }, normal), "ls é".as_bytes());
    }

    #[test]
    fn a_paste_is_bracketed_when_the_program_asks_and_cant_end_the_bracket() {
        let text = TerminalInput::Paste { text: "a\nb\x1b[201~c".into() };
        assert_eq!(encode(&text, Modes { bracketed_paste: true, ..Modes::default() }), b"\x1b[200~a\nb[201~c\x1b[201~");
        assert_eq!(encode(&TerminalInput::Paste { text: "a\nb".into() }, Modes::default()), b"a\rb", "unbracketed: lines end as Enter sends them");
    }

    use proptest::prelude::*;

    proptest! {
        /// Whatever is pasted, it can't end the bracket early (a program
        /// would run the rest as typed).
        #[test]
        fn nothing_pasted_ends_the_bracket(text in ".*") {
            let bytes = encode(&TerminalInput::Paste { text }, Modes { bracketed_paste: true, ..Modes::default() });
            let inside = &bytes[6..bytes.len() - 6];
            prop_assert!(bytes.starts_with(b"\x1b[200~") && bytes.ends_with(b"\x1b[201~"));
            prop_assert!(!inside.contains(&0x1b));
        }

        /// Every key, with every modifier, in every mode, is some bytes.
        #[test]
        fn every_key_is_bytes(n in 0u8..=12, c in any::<char>(), which in 0usize..16, ctrl: bool, alt: bool, shift: bool, app_cursor: bool) {
            let keys = [TermKey::Enter, TermKey::Tab, TermKey::Backspace, TermKey::Escape, TermKey::Up, TermKey::Down, TermKey::Left, TermKey::Right, TermKey::Home, TermKey::End, TermKey::PageUp, TermKey::PageDown, TermKey::Insert, TermKey::Delete, TermKey::F(n.max(1)), TermKey::Char(c)];
            let bytes = encode(&key(keys[which], ctrl, alt, shift), Modes { app_cursor, ..Modes::default() });
            prop_assert!(!bytes.is_empty());
        }
    }
}
