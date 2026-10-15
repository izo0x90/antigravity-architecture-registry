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
