//! Things in the project, as every kind of note on them (tags,
//! discussions) points at them: one layer. A thing (`Subject`) is its
//! owner's (the terminals, the chats, the code...), which names it, says
//! what it says now, and where it is when it moved it (`Resolve`); owners
//! register when they're made and say when they moved their things
//! (`moved`). A thing its owner doesn't keep for good (a terminal's
//! output) is written to the logs when it's noted, and given back from
//! there once its source is gone; one still growing (a command running)
//! is followed to its end, up to the project's cap. Reads give a thing's
//! start, its line count, some of its lines, or a search through all of
//! them (FFF over the logs). Every kind of note registers (`Notes`), so
//! the logs no note points into are removed, and moves reach every note.
//! This crate knows no owner and no kind of note.

mod logs;
#[cfg(feature = "testing")]
pub mod testing;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, Weak};

use harness_contracts::{DETAIL, Held, Kept, Subject};

use crate::logs::Logs;

/// The most of a growing thing (a command running) its log keeps, unless
/// the project says otherwise.
pub const GROWN_MOST: u64 = 64 << 20;

/// An owner of things: its name for one, what one says now. Each says
/// which kinds are its own (`owns`); asked about one of them only.
pub trait Resolve: Send + Sync {
    /// Whether `subject` is of a kind it owns (by its kind alone: no work).
    fn owns(&self, subject: &Subject) -> bool;
    /// `subject`'s name (`Subject::name`, with what it knows of it now).
    fn name(&self, subject: &Subject) -> String;
    /// What `subject` says now (none: it's gone).
    fn now(&self, subject: &Subject) -> Option<String>;
    /// The thing `words` name (as `name` names it), when they're of its
    /// kind: an error says why they don't rightly name one of its own.
    fn named(&self, words: &str) -> Option<Result<Subject, String>>;
    /// Where `subject` is now, when its owner moved it since (lines laid
    /// out anew): none when it's where it was.
    fn moved(&self, _subject: &Subject) -> Option<Subject> {
        None
    }
    /// `subject` as it's kept once noted (lines of a file: with the
    /// snapshot of the files they're in; a terminal's lines: with the
    /// command they're in); an error says why it can't be.
    fn settle(&self, subject: &Subject) -> Result<Subject, String> {
        Ok(subject.clone())
    }
    /// Whether it keeps `subject` for good (code does); one it doesn't (a
    /// terminal's output) is written to the logs when it's noted.
    fn lasts(&self, _subject: &Subject) -> bool {
        true
    }
    /// Whether `subject` is still where it was, all of it (a command
    /// partly scrolled away isn't).
    fn there(&self, subject: &Subject) -> bool {
        self.now(subject).is_some()
    }
    /// `subject` is still growing (a command running): `more` is told what
    /// it says so far, then what it adds, until it ends or `more` says to
    /// stop (false: no note is on it now); then the owner drops it. False:
    /// it isn't growing.
    fn follow(&self, _subject: &Subject, _more: More) -> bool {
        false
    }
    /// Whether what `subject` says has lines of detail (`DETAIL`: a chat
    /// turn's tool output), which reads fold unless asked for in full; no
    /// other thing's lines are ever folded.
    fn detailed(&self, _subject: &Subject) -> bool {
        false
    }
    /// The things noted now, by every kind of note (its own among them):
    /// what it keeps for things no longer noted can go (code: snapshots).
    fn noted(&self, _all: &[Subject]) {}
}

/// What a growing thing adds (`Resolve::follow`): false when it's no
/// longer wanted.
pub type More = Box<dyn FnMut(&str) -> bool + Send>;

/// Some lines of what a thing says, and how many it has in all.
pub type Window = (Vec<String>, u64);

/// A kind of note on things (tags, discussions).
pub trait Notes: Send + Sync {
    /// The logs' files its notes point into: all of them, those of notes
    /// this build can't read too (an error: none may be removed).
    fn kept_files(&self) -> Result<BTreeSet<String>, String>;
    /// The things its notes are on.
    fn subjects(&self) -> Vec<Subject>;
    /// Owners moved their things (or their names changed): its notes keep
    /// them where they are now, named as they are now.
    fn refresh(&self);
}

/// A thing as a note keeps it: where it is (settled), its name, and where
/// what it says is in the logs (none: its owner keeps it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Noting {
    pub subject: Subject,
    pub what: String,
    pub kept: Option<Kept>,
}

/// A line found in what things say (`Things::find`): which of them (its
/// place in what was searched), which of its lines (from 1), the line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub which: usize,
    pub line: u64,
    pub text: String,
}

/// What a search through the logs is told: each line's text is searched
/// in, as written.
const SEARCH_BUDGET_MS: u64 = 2000;

pub struct Things {
    owners: RwLock<Vec<Arc<dyn Resolve>>>,
    notes: RwLock<Vec<Weak<dyn Notes>>>,
    logs: Logs,
    /// Each growing thing's log file, and whether it's still written to
    /// (off once nothing points into it).
    growing: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    grown_most: u64,
    /// The logs as FFF lists them (once there are some).
    search: Mutex<Option<arch_search::RepoFiles>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a thing is, as said beside what it says: nothing of it given,
/// gone; given though it's gone from where it was (its owner keeps what it
/// was: code at a snapshot), as it was.
fn given(held: Held, text: bool) -> Held {
    match (held, text) {
        (_, false) => Held::Gone,
        (Held::Gone, true) => Held::Kept,
        (held, true) => held,
    }
}

/// Some of a text, when it's long (`most` characters), marked so.
pub fn some_of(text: String, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// Whether `line` has `text` in it, as a search here matches (capitals
/// heeded only when `text` has some), as FFF matches the logs.
fn has(line: &str, text: &str) -> bool {
    if text.chars().any(char::is_uppercase) { line.contains(text) } else { line.to_lowercase().contains(&text.to_lowercase()) }
}

impl Things {
    /// The things' logs in `logs`; a growing thing's kept up to
    /// `grown_most` bytes.
    pub fn open(logs: &Path, grown_most: u64) -> Arc<Self> {
        Arc::new(Self { owners: RwLock::default(), notes: RwLock::default(), logs: Logs::open(logs), growing: Mutex::default(), grown_most, search: Mutex::default() })
    }

    /// An owner of things, from now on (the things noted named as it
    /// names them now).
    pub fn register(&self, owner: Arc<dyn Resolve>) {
        self.owners.write().unwrap_or_else(PoisonError::into_inner).push(owner);
        self.moved();
    }

    /// A kind of note, from now on (told of moves; its logs kept).
    pub fn noting(&self, notes: Weak<dyn Notes>) {
        self.notes.write().unwrap_or_else(PoisonError::into_inner).push(notes);
    }

    fn notes(&self) -> Vec<Arc<dyn Notes>> {
        self.notes.read().unwrap_or_else(PoisonError::into_inner).iter().filter_map(Weak::upgrade).collect()
    }

    fn owners(&self) -> RwLockReadGuard<'_, Vec<Arc<dyn Resolve>>> {
        self.owners.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Owners moved their things (lines laid out anew, a shell started
    /// again, files changed): every note keeps them where they are now
    /// (and owners let go of what they kept for where they were).
    pub fn moved(&self) {
        for notes in self.notes() {
            notes.refresh();
        }
        self.tell_noted();
    }

    /// Every owner told what's noted now.
    fn tell_noted(&self) {
        let all: Vec<Subject> = self.notes().iter().flat_map(|n| n.subjects()).collect();
        for owner in self.owners().iter() {
            owner.noted(&all);
        }
    }

    /// The logs no note points into, removed (what a crash left behind:
    /// written, never kept). Once every kind of note is here; none when a
    /// kind of note can't say which it points into.
    pub fn forget_unused(&self) {
        let mut used = BTreeSet::new();
        for notes in self.notes() {
            match notes.kept_files() {
                Ok(files) => used.extend(files),
                Err(e) => {
                    tracing::warn!("The logs weren't cleaned up: which are used couldn't be read: {e}");
                    return;
                }
            }
        }
        let growing = lock(&self.growing);
        for (file, on) in growing.iter() {
            if !used.contains(file) {
                on.store(false, Ordering::Relaxed);
            }
        }
        self.logs.forget_all_but(&used);
        drop(growing);
        self.tell_noted();
    }

    /// The owner of `subject` (none here).
    pub fn owner(&self, subject: &Subject) -> Option<Arc<dyn Resolve>> {
        self.owners().iter().find(|o| o.owns(subject)).cloned()
    }

    /// `subject`'s name, as its owner names it (none: no owner here).
    pub fn name(&self, subject: &Subject) -> Option<String> {
        self.owner(subject).map(|o| o.name(subject))
    }

    /// The thing `words` name, as its owner names it. (An owner that names
    /// it rightly over one that only took the words for its own and says
    /// why they don't fit.)
    pub fn named(&self, words: &str) -> Result<Subject, String> {
        let tries: Vec<Result<Subject, String>> = self.owners().iter().filter_map(|o| o.named(words)).collect();
        let why = match tries.iter().find_map(|t| t.as_ref().err().cloned()) {
            Some(why) => why,
            None => format!("nothing here names {words:?}"),
        };
        tries.into_iter().find_map(Result::ok).ok_or(why)
    }

    /// Where `subject` is now (where it was, when its owner didn't move it).
    pub fn current(&self, subject: &Subject) -> Subject {
        self.owner(subject).and_then(|o| o.moved(subject)).unwrap_or_else(|| subject.clone())
    }

    /// A noted thing (`subject`, named `what`) where its owner says it is
    /// now, named as it's named there: none when neither changed.
    pub fn renamed(&self, subject: &Subject, what: &str) -> Option<(Subject, String)> {
        let now = self.current(subject);
        let named = self.name(&now).unwrap_or_else(|| what.to_owned());
        (now != *subject || named != what).then_some((now, named))
    }

    /// Where a noted thing is: still there (its owner says), gone with
    /// what it said kept in the logs, or gone.
    pub fn held(&self, subject: &Subject, kept: Option<&Kept>) -> Held {
        match (self.owner(subject).is_some_and(|o| o.there(subject)), kept) {
            (true, _) => Held::Source,
            (false, Some(_)) => Held::Kept,
            (false, None) => Held::Gone,
        }
    }

    /// What a noted thing says, whole, and where it is: what the logs
    /// kept of it when they did (written from its source when noted, and
    /// as it grew: all of it, where the source may since have lost some),
    /// else its source.
    pub fn said(&self, subject: &Subject, kept: Option<&Kept>) -> (Held, Option<String>) {
        let held = self.held(subject, kept);
        let text = match kept {
            Some(kept) => self.logs.read(kept).map_err(|e| tracing::warn!("A noted thing's log couldn't be read: {e}")).ok(),
            None => self.owner(subject).and_then(|o| o.now(subject)),
        };
        (given(held, text.is_some()), text)
    }

    /// The start of what a noted thing says (`most` characters, roughly),
    /// how many lines it has, and where it is; read through, never whole.
    pub fn glimpse(&self, subject: &Subject, kept: Option<&Kept>, most: usize) -> (Held, Option<(String, u64)>) {
        let held = self.held(subject, kept);
        let text = match kept {
            Some(kept) => self.logs.head(kept, most.saturating_mul(4)).map_err(|e| tracing::warn!("A noted thing's log couldn't be read: {e}")).ok(),
            None => self.owner(subject).and_then(|o| o.now(subject)).map(|t| {
                let lines = t.lines().count() as u64;
                (t, lines)
            }),
        };
        (given(held, text.is_some()), text.map(|(t, lines)| (some_of(t, most), lines)))
    }

    /// Lines `from..=to` (from 1) of what a noted thing says, and how many
    /// it has in all (none: nothing of it's here); only those read into
    /// memory.
    pub fn lines(&self, subject: &Subject, kept: Option<&Kept>, from: u64, to: u64) -> (Held, Option<Window>) {
        let held = self.held(subject, kept);
        let lines = match kept {
            Some(kept) => self.logs.lines(kept, from, to).map_err(|e| tracing::warn!("A noted thing's log couldn't be read: {e}")).ok(),
            None => self.owner(subject).and_then(|o| o.now(subject)).map(|text| {
                let all: Vec<&str> = text.lines().collect();
                let shown = all.iter().skip(from.saturating_sub(1) as usize).take((to + 1).saturating_sub(from) as usize).map(|l| (*l).to_owned()).collect();
                (shown, all.len() as u64)
            }),
        };
        (given(held, lines.is_some()), lines)
    }

    /// `text` (what `subject` says) with each run of its detail lines (a
    /// chat turn's tool output) said in one line: how many there are. Only
    /// a thing its owner says has detail is folded; every read folds the
    /// same.
    pub fn fold(&self, subject: &Subject, text: &str) -> String {
        if !self.owner(subject).is_some_and(|o| o.detailed(subject)) {
            return text.to_owned();
        }
        let mut out = Vec::new();
        let mut run = 0;
        for line in text.lines().chain(std::iter::once("")) {
            if line.starts_with(DETAIL) {
                run += 1;
                continue;
            }
            if run > 0 {
                out.push(format!("{DETAIL}({run} lines left out: tag_read's `full` gives them)"));
                run = 0;
            }
            out.push(line.to_owned());
        }
        out.pop();
        out.join("\n")
    }

    /// `subject` noted: settled by its owner, named, and (one its owner
    /// doesn't keep for good) written to the logs after a line saying what
    /// it is (`header`, given its name): growing, to a file its owner adds
    /// to until it ends or nothing points into it; else what it says now.
    /// Done before the note is kept (nothing else waits on it); a note not
    /// kept after all lets go of it (`let_go`).
    pub fn note(&self, subject: &Subject, header: &dyn Fn(&str) -> String) -> Result<Noting, String> {
        if let Some(why) = subject.unfit() {
            return Err(why);
        }
        let subject = self.current(subject);
        let owner = self.owner(&subject).ok_or("nothing here knows what it is")?;
        let subject = owner.settle(&subject)?;
        let what = owner.name(&subject);
        let kept = if owner.lasts(&subject) { None } else { Some(self.log(&*owner, &subject, &header(&what))?) };
        Ok(Noting { subject, what, kept })
    }

    fn log(&self, owner: &dyn Resolve, subject: &Subject, header: &str) -> Result<Kept, String> {
        let (kept, mut file) = self.logs.grow(header).map_err(|e| e.to_string())?;
        let on = Arc::new(AtomicBool::new(true));
        let still = on.clone();
        let (most, mut written) = (self.grown_most, 0u64);
        let more: More = Box::new(move |more| {
            if !still.load(Ordering::Relaxed) {
                return false;
            }
            // Past the most a growing thing keeps: said, and no longer followed.
            if written + more.len() as u64 > most {
                let size = if most % (1 << 20) == 0 { format!("{} MB", most >> 20) } else { format!("{most} bytes") };
                let _ = file.write_all(format!("\n(the rest isn't kept: past the {size} a running command's log keeps here)\n").as_bytes());
                return false;
            }
            written += more.len() as u64;
            file.write_all(more.as_bytes()).is_ok()
        });
        if owner.follow(subject, more) {
            lock(&self.growing).insert(kept.file.clone(), on);
            return Ok(kept);
        }
        // Not growing: what it says now, in a file of its own.
        self.logs.remove(&kept);
        let said = owner.now(subject).ok_or("it's gone")?;
        self.logs.write(header, &said).map_err(|e| e.to_string())
    }

    /// A note let go of a thing (it's gone, or wasn't kept): what the
    /// logs kept of it removed, and a growing thing no longer followed,
    /// once no note points into it; its owner told what's noted now.
    pub fn let_go(&self, kept: Option<&Kept>) {
        if let Some(kept) = kept {
            let used = self.notes().iter().map(|n| n.kept_files()).collect::<Result<Vec<_>, _>>();
            match used {
                Ok(used) if !used.iter().any(|files| files.contains(&kept.file)) => {
                    if let Some(on) = lock(&self.growing).remove(&kept.file) {
                        on.store(false, Ordering::Relaxed);
                    }
                    self.logs.remove(kept);
                }
                Ok(_) => {}
                Err(e) => tracing::warn!("A log was left: which are used couldn't be read: {e}"),
            }
        }
        self.tell_noted();
    }

    /// The lines of what `among` say that have `text` in them (capitals
    /// heeded only when it has some), the first `most`, in `among`'s
    /// order: what the logs keep searched with FFF, the rest where it is.
    pub fn find(&self, text: &str, among: &[(&Subject, Option<&Kept>)], most: usize) -> Vec<Found> {
        let mut found = self.find_kept(text, among, most);
        for (which, (subject, kept)) in among.iter().enumerate() {
            if kept.is_some() {
                continue;
            }
            let Some(said) = self.owner(subject).and_then(|o| o.now(subject)) else { continue };
            found.extend(said.lines().zip(1..).filter(|(l, _)| has(l, text)).take(most).map(|(l, line)| Found { which, line, text: l.to_owned() }));
        }
        found.sort_by_key(|f| (f.which, f.line));
        found.truncate(most);
        found
    }

    /// The lines of the logs `among` point into that have `text` in them,
    /// found with FFF (told first which files were written, removed or
    /// grew since).
    fn find_kept(&self, text: &str, among: &[(&Subject, Option<&Kept>)], most: usize) -> Vec<Found> {
        let mut files: BTreeMap<&str, Vec<(usize, &Kept)>> = BTreeMap::new();
        for (which, (_, kept)) in among.iter().enumerate() {
            if let Some(kept) = kept {
                files.entry(kept.file.as_str()).or_default().push((which, kept));
            }
        }
        if files.is_empty() || !self.logs.dir().is_dir() {
            return Vec::new();
        }
        let mut search = lock(&self.search);
        let mut changed: Vec<PathBuf> = self.logs.touched();
        changed.extend(lock(&self.growing).keys().map(|f| self.logs.dir().join(f)));
        if search.is_none() {
            match arch_search::RepoFiles::scan(self.logs.dir(), Box::new(|_| false)) {
                Ok(listed) => *search = Some(listed),
                Err(e) => {
                    tracing::warn!("The logs couldn't be listed to search: {e}");
                    return Vec::new();
                }
            }
        }
        let Some(listed) = search.as_mut() else { return Vec::new() };
        // Every file of ours listed, whatever ignores it (a global ignore of `*.log`).
        let ours: Vec<PathBuf> = std::fs::read_dir(self.logs.dir()).into_iter().flatten().flatten().filter(|e| logs::is_log(&e.file_name().to_string_lossy())).map(|e| e.path()).collect();
        if listed.len() != ours.len() {
            changed.extend(ours);
        }
        if let Err(e) = listed.changed(&changed) {
            tracing::warn!("The logs couldn't be listed again to search: {e}");
        }
        let limits = arch_search::TextLimits { per_file: most, budget_ms: SEARCH_BUDGET_MS, max_file_bytes: u64::MAX };
        let mut found = Vec::new();
        for line in listed.lines(limits).containing(text, usize::MAX >> 1).found {
            let Some(of) = files.get(line.path.as_str()) else { continue };
            for (which, kept) in of {
                let Ok((first, count)) = self.logs.span(kept) else { continue };
                let n = u64::from(line.line);
                if (first..first + count).contains(&n) {
                    found.push(Found { which: *which, line: n + 1 - first, text: line.text.clone() });
                }
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::TerminalId;

    use super::*;
    use crate::testing::Lines;

    fn lines(first: u64, last: u64) -> Subject {
        Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first, last, command: None }
    }

    /// Notes kept in memory: the files they point into, and how often
    /// they were told of moves.
    #[derive(Default)]
    struct Pointing(Mutex<BTreeSet<String>>, std::sync::atomic::AtomicUsize);

    impl Notes for Pointing {
        fn kept_files(&self) -> Result<BTreeSet<String>, String> {
            Ok(lock(&self.0).clone())
        }
        fn subjects(&self) -> Vec<Subject> {
            Vec::new()
        }
        fn refresh(&self) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fixture {
        things: Arc<Things>,
        owner: Arc<Lines>,
        notes: Arc<Pointing>,
        dir: tempfile::TempDir,
    }

    fn things(most: u64) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let things = Things::open(&dir.path().join("tagged"), most);
        let notes = Arc::new(Pointing::default());
        let weak: Weak<dyn Notes> = Arc::downgrade(&notes) as Weak<dyn Notes>;
        things.noting(weak);
        let owner = Arc::new(Lines::default());
        things.register(owner.clone());
        Fixture { things, owner, notes, dir }
    }

    fn header(what: &str) -> String {
        format!("x · {what}")
    }

    #[test]
    fn a_thing_its_owner_doesnt_keep_is_written_when_noted_and_comes_back_whole_once_its_source_went() {
        let Fixture { things, owner, notes, .. } = things(GROWN_MOST);
        assert_eq!(notes.1.load(Ordering::SeqCst), 1, "an owner registered: the notes are told");
        let noted = things.note(&lines(10, 12), &header).unwrap();
        assert_eq!(noted.what, "lines 10 to 12");
        let kept = noted.kept.clone().unwrap();
        assert_eq!(things.said(&noted.subject, Some(&kept)), (Held::Source, Some("text of lines 10 to 12\n".into())));
        owner.scroll_away();
        assert_eq!(things.said(&noted.subject, Some(&kept)), (Held::Kept, Some("text of lines 10 to 12\n".into())));
        assert_eq!(things.glimpse(&noted.subject, Some(&kept), 4), (Held::Kept, Some(("text…".into(), 1))));
        assert_eq!(things.lines(&noted.subject, None, 1, 1), (Held::Gone, None), "nothing kept, and gone");
        assert!(things.note(&lines(0, 0), &header).is_err(), "a thing that's gone can't be noted");
        assert!(things.note(&lines(1, 300), &header).is_err(), "nor too many lines");
    }

    #[test]
    fn a_growing_thing_is_followed_to_the_projects_cap_and_given_back_once_no_note_points_into_it() {
        let Fixture { things, owner, notes, dir } = things(40);
        let noted = things.note(&lines(500, 501), &header).unwrap();
        let kept = noted.kept.clone().unwrap();
        lock(&notes.0).insert(kept.file.clone());
        owner.grow("more\n");
        assert_eq!(things.said(&noted.subject, Some(&kept)).1.as_deref(), Some("text of lines 500 to 501\nmore\n"));
        owner.grow("past the cap\n");
        assert_eq!(owner.followers(), 0, "no longer followed");
        assert!(things.said(&noted.subject, Some(&kept)).1.unwrap().ends_with("more\n\n(the rest isn't kept: past the 40 bytes a running command's log keeps here)\n"), "said so");
        // Another note points into it: kept; none does: removed.
        let path = dir.path().join("tagged").join(&kept.file);
        things.let_go(Some(&kept));
        assert!(path.exists());
        lock(&notes.0).clear();
        things.let_go(Some(&kept));
        assert!(!path.exists());
    }

    #[test]
    fn what_notes_are_on_is_searched_with_fff_where_its_kept_and_where_it_is_otherwise() {
        let Fixture { things, owner, notes, .. } = things(GROWN_MOST);
        let a = things.note(&lines(10, 12), &header).unwrap();
        let b = things.note(&lines(500, 501), &header).unwrap();
        for n in [&a, &b] {
            lock(&notes.0).insert(n.kept.clone().unwrap().file);
        }
        owner.grow("error: 1\nok\nerror: 2\n");
        let among = [(&a.subject, a.kept.as_ref()), (&b.subject, b.kept.as_ref())];
        let found = things.find("error", &among, 10);
        assert_eq!(found, [Found { which: 1, line: 2, text: "error: 1".into() }, Found { which: 1, line: 4, text: "error: 2".into() }], "lines of the thing, not of its file");
        assert!(things.find("ERROR", &among, 10).is_empty(), "capitals heeded");
        assert_eq!(things.find("text of", &among, 1).len(), 1, "the first `most`");
        owner.grow("error: 3\n");
        assert_eq!(things.find("error: 3", &among, 10).len(), 1, "what it grew by since");
        // A thing its owner keeps: searched where it is.
        let lasting = Subject::TerminalLines { terminal: TerminalId::new("t2"), shell: 1, first: 1, last: 2, command: None };
        assert_eq!(things.find("lines 1 to", &[(&lasting, None)], 10), [Found { which: 0, line: 1, text: "text of lines 1 to 2".into() }]);
    }

    #[test]
    fn logs_no_note_points_into_are_removed_at_start_up_but_not_when_a_kind_of_note_cant_say() {
        let Fixture { things, notes, dir, .. } = things(GROWN_MOST);
        let a = things.note(&lines(10, 12), &header).unwrap().kept.unwrap();
        let b = things.note(&lines(20, 22), &header).unwrap().kept.unwrap();
        lock(&notes.0).insert(b.file.clone());
        things.forget_unused();
        let folder = dir.path().join("tagged");
        assert!(!folder.join(&a.file).exists() && folder.join(&b.file).exists());
        // A kind of note that can't say: nothing goes.
        struct Unread;
        impl Notes for Unread {
            fn kept_files(&self) -> Result<BTreeSet<String>, String> {
                Err("a newer build's".into())
            }
            fn subjects(&self) -> Vec<Subject> {
                Vec::new()
            }
            fn refresh(&self) {}
        }
        let unread: Arc<dyn Notes> = Arc::new(Unread);
        things.noting(Arc::downgrade(&unread));
        lock(&notes.0).clear();
        things.forget_unused();
        assert!(folder.join(&b.file).exists());
    }

    #[test]
    fn only_a_thing_its_owner_says_has_detail_is_folded() {
        let Fixture { things, .. } = things(GROWN_MOST);
        let text = format!("you: hi\n• Bash\n{DETAIL}a\n{DETAIL}b\ndone");
        assert_eq!(things.fold(&lines(1, 1), &text), text, "a terminal's lines never");
        let detailed = Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 7, last: 7, command: None };
        assert_eq!(things.fold(&detailed, &text), format!("you: hi\n• Bash\n{DETAIL}(2 lines left out: tag_read's `full` gives them)\ndone"));
    }

    #[test]
    fn some_of_a_long_text_is_cut_with_a_mark() {
        assert_eq!(some_of("é".repeat(10), 4).chars().count(), 5, "cut, with …");
        assert_eq!(some_of("short".into(), 40), "short");
    }

    /// Things know no owner and no kind of note (plan rule 2).
    #[test]
    fn it_depends_on_no_owner_and_no_kind_of_note() {
        let manifest = include_str!("../Cargo.toml");
        let local: Vec<&str> = manifest.lines().filter(|l| l.contains("path = ")).map(|l| l.split('=').next().unwrap().trim()).collect();
        assert_eq!(local, ["arch-search", "harness-contracts", "nah-things"], "searching, the shared types, and itself (its tests' testing feature)");
    }
}
