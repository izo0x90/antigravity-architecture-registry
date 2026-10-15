//! Slots as kept: what each is and its sessions' harness ids. Whether a
//! session runs isn't kept (that's live).

use harness_contracts::{InstanceId, NativeId, SessionKey, Slot, SlotId, SlotSession, SlotStatus, Starter};
use harness_core::slots::{HeldReport, SlotStore};
use rusqlite::{OptionalExtension, params};

use crate::{Activity, StateDb, StateError, failed, log_in};

/// A `slots` row: id, role, name, instance, policy, model, started by.
type SlotRow = (String, String, String, Option<String>, String, Option<String>, String);

fn starter_word(s: Starter) -> &'static str {
    match s {
        Starter::User => "user",
        Starter::Lead => "lead",
        Starter::Queue => "queue",
    }
}

fn starter_of(word: &str) -> Result<Starter, StateError> {
    match word {
        "user" => Ok(Starter::User),
        "lead" => Ok(Starter::Lead),
        "queue" => Ok(Starter::Queue),
        other => Err(StateError::Invalid(format!("slot started by {other:?}: not user, lead or queue"))),
    }
}

impl StateDb {
    fn load_slots(&self) -> Result<Vec<Slot>, StateError> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT id, role, name, instance, policy_json, model_json, started_by FROM slots ORDER BY seq").map_err(failed)?;
            let rows: Vec<SlotRow> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
                .map_err(failed)?
                .collect::<Result<_, _>>()
                .map_err(failed)?;
            let mut sessions_of = tx.prepare("SELECT key, native_id, title, current FROM slot_sessions WHERE slot = ?1 ORDER BY seq").map_err(failed)?;
            let mut slots = Vec::new();
            for (id, role, name, instance, policy, model, started_by) in rows {
                let mut current = None;
                let sessions = sessions_of
                    .query_map(params![id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, bool>(3)?)))
                    .map_err(failed)?
                    .map(|row| {
                        let (key, native_id, title, is_current) = row.map_err(failed)?;
                        if is_current {
                            current = Some(SessionKey::new(key.clone()));
                        }
                        Ok(SlotSession { key: SessionKey::new(key), native_id: native_id.map(NativeId::new), title, thread: None })
                    })
                    .collect::<Result<Vec<_>, StateError>>()?;
                slots.push(Slot {
                    id: SlotId::new(id),
                    role,
                    name,
                    instance_id: instance.map(InstanceId::new),
                    policy: serde_json::from_str(&policy).map_err(failed)?,
                    model: model.map(|m| serde_json::from_str(&m)).transpose().map_err(failed)?,
                    sessions,
                    current,
                    started_by: starter_of(&started_by)?,
                    status: SlotStatus::Ready,
                });
            }
            Ok(slots)
        })
    }

    fn save_slot(&self, slot: &Slot) -> Result<(), StateError> {
        let policy = serde_json::to_string(&slot.policy).map_err(failed)?;
        let model = slot.model.as_ref().map(serde_json::to_string).transpose().map_err(failed)?;
        self.transaction(|tx| {
            let seq: i64 = tx.query_row("SELECT seq FROM slots WHERE id = ?1", params![slot.id.as_str()], |r| r.get(0)).optional().map_err(failed)?.map_or_else(
                || tx.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM slots", [], |r| r.get(0)).map_err(failed),
                Ok,
            )?;
            tx.execute(
                "INSERT INTO slots (id, role, name, instance, policy_json, model_json, started_by, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET role = excluded.role, name = excluded.name, instance = excluded.instance,
                   policy_json = excluded.policy_json, model_json = excluded.model_json, started_by = excluded.started_by",
                params![slot.id.as_str(), slot.role, slot.name, slot.instance_id.as_ref().map(|i| i.as_str()), policy, model, starter_word(slot.started_by), seq],
            )
            .map_err(failed)?;
            tx.execute("DELETE FROM slot_sessions WHERE slot = ?1", params![slot.id.as_str()]).map_err(failed)?;
            for (n, s) in slot.sessions.iter().enumerate() {
                tx.execute(
                    "INSERT INTO slot_sessions (key, slot, native_id, title, current, seq) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![s.key.as_str(), slot.id.as_str(), s.native_id.as_ref().map(|n| n.as_str()), s.title, slot.current.as_ref() == Some(&s.key), n as i64],
                )
                .map_err(failed)?;
            }
            Ok(())
        })
    }

    fn remove_slot(&self, id: &SlotId) -> Result<(), StateError> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM slot_sessions WHERE slot = ?1", params![id.as_str()]).map_err(failed)?;
            tx.execute("DELETE FROM slots WHERE id = ?1", params![id.as_str()]).map_err(failed)?;
            Ok(())
        })
    }
}

impl SlotStore for StateDb {
    fn load(&self) -> Result<Vec<Slot>, String> {
        self.load_slots().map_err(|e| e.to_string())
    }

    fn save(&self, slot: &Slot) -> Result<(), String> {
        self.save_slot(slot).map_err(|e| e.to_string())
    }

    fn remove(&self, id: &SlotId) -> Result<(), String> {
        self.remove_slot(id).map_err(|e| e.to_string())
    }

    fn hold_report(&self, report: &HeldReport) -> Result<(), String> {
        self.transaction(|tx| {
            tx.execute("INSERT INTO held_reports (from_slot, text, at_ms) VALUES (?1, ?2, ?3)", params![report.from, report.text, report.at_ms as i64]).map_err(failed)?;
            Ok(())
        })
        .map_err(|e| e.to_string())
    }

    fn take_reports(&self) -> Result<Vec<HeldReport>, String> {
        self.transaction(|tx| {
            let mut stmt = tx.prepare("SELECT from_slot, text, at_ms FROM held_reports ORDER BY seq").map_err(failed)?;
            let reports = stmt
                .query_map([], |r| Ok(HeldReport { from: r.get(0)?, text: r.get(1)?, at_ms: r.get::<_, i64>(2)? as u64 }))
                .map_err(failed)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(failed)?;
            tx.execute("DELETE FROM held_reports", []).map_err(failed)?;
            Ok(reports)
        })
        .map_err(|e| e.to_string())
    }

    fn note(&self, actor: &str, action: &str, subject: &str, detail: Option<&str>) {
        let at_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let entry = Activity { at_ms, actor: actor.into(), action: action.into(), subject: subject.into(), detail: detail.map(str::to_owned) };
        if let Err(e) = self.transaction(|tx| log_in(tx, &entry)) {
            eprintln!("activity not kept: {e}");
        }
    }
}
