//! The commands the shell ran, from its marks (`scan`): where each one's
//! prompt, typing and output are (by line number), the command line as
//! the shell said it, how it ended and how long it ran. Kept while any of
//! its lines is (a running one always).

use std::time::{Duration, Instant};

use harness_contracts::TerminalCommand;

use crate::feed::At;
use crate::scan::Seen;

/// The most kept (the oldest go first).
const MOST: usize = 5000;

/// The commands, oldest first (their numbers in order).
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

impl Kept {
    /// At its prompt: nothing run yet.
    fn waiting(&self) -> bool {
        self.command.output.is_none()
    }
}

impl Commands {
    /// A mark the shell made, `at` where it was, in change `seq`. A command
    /// that finished: its code and how long it ran.
    pub fn mark(&mut self, seen: &Seen, at: At, seq: u64) -> Option<(Option<i32>, Duration)> {
        let open = self.list.last_mut().filter(|k| k.command.end.is_none());
        match (seen, open) {
            // A prompt drawn again (Ctrl-L, a list of completions, Ctrl-C):
            // the same command, waiting at its new place.
            (Seen::Prompt, Some(kept)) if kept.waiting() => {
                (kept.command.prompt, kept.command.input, kept.command.input_col, kept.command.text, kept.changed) =
                    (at.line, at.line, at.col, String::new(), seq);
            }
            (Seen::Prompt, _) => self.add(at, seq),
            (Seen::Input, Some(kept)) if kept.waiting() => {
                (kept.command.input, kept.command.input_col, kept.changed) = (at.line, at.col, seq)
            }
            (Seen::Typed(text), Some(kept)) if kept.waiting() => (kept.command.text, kept.changed) = (text.clone(), seq),
            (Seen::Started, open) => {
                // No prompt was marked (the shell's prompt is set up after
                // the marks): it starts here.
                if !open.is_some_and(|k| k.waiting()) {
                    self.add(at, seq);
                }
                if let Some(kept) = self.list.last_mut() {
                    (kept.command.output, kept.started, kept.changed) = (Some(at.line), Some(Instant::now()), seq);
                }
            }
            (Seen::Finished(code), Some(kept)) if !kept.waiting() => {
                let ran = kept.started.map_or(Duration::ZERO, |s| s.elapsed());
                // Output that didn't end its line ends with the line.
                let end = if at.col > 0 { at.line + 1 } else { at.line };
                (kept.command.end, kept.command.code, kept.command.ran_ms, kept.changed) =
                    (Some(end), *code, Some(ran.as_millis() as u64), seq);
                return Some((*code, ran));
            }
            _ => {}
        }
        None
    }

    fn add(&mut self, at: At, seq: u64) {
        self.next += 1;
        let command = TerminalCommand {
            id: self.next,
            prompt: at.line,
            input: at.line,
            input_col: at.col,
            output: None,
            end: None,
            text: String::new(),
            code: None,
            ran_ms: None,
        };
        self.list.push(Kept { command, started: None, changed: seq });
        if self.list.len() > MOST {
            self.list.remove(0);
        }
    }

    /// Those changed after `since` (all of them given none).
    pub fn since(&self, since: Option<u64>) -> Vec<TerminalCommand> {
        self.list.iter().filter(|k| since.is_none_or(|s| k.changed > s)).map(|k| k.command.clone()).collect()
    }

    /// Whether any command has lines before `oldest`.
    pub fn any_before(&self, oldest: u64) -> bool {
        self.list.first().is_some_and(|k| k.command.prompt < oldest)
    }

    /// Lines before `oldest` are gone (cleared, or past what's kept): so is
    /// a command all of whose lines were there; one that's running stays,
    /// from the oldest line there is.
    pub fn gone_before(&mut self, oldest: u64, seq: u64) {
        self.list.retain(|k| k.command.end.is_none_or(|end| end > oldest));
        for kept in self.list.iter_mut().filter(|k| k.command.prompt < oldest) {
            let c = &mut kept.command;
            c.prompt = oldest;
            if c.input < oldest {
                (c.input, c.input_col) = (oldest, 0);
            }
            c.output = c.output.map(|l| l.max(oldest));
            kept.changed = seq;
        }
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

    /// The lines were laid out anew where they can't be read (a program has
    /// the whole screen): only the one running stays, from `line`.
    pub fn lost_places(&mut self, line: u64, seq: u64) {
        self.list.retain(|k| k.command.end.is_none());
        for kept in &mut self.list {
            let c = &mut kept.command;
            (c.prompt, c.input, c.input_col) = (line, line, 0);
            c.output = c.output.map(|_| line);
            kept.changed = seq;
        }
    }

    /// The commands, oldest first.
    pub fn list(&self) -> impl DoubleEndedIterator<Item = &TerminalCommand> {
        self.list.iter().map(|k| &k.command)
    }

    /// Command `id`.
    pub fn get(&self, id: u64) -> Option<&TerminalCommand> {
        self.list.iter().rev().map(|k| &k.command).find(|c| c.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u64, col: u16) -> At {
        At { line, col }
    }

    #[test]
    fn a_command_is_its_prompt_its_line_as_typed_its_output_and_its_end() {
        let mut commands = Commands::default();
        commands.mark(&Seen::Prompt, at(10, 0), 1);
        commands.mark(&Seen::Input, at(10, 2), 2);
        commands.mark(&Seen::Typed("make".into()), at(11, 0), 3);
        commands.mark(&Seen::Started, at(11, 0), 3);
        assert_eq!(commands.mark(&Seen::Finished(Some(1)), at(14, 0), 4).map(|(code, _)| code), Some(Some(1)));
        // A prompt drawn again, or left with nothing run: the same one waiting.
        commands.mark(&Seen::Prompt, at(14, 0), 5);
        commands.mark(&Seen::Input, at(14, 2), 5);
        commands.mark(&Seen::Prompt, at(15, 0), 6);
        commands.mark(&Seen::Input, at(15, 2), 6);
        let all = commands.since(None);
        assert_eq!(all.iter().map(|c| (c.id, c.prompt)).collect::<Vec<_>>(), [(1, 10), (2, 15)], "{all:?}");
        let first = &all[0];
        assert_eq!(
            (first.input, first.input_col, first.output, first.end, first.code, first.text.as_str()),
            (10, 2, Some(11), Some(14), Some(1), "make")
        );
        assert_eq!(commands.since(Some(5)).len(), 1, "only the one changed after");
    }

    #[test]
    fn a_command_with_no_prompt_marked_still_runs_and_ends() {
        let mut commands = Commands::default();
        commands.mark(&Seen::Started, at(3, 0), 1);
        assert!(commands.mark(&Seen::Finished(Some(0)), at(9, 0), 2).is_some(), "its end is told");
    }

    #[test]
    fn a_running_command_outlives_its_first_lines_and_a_finished_one_goes_with_its_last() {
        let mut commands = Commands::default();
        for (prompt, end) in [(10, Some(14)), (14, None)] {
            commands.mark(&Seen::Prompt, at(prompt, 0), 1);
            commands.mark(&Seen::Started, at(prompt + 1, 0), 1);
            if let Some(end) = end {
                commands.mark(&Seen::Finished(Some(0)), at(end, 0), 1);
            }
        }
        // `clear` while the second runs: the scrollback goes.
        commands.gone_before(100, 2);
        let left = commands.since(None);
        assert_eq!(left.iter().map(|c| (c.id, c.prompt, c.output)).collect::<Vec<_>>(), [(2, 100, Some(100))]);
        assert!(commands.mark(&Seen::Finished(Some(0)), at(120, 0), 3).is_some(), "its end is still told");
    }
}
