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
    (
        6,
        "CREATE TABLE timeline_snapshots (id TEXT PRIMARY KEY, at_ms INTEGER NOT NULL, previous TEXT, files INTEGER NOT NULL);
     CREATE TABLE timeline_turns (turn TEXT PRIMARY KEY, thread TEXT NOT NULL, slot TEXT NOT NULL, started_ms INTEGER NOT NULL, start TEXT NOT NULL, ended_ms INTEGER, end_snapshot TEXT, files INTEGER, closed_ms INTEGER);
     CREATE INDEX timeline_turns_by_thread ON timeline_turns (thread);",
    ),
    (7, "CREATE TABLE terminals (id TEXT PRIMARY KEY, title TEXT NOT NULL, shell TEXT NOT NULL, folder TEXT NOT NULL, cols INTEGER NOT NULL, rows INTEGER NOT NULL, seq INTEGER NOT NULL);"),
    (8, "CREATE TABLE tags (id TEXT PRIMARY KEY, name TEXT NOT NULL, json TEXT NOT NULL, seq INTEGER NOT NULL);"),
    // A tag only points: what a thing said is no longer kept with it.
    (9, "UPDATE tags SET json = json_set(json, '$.on', (SELECT json_group_array(json(json_remove(value, '$.said'))) FROM json_each(tags.json, '$.on')));"),
    // Was a conversion of logs only this branch's first builds wrote; it
    // converts nothing now. A database already at 10 still opens.
    (10, ""),
    // Discussions: each as kept (its messages' text is in files); numbers
    // given out never come back (one deleted keeps its number).
    (
        11,
        "CREATE TABLE discussions (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL UNIQUE, json TEXT NOT NULL);
     CREATE TABLE discussion_numbers (n INTEGER PRIMARY KEY AUTOINCREMENT);",
    ),
    // What's held is for any slot (a discussion's news to the slot asked),
    // and says who it's from as the slot reads it.
    (
        12,
        "ALTER TABLE held_reports ADD COLUMN to_slot TEXT NOT NULL DEFAULT 'lead';
     UPDATE held_reports SET from_slot = from_slot || ' (sub-slot)';",
    ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_9_drops_what_tagged_things_said() {
        let mut conn = Connection::open_in_memory().unwrap();
        for (_, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 8) {
            conn.execute_batch(sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 8).unwrap();
        let old = r#"{"id":"x","name":"seen","colour":1,"note":"","notedBy":"","on":[{"subject":{"kind":"terminal_lines","terminal":"t1","shell":1,"first":3,"last":3},"what":"terminal t1 lines 3-3","said":"a secret","atMs":5,"by":"user"}]}"#;
        conn.execute("INSERT INTO tags (id, name, json, seq) VALUES ('x', 'seen', ?1, 1)", [old]).unwrap();
        run(&mut conn, "test").unwrap();
        let json: String = conn.query_row("SELECT json FROM tags", [], |r| r.get(0)).unwrap();
        let tag: harness_contracts::Tag = serde_json::from_str(&json).unwrap();
        assert!(!json.contains("a secret") && tag.on[0].what == "terminal t1 lines 3-3" && tag.on[0].at_ms == 5, "{json}");
    }
}
