//! The tags' own logs: what a tagged thing says, kept for a thing its owner
//! doesn't keep for good (a terminal's output, a chat's message). Files in
//! one folder (`tagged-1.log`, `tagged-2.log`...), readable by the server's
//! user only (as the state database is). A thing that's whole when tagged
//! is appended to the file written to now (a new one once it passes
//! `FILE_MOST`); a thing still growing (a command running) gets a file of
//! its own, written to as it grows and read to its end. Each thing comes
//! after a line saying what it is (`== <tag> · <what> · tagged by <who> ·
//! <when>`), for whoever reads or searches the files by hand. A tag keeps
//! where its thing is (`Kept`), never its text; a file no tag points into
//! is removed (and its thing, if growing, isn't written to any more).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::Kept;

/// How big a log file grows before the next one is started.
const FILE_MOST: u64 = 8 << 20;

pub struct Logs {
    dir: PathBuf,
    files: Mutex<Files>,
}

/// The file whole things are appended to now (none until the first, so a
/// file from before is never written to again), and the next file's number.
struct Files {
    shared: Option<u64>,
    next: u64,
}

fn name(n: u64) -> String {
    format!("tagged-{n}.log")
}

fn number(file: &str) -> Option<u64> {
    file.strip_prefix("tagged-")?.strip_suffix(".log")?.parse().ok()
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
                tracing::warn!("The tags' logs ({}) couldn't be made the server user's only: {e}", dir.display());
            }
        }
        let last = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| number(&e.ok()?.file_name().to_string_lossy())).max();
        Self { dir: dir.to_owned(), files: Mutex::new(Files { shared: None, next: last.map_or(1, |n| n + 1) }) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Files> {
        self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether the folder is a link (made to point elsewhere): the logs are
    /// neither written nor removed through it.
    fn linked(&self) -> bool {
        std::fs::symlink_metadata(&self.dir).is_ok_and(|m| m.file_type().is_symlink())
    }

    /// A file of the logs, opened to add to (made, the server's user's only).
    fn append(&self, n: u64) -> std::io::Result<File> {
        if self.linked() {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, format!("{} is a link, not a folder: nothing is written through it", self.dir.display())));
        }
        let mut folder = std::fs::DirBuilder::new();
        folder.recursive(true);
        let mut file = std::fs::OpenOptions::new();
        file.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            folder.mode(0o700);
            file.mode(0o600);
        }
        folder.create(&self.dir)?;
        file.open(self.dir.join(name(n)))
    }

    /// Writes `text` after a line saying what it is (`header`): where it is.
    pub fn write(&self, header: &str, text: &str) -> std::io::Result<Kept> {
        let mut files = self.lock();
        let size = |n: u64| std::fs::metadata(self.dir.join(name(n))).map_or(0, |m| m.len());
        let n = match files.shared {
            Some(n) if size(n) <= FILE_MOST => n,
            _ => {
                files.next += 1;
                files.next - 1
            }
        };
        files.shared = Some(n);
        let mut file = self.append(n)?;
        file.write_all(heading(header).as_bytes())?;
        let start = file.stream_position()?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        Ok(Kept { file: name(n), start, len: Some(text.len() as u64) })
    }

    /// A file of its own for a thing that's growing, after a line saying
    /// what it is: where it is (to the file's end), and the file to add to.
    pub fn grow(&self, header: &str) -> std::io::Result<(Kept, File)> {
        let n = {
            let mut files = self.lock();
            files.next += 1;
            files.next - 1
        };
        let mut file = self.append(n)?;
        file.write_all(heading(header).as_bytes())?;
        let start = file.stream_position()?;
        Ok((Kept { file: name(n), start, len: None }, file))
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

    /// The start of the text `kept` points at (`most` bytes, cut where a
    /// character ends), and how many lines it has in all (read through,
    /// never held).
    pub fn head(&self, kept: &Kept, most: usize) -> std::io::Result<(String, u64)> {
        let mut reader = BufReader::new(self.opened(kept)?);
        let (mut head, mut lines, mut last) = (Vec::new(), 0u64, b'\n');
        loop {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            if head.len() < most {
                head.extend_from_slice(&buf[..buf.len().min(most - head.len())]);
            }
            lines += buf.iter().filter(|b| **b == b'\n').count() as u64;
            last = buf[buf.len() - 1];
            let n = buf.len();
            reader.consume(n);
        }
        let lines = lines + u64::from(last != b'\n');
        let cut = match std::str::from_utf8(&head) {
            Ok(_) => head.len(),
            Err(e) => e.valid_up_to(),
        };
        Ok((String::from_utf8_lossy(&head[..cut]).into_owned(), lines))
    }

    /// The lines of the text `kept` points at that `matches` (1-based,
    /// in order), the first `most`; read through, never held.
    pub fn find(&self, kept: &Kept, matches: &dyn Fn(&str) -> bool, most: usize) -> std::io::Result<Vec<(u64, String)>> {
        let mut reader = BufReader::new(self.opened(kept)?);
        let (mut found, mut line, mut n) = (Vec::new(), Vec::new(), 0u64);
        while found.len() < most && reader.read_until(b'\n', &mut line)? > 0 {
            n += 1;
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\n', '\r']);
            if matches(text) {
                found.push((n, text.to_owned()));
            }
            line.clear();
        }
        Ok(found)
    }

    /// Lines `from..=to` (from 1) of the text `kept` points at, and how
    /// many it has in all; read through, only those held.
    pub fn lines(&self, kept: &Kept, from: u64, to: u64) -> std::io::Result<(Vec<String>, u64)> {
        let mut reader = BufReader::new(self.opened(kept)?);
        let (mut out, mut line, mut n) = (Vec::new(), Vec::new(), 0u64);
        while reader.read_until(b'\n', &mut line)? > 0 {
            n += 1;
            if (from..=to).contains(&n) {
                out.push(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']).to_owned());
            }
            line.clear();
        }
        Ok((out, n))
    }

    /// Removes a growing thing's file never pointed into (it didn't grow).
    pub fn discard(&self, kept: &Kept) {
        let _ = std::fs::remove_file(self.dir.join(&kept.file));
    }

    /// Removes the files nothing points into (`used`: the files that are):
    /// the ones removed.
    pub fn forget_all_but(&self, used: &BTreeSet<String>) -> Vec<String> {
        let mut files = self.lock();
        let mut gone = Vec::new();
        if self.linked() {
            return gone;
        }
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(n) = number(&file) else { continue };
            if !used.contains(&file) && std::fs::remove_file(entry.path()).is_ok() {
                if files.shared == Some(n) {
                    files.shared = None;
                }
                gone.push(file);
            }
        }
        gone
    }
}

/// The line before a thing saying what it is.
fn heading(header: &str) -> String {
    format!("\n== {}\n", header.replace('\n', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn things_are_written_after_what_they_are_and_read_back_whole_or_their_start() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path());
        let a = logs.write("flaky · terminal t1 command 3 · tagged by user", "$ make\nerror: 1\n").unwrap();
        let b = logs.write("flaky · terminal t1 line 4", "and more").unwrap();
        assert_eq!((logs.read(&a).unwrap().as_str(), logs.read(&b).unwrap().as_str()), ("$ make\nerror: 1\n", "and more"));
        assert_eq!(logs.head(&a, 3).unwrap(), ("$ m".to_owned(), 2));
        assert_eq!(logs.head(&b, 100).unwrap(), ("and more".to_owned(), 1));
        let file = std::fs::read_to_string(dir.path().join(&a.file)).unwrap();
        assert!(file.contains("== flaky · terminal t1 command 3 · tagged by user\n$ make"), "{file}");
        // A growing thing: a file of its own, read to its end as it grows.
        let (growing, mut more) = logs.grow("slow · terminal t1 command 4").unwrap();
        assert_ne!(growing.file, a.file);
        more.write_all(b"1\n").unwrap();
        let c = logs.write("x", "y").unwrap();
        assert_eq!(c.file, a.file, "whole things never go in a growing thing's file");
        more.write_all(b"2\n").unwrap();
        assert_eq!(logs.read(&growing).unwrap(), "1\n2\n");
        // Only the server's user reads them.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir.path().join(&a.file)).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // A file nothing points into goes; whole things start another.
        let gone = logs.forget_all_but(&BTreeSet::from([growing.file.clone()]));
        assert_eq!(gone, std::slice::from_ref(&a.file));
        let d = logs.write("x", "z").unwrap();
        assert!(d.file != a.file && d.file != growing.file);
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
        assert!(![d.file.clone(), growing.file.clone()].contains(&Logs::open(dir.path()).write("x", "z").unwrap().file));
    }
}
