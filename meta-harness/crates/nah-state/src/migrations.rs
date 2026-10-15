//! The database's layouts, in order. A layout is never edited once shipped:
//! a change is a new one. Each runs in its own transaction, which sets
//! `user_version` when it commits.

use rusqlite::Connection;

use crate::{StateError, failed};

/// (version, what it adds). Features add theirs as they're built.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE ui_state (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);
     CREATE TABLE feedback (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, json TEXT NOT NULL);
     CREATE TABLE activity (seq INTEGER PRIMARY KEY AUTOINCREMENT, at_ms INTEGER NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, subject TEXT NOT NULL, detail TEXT);",
    ),
    (
        2,
        "CREATE TABLE slots (id TEXT PRIMARY KEY, role TEXT NOT NULL, name TEXT NOT NULL, instance TEXT, policy_json TEXT NOT NULL, model_json TEXT, started_by TEXT NOT NULL, seq INTEGER NOT NULL);
     CREATE TABLE slot_sessions (key TEXT PRIMARY KEY, slot TEXT NOT NULL, native_id TEXT, title TEXT, current INTEGER NOT NULL, seq INTEGER NOT NULL);
     CREATE INDEX slot_sessions_by_slot ON slot_sessions (slot);
     CREATE TABLE held_reports (seq INTEGER PRIMARY KEY AUTOINCREMENT, from_slot TEXT NOT NULL, text TEXT NOT NULL, at_ms INTEGER NOT NULL);",
    ),
    (
        3,
        "CREATE TABLE notifications (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, level TEXT NOT NULL, state TEXT NOT NULL, at_ms INTEGER NOT NULL, json TEXT NOT NULL);
     CREATE INDEX notifications_by_state ON notifications (state, at_ms);",
    ),
    (4, "CREATE TABLE servers (name TEXT PRIMARY KEY, url TEXT NOT NULL, secret TEXT NOT NULL, added_at INTEGER NOT NULL);"),
    (5, "CREATE TABLE ui_shell (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);"),
];

/// The newest layout this NAH knows.
pub const LATEST: i64 = MIGRATIONS[MIGRATIONS.len() - 1].0;

pub(crate) fn run(conn: &mut Connection, shown: &str) -> Result<(), StateError> {
    let damaged = |e: rusqlite::Error| StateError::Damaged { path: shown.to_owned(), reason: e.to_string() };
    let found: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(damaged)?;
    if found > LATEST {
        return Err(StateError::Newer { path: shown.to_owned(), found, known: LATEST });
    }
    for (version, sql) in MIGRATIONS.iter().filter(|(v, _)| *v > found) {
        let tx = conn.transaction().map_err(failed)?;
        tx.execute_batch(sql).map_err(failed)?;
        tx.pragma_update(None, "user_version", version).map_err(failed)?;
        tx.commit().map_err(failed)?;
    }
    Ok(())
}
