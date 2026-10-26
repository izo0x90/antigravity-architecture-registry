//! The timeline as kept: its snapshots, and each turn with the snapshots
//! at its start and end.

use nah_timeline::{Kept, TimelineStore, TurnSpan};
use rusqlite::params;

use crate::{StateDb, StateError, failed};

fn said<T>(r: Result<T, StateError>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}

impl TimelineStore for StateDb {
    fn snapshots(&self) -> Result<Vec<Kept>, String> {
        said(self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT id, at_ms, previous, files FROM timeline_snapshots ORDER BY at_ms, id").map_err(failed)?;
            let rows = stmt
                .query_map([], |r| Ok(Kept { id: r.get(0)?, at_ms: r.get::<_, i64>(1)? as u64, previous: r.get(2)?, files: r.get(3)? }))
                .map_err(failed)?;
            rows.collect::<Result<_, _>>().map_err(failed)
        }))
    }

    fn add_snapshot(&self, s: &Kept) -> Result<(), String> {
        said(self.transaction(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO timeline_snapshots (id, at_ms, previous, files) VALUES (?1, ?2, ?3, ?4)",
                params![s.id, s.at_ms as i64, s.previous, s.files],
            )
            .map_err(failed)?;
            Ok(())
        }))
    }

    fn turns(&self) -> Result<Vec<TurnSpan>, String> {
        said(self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT turn, thread, slot, started_ms, start, ended_ms, end_snapshot, files, closed_ms FROM timeline_turns ORDER BY started_ms, turn").map_err(failed)?;
            let ms = |v: Option<i64>| v.map(|v| v as u64);
            let rows = stmt
                .query_map([], |r| {
                    Ok(TurnSpan {
                        turn: r.get(0)?,
                        thread: r.get(1)?,
                        slot: r.get(2)?,
                        started_ms: r.get::<_, i64>(3)? as u64,
                        start: r.get(4)?,
                        ended_ms: ms(r.get(5)?),
                        end: r.get(6)?,
                        files: r.get(7)?,
                        closed_ms: ms(r.get(8)?),
                    })
                })
                .map_err(failed)?;
            rows.collect::<Result<_, _>>().map_err(failed)
        }))
    }

    fn put_turn(&self, t: &TurnSpan) -> Result<(), String> {
        said(self.transaction(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO timeline_turns (turn, thread, slot, started_ms, start, ended_ms, end_snapshot, files, closed_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![t.turn, t.thread, t.slot, t.started_ms as i64, t.start, t.ended_ms.map(|v| v as i64), t.end, t.files, t.closed_ms.map(|v| v as i64)],
            )
            .map_err(failed)?;
            Ok(())
        }))
    }

    fn thread_closed(&self, thread: &str, at_ms: u64) -> Result<(), String> {
        said(self.transaction(|tx| {
            tx.execute("UPDATE timeline_turns SET closed_ms = ?2 WHERE thread = ?1 AND closed_ms IS NULL", params![thread, at_ms as i64])
                .map_err(failed)?;
            Ok(())
        }))
    }

    fn forget(&self, snapshots: &[String], turns: &[String]) -> Result<(), String> {
        said(self.transaction(|tx| {
            for id in snapshots {
                tx.execute("DELETE FROM timeline_snapshots WHERE id = ?1", params![id]).map_err(failed)?;
            }
            for turn in turns {
                tx.execute("DELETE FROM timeline_turns WHERE turn = ?1", params![turn]).map_err(failed)?;
            }
            Ok(())
        }))
    }
}
