//! The database's layouts, in order. A layout is never edited once shipped:
//! a change is a new one. Each runs in its own transaction, which sets
//! `user_version` when it commits.

use rusqlite::Connection;

use crate::{StateError, failed};

/// (version, what it adds). Features add theirs as they're built.
const MIGRATIONS: &[(i64, &str)] = &[(
    1,
    "CREATE TABLE ui_state (id INTEGER PRIMARY KEY CHECK (id = 1), json TEXT NOT NULL);
     CREATE TABLE feedback (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, json TEXT NOT NULL);
     CREATE TABLE activity (seq INTEGER PRIMARY KEY AUTOINCREMENT, at_ms INTEGER NOT NULL, actor TEXT NOT NULL, action TEXT NOT NULL, subject TEXT NOT NULL, detail TEXT);",
)];

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
