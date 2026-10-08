//! The store as one SQLite file (`.nah/cache/index.sqlite`): one row per
//! source file. WAL mode, one connection (the service writes from one place
//! at a time, and reads it whole once, at start-up).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};

use crate::{Change, FactStore, STORE_VERSION, Stamp, StoreError, Stored};

/// SQLite settings (from the service's config).
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// `PRAGMA synchronous`: "normal" is safe with WAL and much faster than "full".
    pub synchronous: String,
    /// Page cache size in KiB.
    pub cache_kib: i64,
}

pub struct SqliteStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

fn failed(e: impl std::fmt::Display) -> StoreError {
    StoreError::Failed(e.to_string())
}

/// What wrote a store: its layout, and the reader whose facts it holds. A
/// store from any other is rebuilt from the code.
fn writer() -> String {
    format!("store {STORE_VERSION} · facts {} · reader {}", arch_graph::SCHEMA_VERSION, code_analyzer::EXTRACTOR_VERSION)
}

impl SqliteStore {
    /// Opens (or creates) the store at `path`. One written by another
    /// version, or that can't be read, is thrown away and started again.
    pub fn open(path: &Path, settings: &Settings) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(failed)?;
        }
        let connection = match Self::connect(path, settings) {
            Ok(c) => c,
            Err(_) => {
                Self::remove(path);
                Self::connect(path, settings)?
            }
        };
        Ok(Self { path: path.to_path_buf(), connection: Mutex::new(connection) })
    }

    /// The file and its write-ahead log.
    fn remove(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.as_os_str().to_owned();
            p.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }

    fn connect(path: &Path, settings: &Settings) -> Result<Connection, StoreError> {
        let c = Connection::open(path).map_err(failed)?;
        c.pragma_update(None, "journal_mode", "wal").map_err(failed)?;
        c.pragma_update(None, "synchronous", &settings.synchronous).map_err(failed)?;
        c.pragma_update(None, "cache_size", -settings.cache_kib).map_err(failed)?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);").map_err(failed)?;
        let written_by: Option<String> = c.query_row("SELECT value FROM meta WHERE key = 'writer'", [], |r| r.get(0)).ok();
        if written_by.as_deref() != Some(writer().as_str()) {
            c.execute_batch("DROP TABLE IF EXISTS files;").map_err(failed)?;
        }
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                facts TEXT NOT NULL
            );",
        )
        .map_err(failed)?;
        c.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('writer', ?1)", params![writer()]).map_err(failed)?;
        // A file that isn't a database fails here, not later.
        c.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0)).map_err(failed)?;
        Ok(c)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.connection.lock().map_err(|_| StoreError::Failed("lock poisoned".into()))
    }
}

impl FactStore for SqliteStore {
    fn load(&self) -> Result<Vec<Stored>, StoreError> {
        let c = self.connection()?;
        let mut statement = c.prepare("SELECT size, modified, facts FROM files").map_err(failed)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
            .map_err(failed)?;
        let mut out = Vec::new();
        for row in rows {
            let (size, modified, facts) = row.map_err(failed)?;
            // A row that doesn't read is as good as missing: its file is read again.
            if let Ok(extract) = serde_json::from_str(&facts) {
                out.push(Stored { extract, stamp: Stamp { size: size as u64, modified } });
            }
        }
        Ok(out)
    }

    fn apply(&self, changes: &[Change]) -> Result<(), StoreError> {
        let mut c = self.connection()?;
        let tx = c.transaction().map_err(failed)?;
        for change in changes {
            match change {
                Change::Put(stored) => {
                    let facts = serde_json::to_string(&stored.extract).map_err(failed)?;
                    tx.execute(
                        "INSERT OR REPLACE INTO files (path, size, modified, facts) VALUES (?1, ?2, ?3, ?4)",
                        params![stored.extract.file.as_str(), stored.stamp.size as i64, stored.stamp.modified, facts],
                    )
                    .map_err(failed)?;
                }
                Change::Restamp { file, stamp } => {
                    tx.execute("UPDATE files SET size = ?2, modified = ?3 WHERE path = ?1", params![file.as_str(), stamp.size as i64, stamp.modified]).map_err(failed)?;
                }
                Change::Remove(file) => {
                    tx.execute("DELETE FROM files WHERE path = ?1", params![file.as_str()]).map_err(failed)?;
                }
            }
        }
        tx.commit().map_err(failed)
    }

    /// The file and its write-ahead log.
    fn bytes(&self) -> u64 {
        ["", "-wal"]
            .iter()
            .map(|suffix| {
                let mut p = self.path.as_os_str().to_owned();
                p.push(suffix);
                std::fs::metadata(PathBuf::from(p)).map_or(0, |m| m.len())
            })
            .sum()
    }

    fn clear(&self) -> Result<(), StoreError> {
        self.connection()?.execute("DELETE FROM files", []).map_err(failed)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{exercise, extract};

    fn settings() -> Settings {
        Settings { synchronous: "normal".into(), cache_kib: 8192 }
    }

    // req: M.11
    #[test]
    fn the_sqlite_store_keeps_the_contract_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".nah/cache/index.sqlite");
        exercise(&SqliteStore::open(&path, &settings()).unwrap());
        let store = SqliteStore::open(&path, &settings()).unwrap();
        store.apply(&[Change::Put(Box::new(Stored { extract: extract("src/a.rs", "h1"), stamp: Stamp::default() }))]).unwrap();
        drop(store);
        let again = SqliteStore::open(&path, &settings()).unwrap();
        assert_eq!(again.load().unwrap().len(), 1, "kept across a restart");
    }

    // req: M.11
    #[test]
    fn a_store_from_another_version_or_that_cant_be_read_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let store = SqliteStore::open(&path, &settings()).unwrap();
        store.apply(&[Change::Put(Box::new(Stored { extract: extract("src/a.rs", "h1"), stamp: Stamp::default() }))]).unwrap();
        // Written by another reader: its facts can't be trusted.
        store.connection().unwrap().execute("UPDATE meta SET value = 'an older reader' WHERE key = 'writer'", []).unwrap();
        drop(store);
        assert!(SqliteStore::open(&path, &settings()).unwrap().load().unwrap().is_empty(), "thrown away");
        // Not a database at all.
        SqliteStore::remove(&path);
        std::fs::write(&path, "this is not a database, just bytes, enough of them to look like a header").unwrap();
        let store = SqliteStore::open(&path, &settings()).unwrap();
        assert!(store.load().unwrap().is_empty(), "started again");
        store.apply(&[Change::Put(Box::new(Stored { extract: extract("src/a.rs", "h1"), stamp: Stamp::default() }))]).unwrap();
        assert_eq!(store.load().unwrap().len(), 1, "and works");
    }
}
