//! Tags as kept: each by its id, in the order made, the whole of it as
//! written (its name too, as a column, to look at by hand); each change
//! with who made it, in the activity kept (in the same transaction).

use std::collections::BTreeSet;

use harness_contracts::Tag;
use nah_tags::{Change, TagStore};
use rusqlite::{Transaction, params};

use crate::{Activity, StateDb, StateError, failed, log_in};

/// `change` to `tag`, as activity.
fn logged(tx: &Transaction<'_>, tag: &Tag, change: &Change) -> Result<(), StateError> {
    let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let detail = (!change.detail.is_empty()).then(|| change.detail.clone());
    log_in(
        tx,
        &Activity { at_ms, actor: change.by.clone(), action: change.action.to_owned(), subject: format!("tag {}", tag.name), detail },
    )
}

impl TagStore for StateDb {
    /// Read from every tag as written, those this build can't read too
    /// (their logs stay theirs).
    fn kept_files(&self) -> Result<BTreeSet<String>, String> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT json FROM tags").map_err(failed)?;
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
            let mut files = BTreeSet::new();
            for json in rows {
                let tag: serde_json::Value = serde_json::from_str(&json).map_err(failed)?;
                for on in tag["on"].as_array().into_iter().flatten() {
                    if let Some(file) = on["kept"]["file"].as_str() {
                        files.insert(file.to_owned());
                    }
                }
            }
            Ok(files)
        })
        .map_err(|e| e.to_string())
    }

    fn tags(&self) -> Result<Vec<Tag>, String> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT json FROM tags ORDER BY seq").map_err(failed)?;
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
            // One this build can't read (a newer kind of subject) stays as kept.
            Ok(rows.into_iter().filter_map(|json| serde_json::from_str(&json).ok()).collect::<Vec<Tag>>())
        })
        .map_err(|e| e.to_string())
    }

    fn unreadable(&self) -> Result<usize, String> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT json FROM tags").map_err(failed)?;
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
            Ok(rows.iter().filter(|json| serde_json::from_str::<Tag>(json).is_err()).count())
        })
        .map_err(|e| e.to_string())
    }

    fn put(&self, tag: &Tag, change: Option<&Change>) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO tags (id, name, json, seq) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), 0) + 1 FROM tags))
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, json = excluded.json",
                params![tag.id.as_str(), tag.name, serde_json::to_string(tag).map_err(failed)?],
            )
            .map_err(failed)?;
            change.map_or(Ok(()), |c| logged(tx, tag, c))
        })
        .map_err(|e| e.to_string())
    }

    fn remove(&self, tag: &Tag, change: &Change) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM tags WHERE id = ?1", params![tag.id.as_str()]).map_err(failed)?;
            logged(tx, tag, change)
        })
        .map_err(|e| e.to_string())
    }
}
