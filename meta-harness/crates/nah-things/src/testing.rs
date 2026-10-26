//! For tests: an owner of made-up terminal lines.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use harness_contracts::{Subject, TerminalId};

use crate::{More, Resolve};

/// Owns terminal t1's lines (and t2's, which it keeps for good): lines
/// `a` to `b` are named "lines a to b" and say "text of lines a to b";
/// those from line 0 are gone; those from 100 to 999 were laid out anew
/// (once `relay` is called) and are `relaid` thousand lines on now; line 7
/// says lines of detail; lines 600 to 699 are kept with the command they're
/// in (command 6). It doesn't keep t1's for good: once
/// `scroll_away` is called, none is there. Lines from 500 on are still
/// growing: `grow` adds to them.
#[derive(Default)]
pub struct Lines {
    pub relaid: AtomicU64,
    gone: std::sync::atomic::AtomicBool,
    growing: Mutex<Vec<More>>,
    /// How many things it was told are noted, each time it was told.
    pub told: Mutex<Vec<usize>>,
}

impl Lines {
    /// Lays lines 100 to 999 out anew (once more).
    pub fn relay(&self) {
        self.relaid.fetch_add(1, Ordering::SeqCst);
    }

    /// Every line of t1 goes (past what's kept).
    pub fn scroll_away(&self) {
        self.gone.store(true, Ordering::SeqCst);
    }

    /// The growing lines add `text` (told to each who still wants it).
    pub fn grow(&self, text: &str) {
        self.growing.lock().unwrap().retain_mut(|more| more(text));
    }

    /// How many follow the growing lines.
    pub fn followers(&self) -> usize {
        self.growing.lock().unwrap().len()
    }
}

fn kept_for_good(subject: &Subject) -> bool {
    matches!(subject, Subject::TerminalLines { terminal, .. } if terminal.as_str() == "t2")
}

impl Resolve for Lines {
    fn owns(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::TerminalLines { .. })
    }

    fn name(&self, subject: &Subject) -> String {
        let Subject::TerminalLines { first, last, .. } = subject else { return String::new() };
        format!("lines {first} to {last}")
    }

    fn now(&self, subject: &Subject) -> Option<String> {
        let Subject::TerminalLines { first, last, .. } = subject else { return None };
        (*first > 0 && (kept_for_good(subject) || !self.gone.load(Ordering::SeqCst))).then(|| format!("text of lines {first} to {last}\n"))
    }

    fn named(&self, words: &str) -> Option<Result<Subject, String>> {
        let (first, last) = words.strip_prefix("lines ")?.split_once(" to ")?;
        Some(
            first
                .parse()
                .and_then(|first| {
                    Ok(Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last: last.parse()?, command: None })
                })
                .map_err(|e| e.to_string()),
        )
    }

    fn moved(&self, subject: &Subject) -> Option<Subject> {
        let Subject::TerminalLines { terminal, shell, first, last, command } = subject else { return None };
        let by = 1000 * self.relaid.load(Ordering::SeqCst);
        (by > 0 && (100..1000).contains(first)).then(|| Subject::TerminalLines {
            terminal: terminal.clone(),
            shell: *shell,
            first: first + by,
            last: last + by,
            command: *command,
        })
    }

    fn lasts(&self, subject: &Subject) -> bool {
        kept_for_good(subject)
    }

    fn settle(&self, subject: &Subject) -> Result<Subject, String> {
        Ok(match subject.clone() {
            Subject::TerminalLines { terminal, shell, first, last, command: None } if (600..700).contains(&first) => {
                Subject::TerminalLines { terminal, shell, first, last, command: Some(6) }
            }
            other => other,
        })
    }

    fn follow(&self, subject: &Subject, more: More) -> bool {
        let Subject::TerminalLines { first, .. } = subject else { return false };
        if *first < 500 {
            return false;
        }
        let mut more = more;
        if !more(&self.now(subject).unwrap_or_default()) {
            return false;
        }
        self.growing.lock().unwrap().push(more);
        true
    }

    fn detailed(&self, subject: &Subject) -> bool {
        matches!(subject, Subject::TerminalLines { first: 7, .. })
    }

    fn noted(&self, all: &[Subject]) {
        self.told.lock().unwrap().push(all.len());
    }
}
