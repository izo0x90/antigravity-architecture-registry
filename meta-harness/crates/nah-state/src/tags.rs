//! Tags as kept: each by its id, in the order made, the whole of it as
//! written (its name too, as a column, to look at by hand).

use harness_contracts::{Tag, TagId};
use nah_tags::TagStore;
use rusqlite::params;

use crate::{StateDb, StateError, failed};

impl TagStore for StateDb {
    fn tags(&self) -> Result<Vec<Tag>, String> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT json FROM tags ORDER BY seq").map_err(failed)?;
            let rows: Vec<String> = stmt.query_map([], |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
            rows.into_iter().map(|json| serde_json::from_str(&json).map_err(|e| StateError::Invalid(format!("a kept tag can't be read ({e}): {json}")))).collect()
        })
        .map_err(|e| e.to_string())
    }

    fn put(&self, tag: &Tag) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO tags (id, name, json, seq) VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), 0) + 1 FROM tags))
                 ON CONFLICT (id) DO UPDATE SET name = excluded.name, json = excluded.json",
                params![tag.id.as_str(), tag.name, serde_json::to_string(tag).map_err(failed)?],
            )
            .map_err(failed)?;
            Ok(())
        })
        .map_err(|e| e.to_string())
    }

    fn remove(&self, id: &TagId) -> Result<(), String> {
        self.transaction(|tx| tx.execute("DELETE FROM tags WHERE id = ?1", params![id.as_str()]).map(|_| ()).map_err(failed)).map_err(|e| e.to_string())
    }
}
