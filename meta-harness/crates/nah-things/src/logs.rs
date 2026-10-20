//! The logs: what a noted thing says, kept for a thing its owner doesn't
//! keep for good (a terminal's output, a chat's message). One file per
//! thing (`tagged-1.log`, `tagged-2.log`...), in one folder readable by the
//! server's user only (as the state database is): a thing that's whole
//! when noted is written at once; one still growing (a command running) is
//! written to as it grows and read to its end. The file's first line says
//! what it is (`== <tag> · <what> · tagged by <who> · <when>`), for whoever
//! reads or searches the files by hand; only that line is a heading, so
//! nothing a thing says can pass for one. A note keeps where its thing is
//! (`Kept`), never its text; a file no note points into is removed (and
//! its thing, if growing, isn't written to any more).

use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::Kept;

pub struct Logs {
    dir: PathBuf,
    state: Mutex<State>,
}

/// The next file's number, how many lines of each thing were counted
/// (and up to where: a growing one is counted on from there), and the
/// files written or removed since the search last heard.
struct State {
    next: u64,
    counted: HashMap<Kept, Counted>,
    touched: BTreeSet<PathBuf>,
}

#[derive(Clone, Copy)]
struct Counted {
    /// Bytes read, lines in them, and whether the last ended one.
    read: u64,
    lines: u64,
    ended: bool,
}

fn name(n: u64) -> String {
    format!("tagged-{n}.log")
}

fn number(file: &str) -> Option<u64> {
    file.strip_prefix("tagged-")?.strip_suffix(".log")?.parse().ok()
}

/// A file of the logs (one of ours: `tagged-<n>.log`).
pub fn is_log(file: &str) -> bool {
    number(file).is_some()
}

impl Logs {
    /// The logs in `dir` (made when first written to).
    pub fn open(dir: &Path) -> Self {
        // A folder made before they were the server user's only, made so
        // (a link is never followed: nothing is written through one).
        #[cfg(unix)]
        if std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
                tracing::warn!("The logs ({}) couldn't be made the server user's only: {e}", dir.display());
            }
        }
        let last = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| number(&e.ok()?.file_name().to_string_lossy())).max();
        Self { dir: dir.to_owned(), state: Mutex::new(State { next: last.map_or(1, |n| n + 1), counted: HashMap::new(), touched: BTreeSet::new() }) }
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
    /// user's only): where its thing starts, and the file to write it to.
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
            let path = self.dir.join(name(n));
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
        Ok((Kept { file: name_of(&path), start: heading.len() as u64, len: None }, file))
    }

    /// Writes `text` after a line saying what it is (`header`), in a file
    /// of its own: where it is.
    pub fn write(&self, header: &str, text: &str) -> std::io::Result<Kept> {
        let (mut kept, mut file) = self.start(header)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        kept.len = Some(text.len() as u64);
        Ok(kept)
    }

    /// A file for a thing that's growing, after a line saying what it is:
    /// where it is (to the file's end), and the file to add to.
    pub fn grow(&self, header: &str) -> std::io::Result<(Kept, File)> {
        self.start(header)
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

    /// How many lines the text `kept` points at has: counted once, then
    /// only what it grew by since.
    pub fn count(&self, kept: &Kept) -> std::io::Result<u64> {
        let was = self.lock().counted.get(kept).copied().unwrap_or(Counted { read: 0, lines: 0, ended: true });
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
        self.lock().counted.insert(kept.clone(), now);
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
    /// is the thing's when it's among them.
    pub fn span(&self, kept: &Kept) -> std::io::Result<(u64, u64)> {
        let mut before = Vec::new();
        File::open(self.dir.join(&kept.file))?.take(kept.start).read_to_end(&mut before)?;
        Ok((before.iter().filter(|b| **b == b'\n').count() as u64 + 1, self.count(kept)?))
    }

    /// Removes the file `kept` is in (none left pointing into it).
    pub fn remove(&self, kept: &Kept) {
        if self.linked() {
            return;
        }
        let path = self.dir.join(&kept.file);
        if std::fs::remove_file(&path).is_ok() {
            let mut state = self.lock();
            state.counted.retain(|k, _| k.file != kept.file);
            state.touched.insert(path);
        }
    }

    /// Removes the files of ours nothing points into (`used`: the files
    /// that are): the ones removed.
    pub fn forget_all_but(&self, used: &BTreeSet<String>) -> Vec<String> {
        let mut gone = Vec::new();
        if self.linked() {
            return gone;
        }
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if is_log(&file) && !used.contains(&file) && std::fs::remove_file(entry.path()).is_ok() {
                self.lock().touched.insert(entry.path());
                gone.push(file);
            }
        }
        gone
    }

    /// The files written or removed since this was last asked.
    pub fn touched(&self) -> Vec<PathBuf> {
        std::mem::take(&mut self.lock().touched).into_iter().collect()
    }
}

fn name_of(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn things_are_written_each_in_a_file_of_its_own_after_what_they_are_and_read_back_whole_or_their_start() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path());
        let a = logs.write("flaky · terminal t1 command 3 · tagged by user", "$ make\nerror: 1\n").unwrap();
        let b = logs.write("flaky · terminal t1 line 4", "and more").unwrap();
        assert_ne!(a.file, b.file, "each its own: taken off, nothing else of it stays");
        assert_eq!((logs.read(&a).unwrap().as_str(), logs.read(&b).unwrap().as_str()), ("$ make\nerror: 1\n", "and more"));
        assert_eq!(logs.head(&a, 3).unwrap(), ("$ m".to_owned(), 2));
        assert_eq!(logs.head(&b, 100).unwrap(), ("and more".to_owned(), 1));
        assert_eq!(logs.lines(&a, 2, 9).unwrap(), (vec!["error: 1".to_owned()], 2));
        assert_eq!(logs.span(&a).unwrap(), (2, 2), "its first line is the file's second");
        let file = std::fs::read_to_string(dir.path().join(&a.file)).unwrap();
        assert_eq!(file, "== flaky · terminal t1 command 3 · tagged by user\n$ make\nerror: 1\n");
        // What a thing says can't pass for a heading: only a file's first line is one.
        let posing = logs.write("real", "== fake · something else\nmore\n").unwrap();
        assert_eq!((logs.read(&posing).unwrap().as_str(), logs.span(&posing).unwrap()), ("== fake · something else\nmore\n", (2, 2)));
        // A growing thing, read to its end as it grows (and counted on).
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
        // A file nothing points into goes.
        let mut gone = logs.forget_all_but(&BTreeSet::from([growing.file.clone(), b.file.clone()]));
        gone.sort();
        assert_eq!(gone, [a.file.clone(), posing.file.clone()]);
        logs.remove(&b);
        assert!(!dir.path().join(&b.file).exists());
        // A folder made before, readable by others: made the server user's only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
            Logs::open(dir.path());
            assert_eq!(std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777, 0o700);
            // A link: never written through, nor removed from.
            let elsewhere = tempfile::tempdir().unwrap();
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(elsewhere.path(), &link).unwrap();
            let linked = Logs::open(&link);
            assert!(linked.write("x", "y").is_err());
            std::fs::write(elsewhere.path().join("tagged-1.log"), "kept").unwrap();
            assert!(linked.forget_all_but(&BTreeSet::new()).is_empty() && elsewhere.path().join("tagged-1.log").exists());
            std::fs::remove_file(&link).unwrap();
        }
        // Opened again: a file from before is never written to again.
        assert!(![a.file.clone(), b.file.clone(), growing.file.clone()].contains(&Logs::open(dir.path()).write("x", "z").unwrap().file));
    }
}
