//! The project's state database (`.nah/state/state.sqlite`): what the server
//! keeps across restarts that isn't the code's analysis. Unlike the analysis
//! cache it's never thrown away: a database it can't read, or one made by a
//! newer NAH, is refused with what to do, never replaced.
//!
//! One server per project: opening takes an OS lock on `.nah/state/lock`,
//! which dies with the process. Every operation is one short transaction
//! on one connection; callers on async threads go through `spawn_blocking`.
//! Features keep their tables here, each behind its own store interface
//! (the feature's crate defines the interface; this crate implements it).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;

mod discussions;
mod migrations;
mod notifications;
mod servers;
mod slots;
mod tags;
mod terminals;
mod timeline;

pub use servers::{ServerEntry, check_name};

pub use migrations::LATEST;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum StateError {
    #[error("another server is running for this project (pid {pid}); stop it first, or use that one")]
    Locked { pid: String },
    #[error("{path}: made by a newer NAH (layout {found}; this one knows up to {known}); update NAH")]
    Newer { path: String, found: i64, known: i64 },
    #[error("{path}: can't be read ({reason}); move it aside to start with an empty one")]
    Damaged { path: String, reason: String },
    #[error("{0}")]
    Invalid(String),
    #[error("state database: {0}")]
    Failed(String),
}

fn failed(e: impl std::fmt::Display) -> StateError {
    StateError::Failed(e.to_string())
}

/// The state database, open and locked for this server.
pub struct StateDb {
    path: PathBuf,
    conn: Mutex<Connection>,
    /// Held for as long as the server runs: the OS lock lives with it.
    _lock: File,
}

impl StateDb {
    /// Opens `<dir>/state.sqlite`, taking `<dir>/lock` first.
    pub fn open(dir: &Path, settings: &arch_store::Settings) -> Result<Self, StateError> {
        std::fs::create_dir_all(dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
        let lock = take_lock(&dir.join("lock"))?;
        let path = dir.join("state.sqlite");
        let shown = path.display().to_string();
        let damaged = |e: rusqlite::Error| StateError::Damaged { path: shown.clone(), reason: e.to_string() };
        let mut conn = Connection::open(&path).map_err(damaged)?;
        owner_only(&path)?;
        arch_store::configure(&conn, settings).map_err(damaged)?;
        migrations::run(&mut conn, &shown)?;
        Ok(Self { path, conn: Mutex::new(conn), _lock: lock })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs `f` in one transaction: all of it is kept, or none.
    pub fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T, StateError>) -> Result<T, StateError> {
        let mut conn = self.conn.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tx = conn.transaction().map_err(failed)?;
        let out = f(&tx)?;
        tx.commit().map_err(failed)?;
        Ok(out)
    }

    // ---- the cockpit's saved views ----

    /// The cockpit's state as it last saved it.
    pub fn ui_state(&self) -> Result<Option<Value>, StateError> {
        self.transaction(|tx| {
            let text: Option<String> =
                tx.query_row("SELECT json FROM ui_state WHERE id = 1", [], |r| r.get(0)).optional().map_err(failed)?;
            text.map(|t| serde_json::from_str(&t).map_err(failed)).transpose()
        })
    }

    /// The cockpit's own state as home keeps it (which server it shows,
    /// its settings): none before it ever saved.
    pub fn ui_shell(&self) -> Result<Option<Value>, StateError> {
        self.transaction(|tx| {
            let text: Option<String> =
                tx.query_row("SELECT json FROM ui_shell WHERE id = 1", [], |r| r.get(0)).optional().map_err(failed)?;
            text.map(|t| serde_json::from_str(&t).map_err(failed)).transpose()
        })
    }

    pub fn set_ui_shell(&self, state: &Value) -> Result<(), StateError> {
        if !state.is_object() {
            return Err(StateError::Invalid("the cockpit's own state is a JSON object".into()));
        }
        let text = serde_json::to_string(state).map_err(failed)?;
        self.transaction(|tx| {
            tx.execute("INSERT INTO ui_shell (id, json) VALUES (1, ?1) ON CONFLICT (id) DO UPDATE SET json = excluded.json", params![text])
                .map_err(failed)?;
            Ok(())
        })
    }

    /// Replaces it (a JSON object; the shape is the cockpit's).
    pub fn set_ui_state(&self, state: &Value) -> Result<(), StateError> {
        if !state.is_object() {
            return Err(StateError::Invalid("the cockpit's state is a JSON object".into()));
        }
        let text = serde_json::to_string(state).map_err(failed)?;
        self.transaction(|tx| {
            tx.execute("INSERT INTO ui_state (id, json) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET json = excluded.json", params![text])
                .map_err(failed)?;
            Ok(())
        })
    }

    // ---- the chats' unsent text ----

    /// Each slot's unsent message (its text and links), as the cockpit
    /// last saved it.
    pub fn drafts(&self) -> Result<BTreeMap<String, harness_contracts::Message>, StateError> {
        self.transaction(|tx| {
            let mut rows = tx.prepare("SELECT slot, text, links FROM drafts").map_err(failed)?;
            let found =
                rows.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(failed)?;
            found
                .map(|row| {
                    let (slot, text, links) = row.map_err(failed)?;
                    // Links this build can't read: the text, unlinked.
                    Ok((slot, harness_contracts::Message { text, links: serde_json::from_str(&links).unwrap_or_default() }))
                })
                .collect()
        })
    }

    /// Replaces slot `slot`'s unsent message (empty: there is none).
    pub fn set_draft(&self, slot: &str, draft: &harness_contracts::Message) -> Result<(), StateError> {
        let links = serde_json::to_string(&draft.links).map_err(failed)?;
        self.transaction(|tx| {
            if draft.text.is_empty() {
                tx.execute("DELETE FROM drafts WHERE slot = ?1", params![slot]).map_err(failed)?;
            } else {
                tx.execute("INSERT INTO drafts (slot, text, links) VALUES (?1, ?2, ?3) ON CONFLICT (slot) DO UPDATE SET text = excluded.text, links = excluded.links", params![slot, draft.text, links]).map_err(failed)?;
            }
            Ok(())
        })
    }

    // ---- dev feedback ----

    /// Every feedback item, oldest first.
    pub fn feedback(&self) -> Result<Vec<Value>, StateError> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT json FROM feedback ORDER BY seq").map_err(failed)?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(failed)?;
            rows.map(|t| t.map_err(failed).and_then(|t| serde_json::from_str(&t).map_err(failed))).collect()
        })
    }

    /// Adds an item, or replaces the one with its id; how many there are now.
    pub fn put_feedback(&self, id: &str, item: &Value) -> Result<usize, StateError> {
        let text = serde_json::to_string(item).map_err(failed)?;
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO feedback (id, json) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET json = excluded.json",
                params![id, text],
            )
            .map_err(failed)?;
            tx.query_row("SELECT COUNT(*) FROM feedback", [], |r| r.get::<_, i64>(0)).map(|n| n as usize).map_err(failed)
        })
    }

    // ---- activity ----

    /// Notes that something happened: who did what to what.
    pub fn log(&self, entry: &Activity) -> Result<(), StateError> {
        self.transaction(|tx| log_in(tx, entry))
    }

    /// The last `limit` entries, newest first.
    pub fn activity(&self, limit: usize) -> Result<Vec<Activity>, StateError> {
        self.transaction(|tx| {
            let mut stmt =
                tx.prepare("SELECT at_ms, actor, action, subject, detail FROM activity ORDER BY seq DESC LIMIT ?1").map_err(failed)?;
            let rows = stmt
                .query_map(params![limit as i64], |r| {
                    Ok(Activity {
                        at_ms: r.get::<_, i64>(0)? as u64,
                        actor: r.get(1)?,
                        action: r.get(2)?,
                        subject: r.get(3)?,
                        detail: r.get(4)?,
                    })
                })
                .map_err(failed)?;
            rows.map(|r| r.map_err(failed)).collect()
        })
    }
}

/// One thing that happened.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Activity {
    pub at_ms: u64,
    /// Who: "user", or a slot's name.
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: Option<String>,
}

/// Logs inside a caller's transaction (so it's kept with what it describes).
pub fn log_in(tx: &Transaction<'_>, entry: &Activity) -> Result<(), StateError> {
    tx.execute(
        "INSERT INTO activity (at_ms, actor, action, subject, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![entry.at_ms as i64, entry.actor, entry.action, entry.subject, entry.detail],
    )
    .map_err(failed)?;
    Ok(())
}

/// Takes the lock file's OS lock and writes this process's id into it; if
/// another holds it, says which.
/// Only its owner reads it (home's holds other servers' secrets).
fn owner_only(path: &Path) -> Result<(), StateError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| failed(format!("{}: {e}", path.display())))?;
    }
    Ok(())
}

fn take_lock(path: &Path) -> Result<File, StateError> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| failed(format!("{}: {e}", path.display())))?;
    match file.try_lock() {
        Ok(()) => {
            file.set_len(0).and_then(|()| file.rewind()).and_then(|()| write!(file, "{}", std::process::id())).map_err(failed)?;
            Ok(file)
        }
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            Err(StateError::Locked { pid: if pid.trim().is_empty() { "unknown".into() } else { pid.trim().to_owned() } })
        }
        Err(std::fs::TryLockError::Error(e)) => Err(failed(format!("{}: {e}", path.display()))),
    }
}

#[cfg(test)]
mod tests;
