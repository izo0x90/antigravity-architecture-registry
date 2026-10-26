//! What a message links, as an agent is handed it with the message: a
//! text for each tag or thing linked, its heading, then each of its
//! things whole (small, or asked for whole) or as a pointer (its name, its
//! size, its first lines, and how to read the rest), all within one budget
//! for the message: past it, it's only named. The things are read
//! through their owners, as everything here is; one the reader may not
//! read (`reaches`) is named, not handed on.

use harness_contracts::{Include, Kept, Subject};

use crate::{Things, some_of};

/// Up to this much a thing goes in whole on its own (`Include::Auto`).
pub const SMALL_BYTES: usize = 4096;
pub const SMALL_LINES: u64 = 100;
/// What all the things linked in one message take at most.
pub const ATTACHED_MOST: usize = 16_000;
/// A pointer's first lines, each cut to so many characters.
const POINTER_LINES: usize = 10;
const POINTER_LINE: usize = 200;

/// One tag's or thing's part: what heads it, its things, whether they go
/// in whole; or what's gone (a tag deleted since it was linked).
pub struct Attached {
    pub heading: String,
    pub include: Include,
    pub things: Vec<AttachedThing>,
    pub gone: bool,
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

/// Whether `glimpsed` (`some_of` at `most` characters) was cut.
fn cut(glimpsed: &str, most: usize) -> bool {
    glimpsed.chars().count() > most
}

/// What a part left out past the message's budget says.
fn left_out_past_budget() -> String {
    format!("(left out: the message's {} KB are spent; `tag_read` or `thing_read` reads it)", ATTACHED_MOST / 1000)
}

impl Things {
    /// The text of each of `attached`, in order, as a reader who `reaches`
    /// what it may read is handed it; and what the sender is told (a thing
    /// linked whole that didn't fit, a link to what's gone, what's left
    /// out past the budget).
    pub fn attach(&self, attached: &[Attached], reaches: &dyn Fn(&Subject) -> bool) -> (Vec<String>, Vec<String>) {
        let mut notes = Vec::new();
        let mut texts = Vec::with_capacity(attached.len());
        let mut spent = 0;
        let mut left_out = 0;
        for a in attached {
            let mut out = a.heading.clone();
            // The budget spent before its heading: only it goes.
            if ATTACHED_MOST.saturating_sub(spent) < POINTER_LINE {
                left_out += a.things.len().max(1);
                out.push_str(&format!("\n{}", left_out_past_budget()));
                texts.push(out);
                continue;
            }
            if a.gone {
                out.push_str("\n(gone since it was linked: nothing of it)");
                notes.push(format!("{}: gone since it was linked: nothing of it goes.", a.heading));
            } else if a.things.is_empty() {
                out.push_str("\n(on nothing now)");
            }
            let mut its_left_out = 0;
            for t in &a.things {
                if !reaches(&t.subject) {
                    out.push_str(&format!("\n- {}: (not handed on: this reader can't read it)", t.what));
                    notes.push(format!("{}: the agent can't read it: only its name goes.", t.what));
                    continue;
                }
                let room = ATTACHED_MOST.saturating_sub(spent + out.len());
                // The budget spent: counted, said once in the piece.
                if room < POINTER_LINE {
                    its_left_out += 1;
                    continue;
                }
                // Read once: its start (enough to tell small from big, and a pointer's lines).
                let (held, sample) = self.glimpse(&t.subject, t.kept.as_ref(), SMALL_BYTES);
                let what = match held {
                    harness_contracts::Held::Source => t.what.clone(),
                    held => format!("{} ({})", t.what, held.words()),
                };
                let Some((sample, lines)) = sample else {
                    out.push_str(&format!("\n- {what}: (nothing of it is here)"));
                    continue;
                };
                let small = !cut(&sample, SMALL_BYTES) && lines <= SMALL_LINES;
                let whole = match a.include {
                    Include::Auto => small,
                    Include::Whole => true,
                    Include::Pointer => false,
                };
                if whole {
                    let text = if small { Some(sample.clone()) } else { self.glimpse(&t.subject, t.kept.as_ref(), room).1.map(|(t, _)| t) };
                    if let Some(text) = text.filter(|text| !cut(text, room) && text.len() <= room) {
                        out.push_str(&format!("\n- {what}:\n{}", indent(&self.fold(&t.subject, &text, t.whole))));
                        continue;
                    }
                    if a.include == Include::Whole {
                        notes.push(format!(
                            "{}: too big to go in whole with the rest (past {} KB): it goes as its start and how to read the rest.",
                            t.what,
                            ATTACHED_MOST / 1000
                        ));
                    }
                }
                // Its first lines, each and all cut short to fit.
                let head: Vec<String> = sample.lines().take(POINTER_LINES).map(|l| some_of(l.to_owned(), POINTER_LINE)).collect();
                let head = some_of(head.join("\n"), room.saturating_sub(POINTER_LINE).min(POINTER_LINES * POINTER_LINE));
                let more = if lines as usize > POINTER_LINES { "\n  …" } else { "" };
                out.push_str(&format!(
                    "\n- {what} ({lines} lines; its start only: {} reads the rest):\n{}{more}",
                    t.read_by,
                    indent(&head)
                ));
            }
            if its_left_out > 0 {
                out.push_str(&format!("\n(and {its_left_out} more of it, {})", left_out_past_budget().trim_start_matches('(')));
                left_out += its_left_out;
            }
            spent += out.len();
            texts.push(out);
        }
        if left_out > 0 {
            notes.push(format!("{left_out} linked thing(s) left out: the message's {} KB were spent.", ATTACHED_MOST / 1000));
        }
        (texts, notes)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use harness_contracts::{Include, Subject, TerminalId};

    use super::*;
    use crate::testing::Lines;

    /// Things whose text is `n` lines of `width` characters each.
    fn things() -> (tempfile::TempDir, Arc<Things>, Arc<Lines>) {
        let dir = tempfile::tempdir().unwrap();
        let things = Things::open(dir.path(), 1 << 20);
        let owner = Arc::new(Lines::default());
        things.register(owner.clone());
        (dir, things, owner)
    }

    fn lines(first: u64, last: u64) -> Subject {
        Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command: None }
    }

    fn one(subject: Subject, include: Include) -> Attached {
        Attached {
            heading: "@it".into(),
            include,
            things: vec![AttachedThing {
                what: "it".into(),
                subject,
                kept: None,
                read_by: "`thing_read` (\"it\")".into(),
                whole: "`thing_read`",
            }],
            gone: false,
        }
    }

    /// All the texts, as the agent reads them.
    fn all(texts: &[String]) -> String {
        texts.join("\n")
    }

    #[test]
    fn small_goes_whole_big_as_a_capped_pointer_and_the_budget_holds_for_all() {
        let (_dir, things, _owner) = things();
        let every = |_: &Subject| true;
        // The fixture says "text of lines a to b": one line, small.
        let (small, notes) = things.attach(&[one(lines(1, 2), Include::Auto)], &every);
        assert!(small[0].starts_with("@it\n- it:\n  text of lines 1 to 2") && notes.is_empty(), "{}", all(&small));
        // Linked as a pointer: its start, and how to read the rest.
        let (pointer, _) = things.attach(&[one(lines(1, 2), Include::Pointer)], &every);
        assert!(pointer[0].contains("- it (1 lines; its start only: `thing_read` (\"it\") reads the rest)"), "{}", all(&pointer));
        // Many linked: a text each; what goes in stays within the budget,
        // the rest named, said.
        let many: Vec<Attached> = (0..2000).map(|n| one(lines(n, n + 1), Include::Pointer)).collect();
        let (spent, notes) = things.attach(&many, &every);
        assert_eq!(spent.len(), 2000);
        let handed: usize = spent.iter().filter(|t| !t.contains("left out")).map(String::len).sum();
        assert!(handed < ATTACHED_MOST + 2 * POINTER_LINE * POINTER_LINES, "{handed}");
        assert!(
            spent.last().unwrap().contains("(left out: the message's 16 KB are spent") && notes.iter().any(|n| n.contains("left out")),
            "{notes:?}"
        );
        // Not the reader's to read: named, not handed on, and said.
        let (hidden, notes) = things.attach(&[one(lines(1, 2), Include::Whole)], &|_: &Subject| false);
        assert!(
            hidden[0].contains("(not handed on: this reader can't read it)") && !hidden[0].contains("text of lines") && notes.len() == 1,
            "{}",
            all(&hidden)
        );
        // A link to what's gone: said.
        let (gone, notes) =
            things.attach(&[Attached { heading: "#flaky".into(), include: Include::Auto, things: Vec::new(), gone: true }], &every);
        assert!(gone[0].contains("(gone since it was linked") && notes[0].starts_with("#flaky: gone"), "{gone:?}");
        // Text that ends in … isn't taken as cut.
        assert!(!cut("Wait…", 10) && cut(&some_of("a".repeat(20), 10), 10));
    }
}
