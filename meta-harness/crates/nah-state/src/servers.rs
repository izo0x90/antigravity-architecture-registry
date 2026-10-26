//! The servers a home connects to: each by its name, with where it is and
//! its secret. The database file is owner-only (it holds the secrets).

use rusqlite::{OptionalExtension, params};

use harness_contracts::home::HERE;

use crate::{StateDb, StateError, failed};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub name: String,
    /// Where it is: `http://host:port`.
    pub url: String,
    pub secret: String,
    pub added_at_ms: u64,
}

/// A server's name: letters, digits, `-` and `_` (it's in paths), and not
/// `here` (home's own).
pub fn check_name(name: &str) -> Result<(), StateError> {
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(StateError::Invalid(format!("a server's name is letters, digits, - and _ only: {name:?}")));
    }
    if name == HERE {
        return Err(StateError::Invalid(format!("{HERE:?} is this server's own name: pick another")));
    }
    Ok(())
}

fn check_url(url: &str) -> Result<(), StateError> {
    let rest =
        url.strip_prefix("http://").ok_or_else(|| StateError::Invalid(format!("a server's address is http://host:port: {url:?}")))?;
    let host = rest.trim_end_matches('/');
    if host.is_empty() || host.contains('/') {
        return Err(StateError::Invalid(format!("a server's address is http://host:port, with no path: {url:?}")));
    }
    Ok(())
}

impl StateDb {
    /// Every server, in the order they were added.
    pub fn servers(&self) -> Result<Vec<ServerEntry>, StateError> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT name, url, secret, added_at FROM servers ORDER BY added_at, name").map_err(failed)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(ServerEntry { name: r.get(0)?, url: r.get(1)?, secret: r.get(2)?, added_at_ms: r.get::<_, i64>(3)? as u64 })
                })
                .map_err(failed)?;
            rows.collect::<Result<_, _>>().map_err(failed)
        })
    }

    pub fn server(&self, name: &str) -> Result<Option<ServerEntry>, StateError> {
        self.transaction(|tx| {
            tx.query_row("SELECT name, url, secret, added_at FROM servers WHERE name = ?1", params![name], |r| {
                Ok(ServerEntry { name: r.get(0)?, url: r.get(1)?, secret: r.get(2)?, added_at_ms: r.get::<_, i64>(3)? as u64 })
            })
            .optional()
            .map_err(failed)
        })
    }

    /// Adds a server (a name already taken is refused: remove it first).
    pub fn add_server(&self, entry: &ServerEntry) -> Result<(), StateError> {
        check_name(&entry.name)?;
        check_url(&entry.url)?;
        if entry.secret.trim().is_empty() {
            return Err(StateError::Invalid(format!(
                "{}: its secret is empty (`meta-harness token` on that server prints it)",
                entry.name
            )));
        }
        self.transaction(|tx| {
            let taken: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM servers WHERE name = ?1)", params![entry.name], |r| r.get(0)).map_err(failed)?;
            if taken {
                return Err(StateError::Invalid(format!("there's a server named {:?} already: remove it first", entry.name)));
            }
            tx.execute(
                "INSERT INTO servers (name, url, secret, added_at) VALUES (?1, ?2, ?3, ?4)",
                params![entry.name, entry.url.trim_end_matches('/'), entry.secret.trim(), entry.added_at_ms as i64],
            )
            .map_err(failed)?;
            Ok(())
        })
    }

    /// Removes a server: whether there was one by that name.
    pub fn remove_server(&self, name: &str) -> Result<bool, StateError> {
        self.transaction(|tx| Ok(tx.execute("DELETE FROM servers WHERE name = ?1", params![name]).map_err(failed)? > 0))
    }
}
