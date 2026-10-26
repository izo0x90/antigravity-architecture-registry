//! Discussions as kept: each by its id, in the order started, as written
//! (where its messages' text is, not the text); each change with who made
//! it, in the activity kept (in the same transaction).

use std::collections::BTreeSet;

use nah_discussions::{Change, DiscussionStore, Stored};
use rusqlite::{Transaction, params};

use crate::{Activity, StateDb, StateError, failed, log_in};

/// `change` to `discussion`, as activity.
fn logged(tx: &Transaction<'_>, discussion: &Stored, change: &Change) -> Result<(), StateError> {
    let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let detail = (!change.detail.is_empty()).then(|| change.detail.clone());
    log_in(
        tx,
        &Activity {
            at_ms,
            actor: change.by.clone(),
            action: change.action.to_owned(),
            subject: format!("discussion {}", discussion.id),
            detail,
        },
    )
}

/// Every discussion as written.
fn rows(tx: &Transaction<'_>) -> Result<Vec<String>, StateError> {
    let mut stmt = tx.prepare("SELECT json FROM discussions ORDER BY seq").map_err(failed)?;
    stmt.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)
}

impl DiscussionStore for StateDb {
    fn discussions(&self) -> Result<Vec<Stored>, String> {
        // One this build can't read (a newer kind of subject) stays as kept.
        self.transaction(|tx| Ok(rows(tx)?.into_iter().filter_map(|json| serde_json::from_str(&json).ok()).collect()))
            .map_err(|e| e.to_string())
    }

    fn unreadable(&self) -> Result<usize, String> {
        self.transaction(|tx| Ok(rows(tx)?.iter().filter(|json| serde_json::from_str::<Stored>(json).is_err()).count()))
            .map_err(|e| e.to_string())
    }

    /// Read from every discussion as written, those this build can't read
    /// too (the logs they point into stay theirs).
    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        self.transaction(|tx| {
            let mut files = BTreeSet::new();
            for json in rows(tx)? {
                let discussion: serde_json::Value = serde_json::from_str(&json).map_err(failed)?;
                if let Some(file) = discussion["kept"]["file"].as_str() {
                    files.insert(file.to_owned());
                }
            }
            Ok(files)
        })
        .map_err(|e| e.to_string())
    }

    fn number(&self) -> Result<u64, String> {
        self.transaction(|tx| {
            tx.execute("INSERT INTO discussion_numbers DEFAULT VALUES", []).map_err(failed)?;
            let n = tx.last_insert_rowid();
            tx.execute("DELETE FROM discussion_numbers", []).map_err(failed)?;
            Ok(n as u64)
        })
        .map_err(|e| e.to_string())
    }

    fn put(&self, discussion: &Stored, change: Option<&Change>) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO discussions (id, json) VALUES (?1, ?2) ON CONFLICT (id) DO UPDATE SET json = excluded.json",
                params![discussion.id.as_str(), serde_json::to_string(discussion).map_err(failed)?],
            )
            .map_err(failed)?;
            change.map_or(Ok(()), |c| logged(tx, discussion, c))
        })
        .map_err(|e| e.to_string())
    }

    fn remove(&self, discussion: &Stored, change: &Change) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM discussions WHERE id = ?1", params![discussion.id.as_str()]).map_err(failed)?;
            logged(tx, discussion, change)
        })
        .map_err(|e| e.to_string())
    }
}
