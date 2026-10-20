//! The tags' own logs: what a tagged thing says, kept for a thing its owner
//! doesn't keep for good (a terminal's output, a chat's message). Files in
//! one folder (`tagged-1.log`, `tagged-2.log`...), a new one once the last
//! passes `FILE_MOST`, appended to only; each piece after a line saying
//! what it is (`== <tag> · <what> · tagged by <who> · <when>`), for whoever
//! reads or searches the files by hand. A tag keeps where its pieces are
//! (`Kept`), never their text; a file no tag points into is removed.

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_contracts::{Kept, Piece};

/// How big a log file grows before the next one is started.
const FILE_MOST: u64 = 8 << 20;

pub struct Logs {
    dir: PathBuf,
    /// The file written to now (its number), once there's one.
    current: Mutex<Option<u64>>,
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
        let last = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| number(&e.ok()?.file_name().to_string_lossy())).max();
        Self { dir: dir.to_owned(), current: Mutex::new(last) }
    }

    /// Writes `text` after a line saying what it is (`header`): where it is.
    pub fn write(&self, header: &str, text: &str) -> std::io::Result<Piece> {
        let mut current = self.current.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        std::fs::create_dir_all(&self.dir)?;
        let mut n = current.unwrap_or(1);
        let size = |n: u64| std::fs::metadata(self.dir.join(name(n))).map_or(0, |m| m.len());
        if size(n) > FILE_MOST {
            n += 1;
        }
        *current = Some(n);
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join(name(n)))?;
        let header = format!("\n== {}\n", header.replace('\n', " "));
        file.write_all(header.as_bytes())?;
        let start = file.stream_position()?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        Ok(Piece { file: name(n), start, len: text.len() as u64 })
    }

    /// The text `kept` points at, its pieces in order.
    pub fn read(&self, kept: &Kept) -> std::io::Result<String> {
        let mut out = Vec::new();
        for piece in &kept.pieces {
            let mut file = std::fs::File::open(self.dir.join(&piece.file))?;
            file.seek(SeekFrom::Start(piece.start))?;
            file.take(piece.len).read_to_end(&mut out)?;
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// Removes the files nothing points into (`used`: the files that are).
    pub fn forget_all_but(&self, used: &BTreeSet<String>) {
        let mut current = self.current.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(n) = number(&file) else { continue };
            if !used.contains(&file) && std::fs::remove_file(entry.path()).is_ok() && *current == Some(n) {
                // The next piece starts a file of its own (the numbers go on).
                *current = Some(n + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_written_after_what_they_are_and_read_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let logs = Logs::open(dir.path());
        let a = logs.write("flaky · terminal t1 command 3 · tagged by user", "$ make\nerror: 1\n").unwrap();
        let b = logs.write("flaky · more", "and more\n").unwrap();
        assert_eq!(logs.read(&Kept { pieces: vec![a.clone(), b.clone()] }).unwrap(), "$ make\nerror: 1\nand more\n");
        let file = std::fs::read_to_string(dir.path().join(&a.file)).unwrap();
        assert!(file.contains("== flaky · terminal t1 command 3 · tagged by user\n$ make"), "{file}");
        // A file nothing points into goes; a new piece starts another.
        logs.forget_all_but(&BTreeSet::new());
        assert!(!dir.path().join(&a.file).exists());
        let c = logs.write("x", "y").unwrap();
        assert_ne!(c.file, a.file);
        // Opened again: it goes on from the last file.
        assert_eq!(Logs::open(dir.path()).write("x", "z").unwrap().file, c.file);
    }
}
