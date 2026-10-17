//! The commands the shell ran, from its marks (`scan`): where each one's
//! prompt, typing and output are (by line number), what was typed, how it
//! ended and how long it ran. Kept while its prompt's line is.

use std::time::{Duration, Instant};

use harness_contracts::TerminalCommand;

use crate::feed::At;
use crate::scan::Seen;

/// The most kept (the oldest go first).
const MOST: usize = 5000;

#[derive(Default)]
pub struct Commands {
    list: Vec<Kept>,
    next: u64,
}

struct Kept {
    command: TerminalCommand,
    /// When it started running.
    started: Option<Instant>,
    /// The change it last changed in (`Changes`' count).
    changed: u64,
}

impl Commands {
    /// A mark the shell made, `at` where it was, in change `seq`: what was
    /// typed read with `typed` (from where typing started to the line the
    /// output starts on). A command that finished: its code and how long
    /// it ran.
    pub fn mark(&mut self, seen: Seen, at: At, seq: u64, typed: impl FnOnce(At, u64) -> String) -> Option<(Option<i32>, Duration)> {
        let open = self.list.last_mut().filter(|k| k.command.end.is_none());
        match (seen, open) {
            (Seen::Prompt, open) => {
                // A prompt left with nothing run (Enter on an empty line) isn't one.
                if open.is_some_and(|k| k.command.output.is_none()) {
                    self.list.pop();
                }
                self.next += 1;
                let command = TerminalCommand { id: self.next, prompt: at.line, input: at.line, input_col: at.col, output: None, end: None, text: String::new(), code: None, ran_ms: None };
                self.list.push(Kept { command, started: None, changed: seq });
                if self.list.len() > MOST {
                    self.list.remove(0);
                }
            }
            (Seen::Input, Some(kept)) if kept.command.output.is_none() => {
                (kept.command.input, kept.command.input_col, kept.changed) = (at.line, at.col, seq);
            }
            (Seen::Started, Some(kept)) if kept.command.output.is_none() => {
                kept.command.text = typed(At { line: kept.command.input, col: kept.command.input_col }, at.line);
                (kept.command.output, kept.started, kept.changed) = (Some(at.line), Some(Instant::now()), seq);
            }
            (Seen::Finished(code), Some(kept)) if kept.command.output.is_some() => {
                let ran = kept.started.map_or(Duration::ZERO, |s| s.elapsed());
                // Output that didn't end its line ends with the line.
                let end = if at.col > 0 { at.line + 1 } else { at.line };
                (kept.command.end, kept.command.code, kept.command.ran_ms, kept.changed) = (Some(end), code, Some(ran.as_millis() as u64), seq);
                return Some((code, ran));
            }
            _ => {}
        }
        None
    }

    /// Those changed after `since` (all of them given none).
    pub fn since(&self, since: Option<u64>) -> Vec<TerminalCommand> {
        self.list.iter().filter(|k| since.is_none_or(|s| k.changed > s)).map(|k| k.command.clone()).collect()
    }

    /// Lines before `oldest` are gone: so are the commands whose prompt was there.
    pub fn gone_before(&mut self, oldest: u64) {
        self.list.retain(|k| k.command.prompt >= oldest);
    }

    /// The lines were laid out anew: each place moved by `to` (none: it's
    /// gone, and so is its command), the line after a line by `after`, in
    /// change `seq`.
    pub fn moved(&mut self, seq: u64, to: impl Fn(u64, u16) -> Option<(u64, u16)>, after: impl Fn(u64) -> Option<u64>) {
        self.list.retain_mut(|k| {
            let c = &mut k.command;
            let (Some((prompt, _)), Some((input, input_col))) = (to(c.prompt, 0), to(c.input, c.input_col)) else { return false };
            (c.prompt, c.input, c.input_col) = (prompt, input, input_col);
            c.output = c.output.and_then(|l| to(l, 0)).map(|(l, _)| l);
            c.end = c.end.and_then(|l| after(l.checked_sub(1)?));
            k.changed = seq;
            true
        });
    }

    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// The commands, oldest first.
    pub fn list(&self) -> impl DoubleEndedIterator<Item = &TerminalCommand> {
        self.list.iter().map(|k| &k.command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u64, col: u16) -> At {
        At { line, col }
    }

    #[test]
    fn a_command_is_its_prompt_its_typing_its_output_and_its_end() {
        let mut commands = Commands::default();
        commands.mark(Seen::Prompt, at(10, 0), 1, |_, _| unreachable!());
        commands.mark(Seen::Input, at(10, 2), 2, |_, _| unreachable!());
        commands.mark(Seen::Started, at(11, 0), 3, |from, to| format!("{}:{} to {to}", from.line, from.col));
        assert_eq!(commands.mark(Seen::Finished(Some(1)), at(14, 0), 4, |_, _| unreachable!()).map(|(code, _)| code), Some(Some(1)));
        // Enter on an empty prompt: not a command.
        commands.mark(Seen::Prompt, at(14, 0), 5, |_, _| unreachable!());
        commands.mark(Seen::Input, at(14, 2), 5, |_, _| unreachable!());
        commands.mark(Seen::Prompt, at(15, 0), 6, |_, _| unreachable!());
        let all = commands.since(None);
        assert_eq!(all.len(), 2, "{all:?}");
        let first = &all[0];
        assert_eq!((first.prompt, first.input, first.input_col, first.output, first.end, first.code), (10, 10, 2, Some(11), Some(14), Some(1)));
        assert_eq!(first.text, "10:2 to 11");
        assert_eq!(commands.since(Some(4)).len(), 1, "only the one changed after");
        commands.gone_before(11);
        assert_eq!(commands.since(None).len(), 1, "its prompt's line gone, it's gone");
    }
}
