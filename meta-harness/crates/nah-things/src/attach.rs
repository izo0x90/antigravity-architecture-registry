//! What a message links, put after its text as an agent is handed it:
//! each link's heading, then each of its things whole (small, or asked
//! for whole) or as a pointer (its name, its size, its first lines, and
//! how to read the rest), within one budget for the message. The things
//! are read through their owners, as everything here is. Only the user
//! links things, so everything linked is handed on.

use harness_contracts::{Expanded, Include, Kept, Subject};

use crate::{Things, some_of};

/// Up to this much a thing goes in whole on its own (`Include::Auto`).
pub const SMALL_BYTES: usize = 4096;
pub const SMALL_LINES: u64 = 100;
/// What all the things linked in one message take at most.
pub const ATTACHED_MOST: usize = 16_000;
/// A pointer's first lines.
const POINTER_LINES: u64 = 10;

/// One link's part: what heads it, its things, whether they go in whole.
pub struct Attached {
    pub heading: String,
    pub include: Include,
    pub things: Vec<AttachedThing>,
}

/// A thing linked: its name, what it is, where it's kept (a tagged
/// thing's log), and how an agent reads the rest of it.
pub struct AttachedThing {
    pub what: String,
    pub subject: Subject,
    pub kept: Option<Kept>,
    pub read_by: String,
    /// How the reader gets what's folded out of it (a turn's tool output).
    pub whole: &'static str,
}

fn indent(text: &str) -> String {
    text.lines().map(|l| format!("  {l}")).collect::<Vec<_>>().join("\n")
}

impl Things {
    /// `text` with what `attached` links after it.
    pub fn attach(&self, text: &str, attached: &[Attached]) -> Expanded {
        if attached.is_empty() {
            return Expanded { text: text.to_owned(), notes: Vec::new() };
        }
        let mut notes = Vec::new();
        let mut out = format!("{text}\n\n---\nWhat's linked above, as it is now:");
        let start = out.len();
        for a in attached {
            out.push_str(&format!("\n\n{}", a.heading));
            if a.things.is_empty() {
                out.push_str("\n(on nothing now)");
            }
            for t in &a.things {
                let (held, sample) = self.glimpse(&t.subject, t.kept.as_ref(), SMALL_BYTES + 1);
                let what = match held {
                    harness_contracts::Held::Source => t.what.clone(),
                    held => format!("{} ({})", t.what, held.words()),
                };
                let Some((sample, lines)) = sample else {
                    out.push_str(&format!("\n- {what}: (nothing of it is here)"));
                    continue;
                };
                let small = !sample.ends_with('…') && sample.len() <= SMALL_BYTES && lines <= SMALL_LINES;
                let whole = match a.include {
                    Include::Auto => small,
                    Include::Whole => true,
                    Include::Pointer => false,
                };
                if whole {
                    let room = ATTACHED_MOST.saturating_sub(out.len() - start);
                    let text = if small { sample.clone() } else { self.glimpse(&t.subject, t.kept.as_ref(), room).1.map(|(t, _)| t).unwrap_or_default() };
                    if !text.ends_with('…') && text.len() <= room {
                        out.push_str(&format!("\n- {what}:\n{}", indent(&self.fold(&t.subject, &text, t.whole))));
                        continue;
                    }
                    if a.include == Include::Whole {
                        notes.push(format!("{}: too big to go in whole with the rest (past {} KB): sent as a pointer.", t.what, ATTACHED_MOST / 1000));
                    }
                }
                let head = self.lines(&t.subject, t.kept.as_ref(), 1, POINTER_LINES).1.map(|(shown, _)| shown.join("\n")).unwrap_or_else(|| some_of(sample, 400));
                let more = if lines > POINTER_LINES { "\n  …" } else { "" };
                out.push_str(&format!("\n- {what} ({lines} lines; its start only: {} reads the rest):\n{}{more}", t.read_by, indent(&head)));
            }
        }
        Expanded { text: out, notes }
    }
}
