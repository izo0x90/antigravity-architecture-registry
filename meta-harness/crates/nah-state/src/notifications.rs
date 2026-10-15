//! Notifications as kept: each by its id, with its level and state as
//! columns (to find the open ones) and the whole of it as written.

use harness_contracts::{Level, Notification, NotificationId, NotificationState};
use nah_notify::NotificationStore;
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{StateDb, StateError, failed};

fn word<T: serde::Serialize>(value: &T) -> Result<String, StateError> {
    match serde_json::to_value(value).map_err(failed)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(StateError::Invalid(format!("{other} isn't a word"))),
    }
}

fn read(json: String) -> Result<Notification, StateError> {
    serde_json::from_str(&json).map_err(|e| StateError::Invalid(format!("a kept notification can't be read ({e}): {json}")))
}

fn select(tx: &Transaction<'_>, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Notification>, StateError> {
    let mut stmt = tx.prepare(sql).map_err(failed)?;
    let rows: Vec<String> = stmt.query_map(args, |r| r.get(0)).map_err(failed)?.collect::<Result<_, _>>().map_err(failed)?;
    rows.into_iter().map(read).collect()
}

fn write(tx: &Transaction<'_>, n: &Notification) -> Result<(), StateError> {
    tx.execute(
        "INSERT INTO notifications (id, level, state, at_ms, json) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (id) DO UPDATE SET level = excluded.level, state = excluded.state, at_ms = excluded.at_ms, json = excluded.json",
        params![n.id.as_str(), word(&n.level)?, word(&n.state)?, n.at_ms as i64, serde_json::to_string(n).map_err(failed)?],
    )
    .map_err(failed)?;
    Ok(())
}

impl NotificationStore for StateDb {
    fn open(&self) -> Result<Vec<Notification>, String> {
        let open = word(&NotificationState::Open).map_err(|e| e.to_string())?;
        self.transaction(|tx| select(tx, "SELECT json FROM notifications WHERE state = ?1 ORDER BY at_ms, seq", params![open])).map_err(|e| e.to_string())
    }

    fn recent(&self, limit: usize) -> Result<Vec<Notification>, String> {
        self.transaction(|tx| select(tx, "SELECT json FROM notifications ORDER BY at_ms DESC, seq DESC LIMIT ?1", params![limit as i64])).map_err(|e| e.to_string())
    }

    fn get(&self, id: &NotificationId) -> Result<Option<Notification>, String> {
        self.transaction(|tx| tx.query_row("SELECT json FROM notifications WHERE id = ?1", params![id.as_str()], |r| r.get::<_, String>(0)).optional().map_err(failed)?.map(read).transpose())
            .map_err(|e| e.to_string())
    }

    fn put(&self, notifications: &[Notification]) -> Result<(), String> {
        self.transaction(|tx| notifications.iter().try_for_each(|n| write(tx, n))).map_err(|e| e.to_string())
    }

    fn count(&self, prefix: &str) -> Result<u32, String> {
        self.transaction(|tx| tx.query_row("SELECT COUNT(*) FROM notifications WHERE substr(id, 1, length(?1)) = ?1", params![prefix], |r| r.get(0)).map_err(failed)).map_err(|e| e.to_string())
    }

    fn expire_waiting(&self) -> Result<usize, String> {
        let (open, waiting) = (word(&NotificationState::Open).map_err(|e| e.to_string())?, word(&Level::Waiting).map_err(|e| e.to_string())?);
        self.transaction(|tx| {
            let waited = select(tx, "SELECT json FROM notifications WHERE state = ?1 AND level = ?2", params![open, waiting])?;
            for n in &waited {
                let expired = n.moved(NotificationState::Expired).map_err(|e| StateError::Invalid(e.to_string()))?;
                write(tx, &expired)?;
            }
            Ok(waited.len())
        })
        .map_err(|e| e.to_string())
    }
}
