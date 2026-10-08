//! The store as one SQLite file (`.nah/cache/index.sqlite`): per source
//! file its facts, its indexer output and its occurrences; one row for the
//! combined result. WAL mode, one connection (the service writes from one
//! place at a time, reads it whole at start-up, and reads one file's
//! occurrences on request).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, params};

use arch_graph::RepoPath;
use code_analyzer::Occurrence;

use crate::{Change, Commit, FactStore, Loaded, STORE_VERSION, Stamp, StoreError, Stored};

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
            c.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS indexed; DROP TABLE IF EXISTS combined; DROP TABLE IF EXISTS occurrences;").map_err(failed)?;
        }
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                size INTEGER NOT NULL,
                modified INTEGER NOT NULL,
                facts TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS indexed (
                path TEXT PRIMARY KEY,
                data TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS combined (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                key TEXT NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS occurrences (
                path TEXT PRIMARY KEY,
                data TEXT NOT NULL
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
    fn load(&self) -> Result<Loaded, StoreError> {
        let c = self.connection()?;
        let mut loaded = Loaded::default();
        let mut statement = c.prepare("SELECT size, modified, facts FROM files").map_err(failed)?;
        let rows = statement
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))
            .map_err(failed)?;
        for row in rows {
            let (size, modified, facts) = row.map_err(failed)?;
            // A row that doesn't read is as good as missing: its file is read again.
            if let Ok(extract) = serde_json::from_str(&facts) {
                loaded.files.push(Stored { extract, stamp: Stamp { size: size as u64, modified } });
            }
        }
        let mut statement = c.prepare("SELECT data FROM indexed").map_err(failed)?;
        for row in statement.query_map([], |r| r.get::<_, String>(0)).map_err(failed)? {
            if let Ok(file) = serde_json::from_str(&row.map_err(failed)?) {
                loaded.indexed.push(file);
            }
        }
        let combined: Option<(String, String)> = c.query_row("SELECT key, data FROM combined WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        loaded.combined = combined.and_then(|(key, data)| serde_json::from_str(&data).ok().map(|c| (key, c)));
        Ok(loaded)
    }

    fn commit(&self, commit: &Commit<'_>) -> Result<(), StoreError> {
        let mut c = self.connection()?;
        let tx = c.transaction().map_err(failed)?;
        for change in commit.files {
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
        if let Some(indexed) = commit.indexed {
            for file in indexed.drop {
                tx.execute("DELETE FROM indexed WHERE path = ?1", params![file.as_str()]).map_err(failed)?;
            }
            for file in indexed.put {
                let data = serde_json::to_string(file).map_err(failed)?;
                tx.execute("INSERT OR REPLACE INTO indexed (path, data) VALUES (?1, ?2)", params![file.path.as_str(), data]).map_err(failed)?;
            }
        }
        if let Some(rows) = commit.combined {
            let data = serde_json::to_string(rows.combined).map_err(failed)?;
            tx.execute("INSERT OR REPLACE INTO combined (id, key, data) VALUES (1, ?1, ?2)", params![rows.key, data]).map_err(failed)?;
            tx.execute("DELETE FROM occurrences", []).map_err(failed)?;
            {
                let mut insert = tx.prepare("INSERT INTO occurrences (path, data) VALUES (?1, ?2)").map_err(failed)?;
                for (path, list) in rows.occurrences {
                    insert.execute(params![path.as_str(), serde_json::to_string(list).map_err(failed)?]).map_err(failed)?;
                }
            }
        }
        tx.commit().map_err(failed)
    }

    fn occurrences(&self, file: &RepoPath) -> Result<Option<Vec<Occurrence>>, StoreError> {
        let c = self.connection()?;
        let data: Option<String> = c.query_row("SELECT data FROM occurrences WHERE path = ?1", params![file.as_str()], |r| r.get(0)).ok();
        data.map(|d| serde_json::from_str(&d).map_err(failed)).transpose()
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
        self.connection()?.execute_batch("DELETE FROM files; DELETE FROM indexed; DELETE FROM combined; DELETE FROM occurrences;").map_err(failed)?;
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

    fn put(file: &str) -> Commit<'static> {
        let change = Box::leak(Box::new([Change::Put(Box::new(Stored { extract: extract(file, "h1"), stamp: Stamp::default() }))]));
        Commit { files: change, ..Commit::default() }
    }

    // req: M.11, AP.7
    #[test]
    fn the_sqlite_store_keeps_the_contract_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".nah/cache/index.sqlite");
        exercise(&SqliteStore::open(&path, &settings()).unwrap());
        let store = SqliteStore::open(&path, &settings()).unwrap();
        store.commit(&put("src/a.rs")).unwrap();
        drop(store);
        let again = SqliteStore::open(&path, &settings()).unwrap();
        assert_eq!(again.load().unwrap().files.len(), 1, "kept across a restart");
    }

    // req: M.11
    #[test]
    fn a_store_from_another_version_or_that_cant_be_read_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let store = SqliteStore::open(&path, &settings()).unwrap();
        store.commit(&put("src/a.rs")).unwrap();
        // Written by another reader: its facts can't be trusted.
        store.connection().unwrap().execute("UPDATE meta SET value = 'an older reader' WHERE key = 'writer'", []).unwrap();
        drop(store);
        assert_eq!(SqliteStore::open(&path, &settings()).unwrap().load().unwrap(), Loaded::default(), "thrown away");
        // Not a database at all.
        SqliteStore::remove(&path);
        std::fs::write(&path, "this is not a database, just bytes, enough of them to look like a header").unwrap();
        let store = SqliteStore::open(&path, &settings()).unwrap();
        assert_eq!(store.load().unwrap(), Loaded::default(), "started again");
        store.commit(&put("src/a.rs")).unwrap();
        assert_eq!(store.load().unwrap().files.len(), 1, "and works");
    }
}
