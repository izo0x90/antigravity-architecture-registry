//! Where commands start and end, as the shell marks them (OSC 133, the
//! marks VS Code and iTerm read): `ESC ] 133 ; C` when a command starts
//! running, `ESC ] 133 ; D ; <code>` when it's done. Read from the output as
//! it comes, a mark split across reads included.

/// A mark the shell made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A command started running.
    Started,
    /// The command ended, with its exit code when given.
    Finished(Option<i32>),
}

/// Reads marks out of a terminal's output.
#[derive(Default)]
pub struct Marks {
    /// Bytes of an escape sequence not finished in the last read.
    partial: Vec<u8>,
    in_osc: bool,
    escaped: bool,
}

/// An OSC longer than this isn't a mark (and isn't kept).
const LONGEST: usize = 64;

impl Marks {
    /// The marks in `bytes` (the output just read).
    pub fn read(&mut self, bytes: &[u8]) -> Vec<Mark> {
        let mut marks = Vec::new();
        for &b in bytes {
            if !self.in_osc {
                // `ESC ]` starts one.
                if self.escaped && b == b']' {
                    self.in_osc = true;
                    self.partial.clear();
                }
                self.escaped = b == 0x1b;
                continue;
            }
            // Ended by BEL, or by ESC \ (the ESC is kept until the `\`).
            let end = b == 0x07 || (self.escaped && b == b'\\');
            if end {
                if self.escaped {
                    self.partial.pop();
                }
                marks.extend(parse(&self.partial));
                self.in_osc = false;
                self.escaped = false;
                self.partial.clear();
                continue;
            }
            self.escaped = b == 0x1b;
            if self.partial.len() < LONGEST {
                self.partial.push(b);
            }
        }
        marks
    }
}

fn parse(osc: &[u8]) -> Option<Mark> {
    let text = std::str::from_utf8(osc).ok()?;
    let mut parts = text.split(';');
    if parts.next()? != "133" {
        return None;
    }
    match parts.next()? {
        "C" => Some(Mark::Started),
        "D" => Some(Mark::Finished(parts.next().and_then(|c| c.parse().ok()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_read_out_of_output_even_split_across_reads() {
        let mut marks = Marks::default();
        assert_eq!(marks.read(b"$ make\x1b]133;C\x07building\r\n"), [Mark::Started]);
        assert_eq!(marks.read(b"done\r\n\x1b]13"), []);
        assert_eq!(marks.read(b"3;D;2\x1b\\$ "), [Mark::Finished(Some(2))], "split, ended by ESC \\");
        assert_eq!(marks.read(b"\x1b]0;title\x07\x1b]133;D\x07"), [Mark::Finished(None)], "another OSC isn't one");
        assert_eq!(marks.read(b"\x1b[31mred\x1b[0m"), [], "colours aren't");
    }
}
