use super::*;

fn settings() -> arch_store::Settings {
    arch_store::Settings { synchronous: "normal".into(), cache_kib: 1024, busy_retry_us: 50, busy_give_up_ms: 1000 }
}

#[test]
fn it_opens_empty_at_the_latest_layout_and_keeps_what_it_holds() {
    let dir = tempfile::tempdir().unwrap();
    {
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        assert_eq!(db.ui_state().unwrap(), None);
        db.set_ui_state(&serde_json::json!({ "columns": 3 })).unwrap();
        assert!(db.set_ui_state(&serde_json::json!([1])).is_err(), "only an object");
        assert_eq!(db.put_feedback("a", &serde_json::json!({ "id": "a", "n": 1 })).unwrap(), 1);
        assert_eq!(db.put_feedback("b", &serde_json::json!({ "id": "b" })).unwrap(), 2);
        assert_eq!(db.put_feedback("a", &serde_json::json!({ "id": "a", "n": 2 })).unwrap(), 2, "same id: replaced");
        db.log(&Activity { at_ms: 5, actor: "user".into(), action: "opened".into(), subject: "slot lead".into(), detail: None }).unwrap();
    }
    let db = StateDb::open(dir.path(), &settings()).unwrap();
    assert_eq!(db.ui_state().unwrap(), Some(serde_json::json!({ "columns": 3 })));
    assert_eq!(db.feedback().unwrap(), [serde_json::json!({ "id": "a", "n": 2 }), serde_json::json!({ "id": "b" })], "in the order they came");
    assert_eq!(db.activity(10).unwrap()[0].action, "opened");
    let version: i64 = db.conn.lock().unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, LATEST);
}

#[test]
fn a_second_server_on_the_same_project_is_refused_with_the_first_ones_pid() {
    let dir = tempfile::tempdir().unwrap();
    let _first = StateDb::open(dir.path(), &settings()).unwrap();
    let err = StateDb::open(dir.path(), &settings()).err().unwrap();
    assert_eq!(err, StateError::Locked { pid: std::process::id().to_string() });
    assert!(err.to_string().starts_with("another server is running for this project (pid "), "{err}");
}

#[test]
fn the_lock_goes_with_the_server() {
    let dir = tempfile::tempdir().unwrap();
    drop(StateDb::open(dir.path(), &settings()).unwrap());
    assert!(StateDb::open(dir.path(), &settings()).is_ok(), "closed: free to open again");
}

#[test]
fn a_database_from_a_newer_nah_is_refused_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = Connection::open(dir.path().join("state.sqlite")).unwrap();
        conn.pragma_update(None, "user_version", LATEST + 1).unwrap();
    }
    let err = StateDb::open(dir.path(), &settings()).err().unwrap();
    assert!(matches!(err, StateError::Newer { found, known, .. } if found == LATEST + 1 && known == LATEST), "{err:?}");
    let version: i64 = Connection::open(dir.path().join("state.sqlite")).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, LATEST + 1, "left as it was");
}

#[test]
fn a_damaged_database_is_refused_with_its_path_and_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("state.sqlite"), b"this isn't a database, nor is it 100 bytes of one").unwrap();
    let err = StateDb::open(dir.path(), &settings()).err().unwrap().to_string();
    assert!(err.contains("state.sqlite") && err.ends_with("move it aside to start with an empty one"), "{err}");
    assert!(dir.path().join("state.sqlite").exists(), "never deleted");
}

#[test]
fn a_transaction_that_fails_keeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(dir.path(), &settings()).unwrap();
    let err = db.transaction(|tx| {
        log_in(tx, &Activity { at_ms: 1, actor: "user".into(), action: "half".into(), subject: "x".into(), detail: None })?;
        Err::<(), _>(StateError::Invalid("stopped half way".into()))
    });
    assert!(err.is_err());
    assert!(db.activity(10).unwrap().is_empty(), "the half that ran isn't kept");
}

mod slots {
    use super::*;
    use harness_contracts::{InstanceId, NativeId, RuntimeMode, RuntimePolicy, SessionKey, Slot, SlotId, SlotSession, SlotStatus, Starter, ThreadId};
    use harness_core::slots::{HeldReport, SlotStore};

    fn slot(id: &str) -> Slot {
        Slot {
            id: SlotId::new(id),
            role: "sub".into(),
            name: format!("{id} name"),
            instance_id: Some(InstanceId::new("claude-code")),
            policy: RuntimePolicy { runtime_mode: RuntimeMode::FullAccess, ..RuntimePolicy::default() },
            model: None,
            sessions: vec![
                SlotSession { key: SessionKey::new(format!("{id}-k1")), native_id: Some(NativeId::new("n1")), title: Some("first".into()), thread: Some(ThreadId::new("live")) },
                SlotSession { key: SessionKey::new(format!("{id}-k2")), native_id: None, title: None, thread: None },
            ],
            current: Some(SessionKey::new(format!("{id}-k2"))),
            started_by: Starter::Lead,
            status: SlotStatus::Ready,
        }
    }

    #[test]
    fn a_slot_comes_back_as_kept_with_nothing_running() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = StateDb::open(dir.path(), &settings()).unwrap();
            db.save(&slot("b")).unwrap();
            db.save(&slot("a")).unwrap();
            let mut renamed = slot("b");
            renamed.name = "renamed".into();
            renamed.sessions.remove(0);
            db.save(&renamed).unwrap();
        }
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        let loaded = db.load().unwrap();
        assert_eq!(loaded.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["b", "a"], "in the order they were made");
        assert_eq!(loaded[0].name, "renamed");
        assert_eq!(loaded[0].sessions.len(), 1, "a session forgotten is gone");
        let a = &loaded[1];
        assert_eq!((a.current.as_ref().map(|k| k.as_str()), a.policy.runtime_mode, a.started_by), (Some("a-k2"), RuntimeMode::FullAccess, Starter::Lead));
        assert_eq!(a.sessions[0].native_id.as_ref().map(|n| n.as_str()), Some("n1"));
        assert!(a.sessions.iter().all(|s| s.thread.is_none()), "running isn't kept");
        db.remove(&SlotId::new("a")).unwrap();
        assert_eq!(db.load().unwrap().len(), 1);
    }

    #[test]
    fn held_reports_come_out_once_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        for (n, text) in ["one", "two"].iter().enumerate() {
            db.hold_report(&HeldReport { from: "sub 1".into(), text: (*text).into(), at_ms: n as u64 }).unwrap();
        }
        let taken = db.take_reports().unwrap();
        assert_eq!(taken.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert!(db.take_reports().unwrap().is_empty(), "taken: gone");
        db.note("lead", "sent", "sub 1", None);
        assert_eq!(db.activity(1).unwrap()[0].action, "sent");
    }
}

mod notifications {
    use super::*;
    use harness_contracts::{Notification, NotificationId, NotificationKind, NotificationState, SlotId};
    use nah_notify::NotificationStore;

    fn note(id: &str, kind: NotificationKind, at_ms: u64) -> Notification {
        Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: Some(SlotId::new("sub-1")), thread: None, request: None, title: id.into(), detail: None, at_ms, state: NotificationState::Open }
    }

    #[test]
    fn they_are_kept_by_id_and_a_restart_expires_only_what_waited() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        let (ask, done) = (note("req:r1", NotificationKind::Approval, 1), note("turn:u1", NotificationKind::TurnFinished, 2));
        db.put(&[ask.clone(), done.clone()]).unwrap();
        db.put(std::slice::from_ref(&ask)).unwrap();
        assert_eq!(db.open().unwrap(), [ask.clone(), done.clone()], "kept once, by id");
        assert_eq!(db.count("req:").unwrap(), 1);
        assert_eq!(db.count("req_").unwrap(), 0, "a prefix is literal");
        assert_eq!(db.expire_waiting().unwrap(), 1);
        assert_eq!(db.get(&ask.id).unwrap().map(|n| n.state), Some(NotificationState::Expired));
        assert_eq!(db.open().unwrap(), std::slice::from_ref(&done));
        assert_eq!(db.recent(10).unwrap().iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["turn:u1", "req:r1"], "newest first");
        assert_eq!(db.get(&NotificationId::new("nope")).unwrap(), None);
    }
}

mod servers {
    use super::*;
    use crate::ServerEntry;

    fn entry(name: &str) -> ServerEntry {
        ServerEntry { name: name.into(), url: "http://100.64.0.2:8080".into(), secret: "s3cret".into(), added_at_ms: 1 }
    }

    #[test]
    fn servers_are_kept_by_name_with_their_secret_and_the_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        db.add_server(&entry("cloud")).unwrap();
        db.add_server(&ServerEntry { url: "http://127.0.0.1:9090/".into(), ..entry("box") }).unwrap();
        assert_eq!(db.servers().unwrap().iter().map(|s| (s.name.as_str(), s.url.as_str())).collect::<Vec<_>>(), [("box", "http://127.0.0.1:9090"), ("cloud", "http://100.64.0.2:8080")]);
        assert_eq!(db.server("cloud").unwrap().map(|s| s.secret), Some("s3cret".into()));
        for (bad, says) in [(entry("cloud"), "already"), (entry("here"), "this server's own"), (entry("a b"), "letters, digits"), (ServerEntry { url: "https://x".into(), ..entry("x") }, "http://host:port"), (ServerEntry { url: "http://h:1/x/".into(), ..entry("p") }, "no path"), (ServerEntry { secret: " ".into(), ..entry("y") }, "meta-harness token")] {
            let err = db.add_server(&bad).unwrap_err().to_string();
            assert!(err.contains(says), "{err}");
        }
        assert!(db.remove_server("cloud").unwrap() && !db.remove_server("cloud").unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(db.path()).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
}

mod timeline {
    use super::*;
    use nah_timeline::{Kept, TimelineStore, TurnSpan};

    #[test]
    fn snapshots_and_turns_are_kept_and_let_go() {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        let (a, b) = (Kept { id: "1-a".into(), at_ms: 1, previous: None, files: 0 }, Kept { id: "2-b".into(), at_ms: 2, previous: Some("1-a".into()), files: 3 });
        db.add_snapshot(&b).unwrap();
        db.add_snapshot(&a).unwrap();
        assert_eq!(db.snapshots().unwrap(), [a.clone(), b.clone()], "oldest first");
        let mut turn = TurnSpan { turn: "u1".into(), thread: "t1".into(), slot: "lead".into(), started_ms: 1, start: "1-a".into(), ended_ms: None, end: None, files: None, closed_ms: None };
        db.put_turn(&turn).unwrap();
        (turn.ended_ms, turn.end, turn.files) = (Some(2), Some("2-b".into()), Some(3));
        db.put_turn(&turn).unwrap();
        db.thread_closed("t1", 5).unwrap();
        db.thread_closed("t1", 9).unwrap();
        turn.closed_ms = Some(5);
        assert_eq!(db.turns().unwrap(), [turn], "written again with its end; closed once");
        db.forget(&["1-a".into()], &["u1".into()]).unwrap();
        assert_eq!((db.snapshots().unwrap(), db.turns().unwrap().len()), (vec![b], 0));
    }
}
