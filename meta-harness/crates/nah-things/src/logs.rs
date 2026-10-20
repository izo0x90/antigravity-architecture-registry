//! Logs: text kept in files of its own, one file per piece (`<prefix>-1.log`,
//! `<prefix>-2.log`...), in one folder readable by the server's user only
//! (as the state database is), read back in parts and searched with FFF.
//! The things layer keeps what a noted thing says in them (for a thing its
//! owner doesn't keep for good: a terminal's output, a chat's message);
//! another kind of note can keep its own text in a folder of its own, the
//! same way. A piece whole when written is written at once (its line count
//! kept with where it is); one still growing (a command running) is written
//! to as it grows and read to its end. A file's first line says what it
//! holds, for whoever reads or searches the files by hand; only that line
//! is a heading, so nothing a piece says can pass for one. Whoever keeps a
//! piece keeps where it is (`Kept`); a file nothing points into is removed.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::Kept;

/// How long a search through the logs may take before it answers with
/// what it found (said to be partial).
const SEARCH_BUDGET_MS: u64 = 2000;

pub struct Logs {
    dir: PathBuf,
    prefix: String,
    state: Mutex<State>,
    /// The files as FFF lists them (once there are some).
    search: Mutex<Option<arch_search::RepoFiles>>,
}

/// The next file's number; how many lines of each growing piece were
/// counted (and up to where: it's counted on from there); the files written
/// or removed since the search last heard, and the growing ones' sizes as
/// it last saw them.
struct State {
    next: u64,
    counted: HashMap<(String, u64), Counted>,
    touched: BTreeSet<PathBuf>,
    growing: BTreeMap<String, u64>,
}

#[derive(Clone, Copy)]
struct Counted {
    /// Bytes read, lines in them, and whether the last ended one.
    read: u64,
    lines: u64,
    ended: bool,
}

/// A line found in the logs: its file, which of the file's lines (from 1),
/// the line.
pub type FoundLine = (String, u64, String);

/// Whether `line` has every word of `query` in it, in any order (capitals
/// heeded only when `query` has some): how every search of notes matches.
pub fn matches(line: &str, query: &str) -> bool {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    if query.chars().any(char::is_uppercase) {
        words.iter().all(|w| line.contains(w))
    } else {
        let line = line.to_lowercase();
        words.iter().all(|w| line.contains(&w.to_lowercase()))
    }
}

impl Logs {
    /// The logs in `dir`, their files named `<prefix>-<n>.log` (the folder
    /// made when first written to).
    pub fn open(dir: &Path, prefix: &str) -> Self {
        // A folder made before they were the server user's only, made so
        // (a link is never followed: nothing is written through one).
        #[cfg(unix)]
        if std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
                tracing::warn!("The logs ({}) couldn't be made the server user's only: {e}", dir.display());
            }
        }
        let logs = Self { dir: dir.to_owned(), prefix: prefix.to_owned(), state: Mutex::new(State { next: 1, counted: HashMap::new(), touched: BTreeSet::new(), growing: BTreeMap::new() }), search: Mutex::default() };
        let last = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| logs.number(&e.ok()?.file_name().to_string_lossy())).max();
        logs.lock().next = last.map_or(1, |n| n + 1);
        logs
    }

    fn name(&self, n: u64) -> String {
        format!("{}-{n}.log", self.prefix)
    }

    fn number(&self, file: &str) -> Option<u64> {
        file.strip_prefix(self.prefix.as_str())?.strip_prefix('-')?.strip_suffix(".log")?.parse().ok()
    }

    /// A file of these logs.
    pub fn is_log(&self, file: &str) -> bool {
        self.number(file).is_some()
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the folder is a link (made to point elsewhere): the logs are
    /// neither written nor removed through it.
    fn linked(&self) -> bool {
        std::fs::symlink_metadata(&self.dir).is_ok_and(|m| m.file_type().is_symlink())
    }

    /// A new file, its first line saying what it holds (made, the server's
    /// user's only): where its piece starts, and the file to write it to.
    fn start(&self, header: &str) -> std::io::Result<(Kept, File)> {
        if self.linked() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is a link, not a folder: nothing is written through it", self.dir.display())));
        }
        let mut folder = std::fs::DirBuilder::new();
        folder.recursive(true);
        let mut opened = std::fs::OpenOptions::new();
        opened.create_new(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            folder.mode(0o700);
            opened.mode(0o600);
        }
        folder.create(&self.dir)?;
        let (path, mut file) = loop {
            let n = {
                let mut state = self.lock();
                state.next += 1;
                state.next - 1
            };
            let path = self.dir.join(self.name(n));
            match opened.open(&path) {
                Ok(file) => break (path, file),
                // One a crash left behind, or another server's: the next.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let heading = format!("== {}\n", header.replace(['\n', '\r'], " "));
        file.write_all(heading.as_bytes())?;
        self.lock().touched.insert(path.clone());
        Ok((Kept { file: name_of(&path), start: heading.len() as u64, len: None, lines: None }, file))
    }

    /// Writes `text` after a line saying what it is (`header`), in a file
    /// of its own: where it is.
    pub fn write(&self, header: &str, text: &str) -> std::io::Result<Kept> {
        let (mut kept, mut file) = self.start(header)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        kept.len = Some(text.len() as u64);
        kept.lines = Some(text.lines().count() as u64);
        Ok(kept)
    }

    /// A file for a piece that's growing, after a line saying what it is:
    /// where it is (to the file's end), and the file to add to.
    pub fn grow(&self, header: &str) -> std::io::Result<(Kept, File)> {
        let (kept, file) = self.start(header)?;
        self.lock().growing.insert(kept.file.clone(), 0);
        Ok((kept, file))
    }

    fn opened(&self, kept: &Kept) -> std::io::Result<impl Read> {
        let mut file = File::open(self.dir.join(&kept.file))?;
        file.seek(SeekFrom::Start(kept.start))?;
        Ok(file.take(kept.len.unwrap_or(u64::MAX)))
    }

    /// The text `kept` points at, whole.
    pub fn read(&self, kept: &Kept) -> std::io::Result<String> {
        let mut out = Vec::new();
        self.opened(kept)?.read_to_end(&mut out)?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// How many lines the text `kept` points at has: as kept with it when
    /// it was whole; a growing piece's counted once, then only what it grew
    /// by since.
    pub fn count(&self, kept: &Kept) -> std::io::Result<u64> {
        if let Some(lines) = kept.lines {
            return Ok(lines);
        }
        let key = (kept.file.clone(), kept.start);
        let was = self.lock().counted.get(&key).copied().unwrap_or(Counted { read: 0, lines: 0, ended: true });
        let mut file = File::open(self.dir.join(&kept.file))?;
        file.seek(SeekFrom::Start(kept.start + was.read))?;
        let left = kept.len.map_or(u64::MAX, |len| len.saturating_sub(was.read));
        let mut reader = BufReader::new(file.take(left));
        let mut now = was;
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            now.lines += buf.iter().filter(|b| **b == b'\n').count() as u64;
            now.ended = buf[buf.len() - 1] == b'\n';
            now.read += buf.len() as u64;
            let n = buf.len();
            reader.consume(n);
        }
        self.lock().counted.insert(key, now);
        Ok(now.lines + u64::from(!now.ended))
    }

    /// The start of the text `kept` points at (`most` bytes, cut where a
    /// character ends), and how many lines it has in all (`count`).
    pub fn head(&self, kept: &Kept, most: usize) -> std::io::Result<(String, u64)> {
        let mut head = Vec::new();
        self.opened(kept)?.take(most as u64).read_to_end(&mut head)?;
        let cut = match std::str::from_utf8(&head) {
            Ok(_) => head.len(),
            Err(e) => e.valid_up_to(),
        };
        Ok((String::from_utf8_lossy(&head[..cut]).into_owned(), self.count(kept)?))
    }

    /// Lines `from..=to` (from 1) of the text `kept` points at, and how
    /// many it has in all; read only up to the last asked for.
    pub fn lines(&self, kept: &Kept, from: u64, to: u64) -> std::io::Result<(Vec<String>, u64)> {
        let mut reader = BufReader::new(self.opened(kept)?);
        let (mut out, mut line, mut n) = (Vec::new(), Vec::new(), 0u64);
        while n < to && reader.read_until(b'\n', &mut line)? > 0 {
            n += 1;
            if n >= from {
                out.push(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']).to_owned());
            }
            line.clear();
        }
        Ok((out, self.count(kept)?))
    }

    /// Which line of its file the text `kept` points at starts on (from
    /// 1), and how many lines it has: a line of the file found by a search
    /// is the piece's when it's among them.
    pub fn span(&self, kept: &Kept) -> std::io::Result<(u64, u64)> {
        let mut before = Vec::new();
        File::open(self.dir.join(&kept.file))?.take(kept.start).read_to_end(&mut before)?;
        Ok((before.iter().filter(|b| **b == b'\n').count() as u64 + 1, self.count(kept)?))
    }

    /// Removes file `file` (nothing points into it any more).
    pub fn remove(&self, file: &str) {
        if self.linked() || !self.is_log(file) {
            return;
        }
        let path = self.dir.join(file);
        if std::fs::remove_file(&path).is_ok() {
            let mut state = self.lock();
            state.counted.retain(|(f, _), _| f != file);
            state.growing.remove(file);
            state.touched.insert(path);
        }
    }

    /// Removes the files of these logs nothing points into (`used`: the
    /// files that are): the ones removed.
    pub fn forget_all_but(&self, used: &BTreeSet<String>) -> Vec<String> {
        let mut gone = Vec::new();
        if self.linked() {
            return gone;
        }
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if self.is_log(&file) && !used.contains(&file) {
                self.remove(&file);
                gone.push(file);
            }
        }
        gone
    }

    /// The lines of files `files` that have every word of `query` in them
    /// (`matches`), at most `per_file` of each piece's own: found with FFF
    /// (told first which files were written, removed or grew since), and
    /// whether the search ran out of time before it was through.
    pub fn search(&self, query: &str, files: &BTreeSet<String>, per_file: usize) -> (Vec<FoundLine>, bool) {
        // The longest word finds the lines; each must have every word.
        let Some(word) = query.split_whitespace().max_by_key(|w| w.len()) else { return (Vec::new(), false) };
        if files.is_empty() || !self.dir.is_dir() {
            return (Vec::new(), false);
        }
        let mut search = self.search.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if search.is_none() {
            match arch_search::RepoFiles::scan(&self.dir, Box::new(|_| false)) {
                Ok(listed) => *search = Some(listed),
                Err(e) => {
                    tracing::warn!("The logs couldn't be listed to search: {e}");
                    return (Vec::new(), false);
                }
            }
        }
        let Some(listed) = search.as_mut() else { return (Vec::new(), false) };
        let mut changed: Vec<PathBuf> = {
            let mut state = self.lock();
            let mut changed: Vec<PathBuf> = std::mem::take(&mut state.touched).into_iter().collect();
            // A growing file, when it grew since.
            for (file, seen) in state.growing.iter_mut() {
                let size = std::fs::metadata(self.dir.join(file)).map_or(0, |m| m.len());
                if size != *seen {
                    *seen = size;
                    changed.push(self.dir.join(file));
                }
            }
            changed
        };
        // Every file of ours listed, whatever ignores it (a global ignore of `*.log`).
        if listed.len() < files.len() {
            changed.extend(std::fs::read_dir(&self.dir).into_iter().flatten().flatten().filter(|e| self.is_log(&e.file_name().to_string_lossy())).map(|e| e.path()));
        }
        if let Err(e) = listed.changed(&changed) {
            tracing::warn!("The logs couldn't be listed again to search: {e}");
        }
        // One more a file: its heading, never a piece's.
        let limits = arch_search::TextLimits { per_file: per_file.saturating_add(1), budget_ms: SEARCH_BUDGET_MS, max_file_bytes: u64::MAX };
        let searched = listed.lines(limits).containing(word, usize::MAX >> 1);
        let mut each: BTreeMap<String, usize> = BTreeMap::new();
        let found = searched
            .found
            .into_iter()
            .filter(|l| files.contains(&l.path) && l.line > 1 && matches(&l.text, query))
            .filter(|l| {
                let n = each.entry(l.path.clone()).or_default();
                *n += 1;
                *n <= per_file
            })
            .map(|l| (l.path, u64::from(l.line), l.text))
            .collect();
        (found, searched.partial)
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_written_each_in_a_file_of_its_own_after_what_they_are_and_read_back_whole_or_their_start() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path(), "tagged");
        let a = logs.write("flaky · terminal t1 command 3 · tagged by user", "$ make\nerror: 1\n").unwrap();
        let b = logs.write("flaky · terminal t1 line 4", "and more").unwrap();
        assert_ne!(a.file, b.file, "each its own: taken off, nothing else of it stays");
        assert_eq!((logs.read(&a).unwrap().as_str(), logs.read(&b).unwrap().as_str()), ("$ make\nerror: 1\n", "and more"));
        assert_eq!((a.lines, b.lines), (Some(2), Some(1)), "counted as it's written");
        assert_eq!(logs.head(&a, 3).unwrap(), ("$ m".to_owned(), 2));
        assert_eq!(logs.lines(&a, 2, 9).unwrap(), (vec!["error: 1".to_owned()], 2));
        assert_eq!(logs.span(&a).unwrap(), (2, 2), "its first line is the file's second");
        let file = std::fs::read_to_string(dir.path().join(&a.file)).unwrap();
        assert_eq!(file, "== flaky · terminal t1 command 3 · tagged by user\n$ make\nerror: 1\n");
        // What a piece says can't pass for a heading: only a file's first line is one.
        let posing = logs.write("real", "== fake · something else\nmore\n").unwrap();
        assert_eq!((logs.read(&posing).unwrap().as_str(), logs.span(&posing).unwrap()), ("== fake · something else\nmore\n", (2, 2)));
        // A growing piece, read to its end as it grows (and counted on).
        let (growing, mut more) = logs.grow("slow · terminal t1 command 4").unwrap();
        more.write_all(b"1\n").unwrap();
        assert_eq!(logs.count(&growing).unwrap(), 1);
        more.write_all(b"2\n3").unwrap();
        assert_eq!((logs.read(&growing).unwrap().as_str(), logs.count(&growing).unwrap()), ("1\n2\n3", 3));
        // Only the server's user reads them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.path().join(&a.file)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A file nothing points into goes; another prefix's are another's.
        std::fs::write(dir.path().join("said-1.log"), "== x\ny").unwrap();
        let mut gone = logs.forget_all_but(&BTreeSet::from([growing.file.clone(), b.file.clone()]));
        gone.sort();
        assert_eq!(gone, [a.file.clone(), posing.file.clone()]);
        assert!(dir.path().join("said-1.log").exists());
        logs.remove(&b.file);
        assert!(!dir.path().join(&b.file).exists());
        // A folder made before, readable by others: made the server user's only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            Logs::open(dir.path(), "tagged");
            assert_eq!(std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777, 0o700);
            // A link: never written through, nor removed from.
            let elsewhere = tempfile::tempdir().unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(elsewhere.path(), &link).unwrap();
            let linked = Logs::open(&link, "tagged");
            assert!(linked.write("x", "y").is_err());
            std::fs::write(elsewhere.path().join("tagged-1.log"), "kept").unwrap();
            assert!(linked.forget_all_but(&BTreeSet::new()).is_empty() && elsewhere.path().join("tagged-1.log").exists());
            std::fs::remove_file(&link).unwrap();
        }
        // Opened again: a file from before is never written to again.
        assert!(![a.file.clone(), b.file.clone(), growing.file.clone()].contains(&Logs::open(dir.path(), "tagged").write("x", "z").unwrap().file));
    }

    #[test]
    fn a_search_finds_lines_with_every_word_in_any_order_never_a_heading() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path(), "said");
        let a = logs.write("build failed · said by user", "the build is flaky\nflaky, the build\nonly flaky\n").unwrap();
        let b = logs.write("other", "build flaky too\n").unwrap();
        let only_a = BTreeSet::from([a.file.clone()]);
        let (found, partial) = logs.search("flaky build", &only_a, 10);
        assert_eq!((found.iter().map(|(_, n, t)| (*n, t.as_str())).collect::<Vec<_>>(), partial), (vec![(2, "the build is flaky"), (3, "flaky, the build")], false), "every word, any order; only the files asked; the heading is no piece's");
        assert!(logs.search("BUILD", &only_a, 10).0.is_empty(), "capitals heeded when there are some");
        assert_eq!(logs.search("flaky", &only_a, 1).0.len(), 1, "as many of a piece's as asked");
        let mut both = only_a.clone();
        both.insert(b.file.clone());
        assert_eq!(logs.search("flaky too", &both, 10).0.len(), 1);
        assert!(matches("  flaky build  ", " build   flaky "), "spaces don't count");
    }
}
