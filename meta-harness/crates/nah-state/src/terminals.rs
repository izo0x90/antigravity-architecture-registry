//! The terminals a server has open: which, and how they were started. Their
//! shells don't outlive the server: kept ones come back ended, to be
//! started again in the same folder.

use harness_contracts::{TermSize, Terminal, TerminalId, TerminalStatus};
use rusqlite::params;

use crate::{StateDb, StateError, failed};

impl StateDb {
    /// Every terminal kept, in the order opened, each ended.
    pub fn terminals(&self) -> Result<Vec<Terminal>, StateError> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT id, title, shell, folder, cols, rows FROM terminals ORDER BY seq").map_err(failed)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(Terminal {
                        id: TerminalId::new(r.get::<_, String>(0)?),
                        title: r.get(1)?,
                        shell: r.get(2)?,
                        folder: r.get(3)?,
                        size: TermSize { cols: r.get(4)?, rows: r.get(5)? },
                        status: TerminalStatus::Ended { code: None },
                    })
                })
                .map_err(failed)?;
            rows.collect::<Result<_, _>>().map_err(failed)
        })
    }

    /// Keeps `terminal` (replacing what was kept of it; its place in the order stays).
    pub fn keep_terminal(&self, terminal: &Terminal) -> Result<(), StateError> {
        self.transaction(|tx| {
            tx.execute(
                "INSERT INTO terminals (id, title, shell, folder, cols, rows, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, (SELECT COALESCE(MAX(seq), 0) + 1 FROM terminals))
                 ON CONFLICT (id) DO UPDATE SET title = excluded.title, shell = excluded.shell, folder = excluded.folder, cols = excluded.cols, rows = excluded.rows",
                params![terminal.id.as_str(), terminal.title, terminal.shell, terminal.folder, terminal.size.cols, terminal.size.rows],
            )
            .map_err(failed)?;
            Ok(())
        })
    }

    pub fn forget_terminal(&self, id: &TerminalId) -> Result<(), StateError> {
        self.transaction(|tx| tx.execute("DELETE FROM terminals WHERE id = ?1", params![id.as_str()]).map(|_| ()).map_err(failed))
    }
}
