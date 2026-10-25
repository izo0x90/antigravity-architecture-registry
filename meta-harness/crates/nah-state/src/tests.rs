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
    // A chat's unsent text is its own row, by slot, not part of the layout.
    assert!(db.drafts().unwrap().is_empty());
    let link = harness_contracts::Link { start: 0, end: 6, to: harness_contracts::Linked::Thing { subject: harness_contracts::Subject::CodeFile { path: "a.rs".into() } }, include: harness_contracts::Include::Whole };
    let linked = harness_contracts::Message { text: "@a.rs to the lead, more".into(), links: vec![link] };
    db.set_draft("lead", &"to the lead".into()).unwrap();
    db.set_draft("sub-1", &"to the reviewer".into()).unwrap();
    db.set_draft("lead", &linked).unwrap();
    assert_eq!(db.drafts().unwrap(), [("lead".to_owned(), linked), ("sub-1".to_owned(), "to the reviewer".into())].into(), "with its links");
    db.set_draft("sub-1", &"".into()).unwrap();
    assert_eq!(db.drafts().unwrap().keys().collect::<Vec<_>>(), ["lead"], "emptied: gone");
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
        let (lead, sub) = (SlotId::new("lead"), SlotId::new("s2"));
        for (n, text) in ["one", "two"].iter().enumerate() {
            db.hold_report(&HeldReport { to: lead.clone(), from: "sub 1 (sub-slot)".into(), message: (*text).into(), at_ms: n as u64 }).unwrap();
        }
        // The user's, in a discussion: its links kept with it.
        let link = harness_contracts::Link { start: 4, end: 6, to: harness_contracts::Linked::Tag { tag: harness_contracts::TagId::new("x") }, include: harness_contracts::Include::Auto };
        db.hold_report(&HeldReport { to: sub.clone(), from: "the user".into(), message: harness_contracts::Message { text: "see #x".into(), links: vec![link.clone()] }, at_ms: 3 }).unwrap();
        let taken = db.take_reports(&lead).unwrap();
        assert_eq!(taken.iter().map(|r| r.message.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert!(db.take_reports(&lead).unwrap().is_empty(), "taken: gone");
        assert_eq!(db.take_reports(&sub).unwrap().iter().map(|r| (r.from.as_str(), r.message.text.as_str(), r.message.links.clone())).collect::<Vec<_>>(), [("the user", "see #x", vec![link])], "each slot's own");
        db.note("lead", "sent", "sub 1", None);
        assert_eq!(db.activity(1).unwrap()[0].action, "sent");
    }

}

mod notifications {
    use super::*;
    use harness_contracts::{Notification, NotificationId, NotificationKind, NotificationState, SlotId};
    use nah_notify::NotificationStore;

    fn note(id: &str, kind: NotificationKind, at_ms: u64) -> Notification {
        Notification { id: NotificationId::new(id), server: None, level: kind.level(), kind, slot: Some(SlotId::new("sub-1")), thread: None, request: None, terminal: None, discussion: None, title: id.into(), detail: None, at_ms, state: NotificationState::Open }
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

mod terminals {
    use harness_contracts::{TermSize, Terminal, TerminalId, TerminalStatus};

    use super::*;

    fn terminal(id: &str, title: &str) -> Terminal {
        Terminal { id: TerminalId::new(id), title: title.into(), status: TerminalStatus::Running, size: TermSize { cols: 80, rows: 24 }, shell: "/bin/bash".into(), folder: "/p".into() }
    }

    #[test]
    fn terminals_come_back_ended_in_the_order_opened() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = StateDb::open(dir.path(), &settings()).unwrap();
            db.keep_terminal(&terminal("t1", "bash")).unwrap();
            db.keep_terminal(&terminal("t2", "bash")).unwrap();
            db.keep_terminal(&terminal("t1", "make")).unwrap();
            db.keep_terminal(&terminal("t3", "bash")).unwrap();
            db.forget_terminal(&TerminalId::new("t3")).unwrap();
        }
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        let kept = db.terminals().unwrap();
        assert_eq!(kept.iter().map(|t| (t.id.as_str(), t.title.as_str())).collect::<Vec<_>>(), [("t1", "make"), ("t2", "bash")], "kept again: its place stays");
        assert!(kept.iter().all(|t| t.status == TerminalStatus::Ended { code: None }), "their shells didn't outlive the server");
    }

    #[test]
    fn tags_are_kept_in_the_order_made_and_each_change_replaces_what_was_kept_with_who_made_it() {
        use nah_tags::{Change, TagStore};
        let dir = tempfile::tempdir().unwrap();
        let tag = |id: &str, name: &str| harness_contracts::Tag { id: harness_contracts::TagId::new(id), name: name.into(), colour: 1, note: String::new(), noted_by: String::new(), on: Vec::new() };
        {
            let db = StateDb::open(dir.path(), &settings()).unwrap();
            let change = |action, detail: &str| Change { by: "lead".into(), action, detail: detail.into() };
            db.put(&tag("a", "first"), Some(&change("tagged", "terminal t1 command 3"))).unwrap();
            db.put(&tag("b", "second"), None).unwrap();
            db.put(&tag("a", "renamed"), Some(&change("renamed", "was first"))).unwrap();
            db.put(&tag("c", "gone"), None).unwrap();
            db.remove(&tag("c", "gone"), &change("deleted", "")).unwrap();
            let kept: Vec<_> = db.activity(10).unwrap().into_iter().map(|a| (a.actor, a.action, a.subject, a.detail)).collect();
            let entry = |action: &str, subject: &str, detail: Option<&str>| ("lead".to_owned(), action.to_owned(), subject.to_owned(), detail.map(str::to_owned));
            assert_eq!(kept, [entry("deleted", "tag gone", None), entry("renamed", "tag renamed", Some("was first")), entry("tagged", "tag first", Some("terminal t1 command 3"))], "who changed what, newest first; a move by its owner isn't anyone's");
        }
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        // One a later build kept, of a kind of subject this one doesn't know: left alone.
        db.transaction(|tx| tx.execute("INSERT INTO tags (id, name, json, seq) VALUES ('z', 'newer', '{\"id\":\"z\",\"name\":\"newer\",\"colour\":0,\"on\":[{\"subject\":{\"kind\":\"chat_message\"},\"said\":\"x\",\"atMs\":0,\"kept\":{\"file\":\"tagged-7.log\",\"start\":0}}]}', 99)", []).map(|_| ()).map_err(crate::failed)).unwrap();
        assert_eq!(db.tags().unwrap().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["renamed", "second"]);
        // Its logs stay its own: they're said to be used; and it's said
        // there's one this build can't read.
        assert_eq!(db.kept_files().unwrap(), std::collections::BTreeSet::from(["tagged-7.log".to_owned()]));
        assert_eq!(db.unreadable().unwrap(), 1);
    }

    #[test]
    fn discussions_are_kept_in_the_order_started_their_numbers_never_given_twice_with_who_changed_them() {
        use harness_contracts::{DiscussionId, DiscussionKind, DiscussionStatus, Kept, Named, Subject, TerminalId};
        use nah_discussions::{Change, DiscussionStore, Stored};
        let dir = tempfile::tempdir().unwrap();
        let on = Named { subject: Subject::TerminalLines { terminal: TerminalId::new("t1"), shell: 1, first: 3, last: 4, command: None }, what: "terminal t1 lines 3-4".into() };
        let kept = |file: &str| Kept { file: file.into(), start: 10, len: None, lines: None };
        let discussion = |n: u64, status| Stored { id: DiscussionId::new(format!("d{n}")), on: on.clone(), kept: Some(kept(&format!("tagged-{n}.log"))), kind: DiscussionKind::Question, by: "user".into(), asked: "lead".into(), status, said: Vec::new(), names: Default::default(), from: None };
        let change = |action| Change { by: "user".into(), action, detail: "d on it".into() };
        {
            let db = StateDb::open(dir.path(), &settings()).unwrap();
            assert_eq!((db.number().unwrap(), db.number().unwrap()), (1, 2));
            db.put(&discussion(1, DiscussionStatus::Open), Some(&change("started"))).unwrap();
            db.put(&discussion(2, DiscussionStatus::Open), Some(&change("started"))).unwrap();
            db.put(&discussion(1, DiscussionStatus::Settled), Some(&change("settled"))).unwrap();
            db.put(&discussion(1, DiscussionStatus::Settled), None).unwrap();
            db.remove(&discussion(2, DiscussionStatus::Open), &change("deleted")).unwrap();
            let logged: Vec<_> = db.activity(10).unwrap().into_iter().map(|a| (a.action, a.subject)).collect();
            assert_eq!(logged, [("deleted".to_owned(), "discussion d2".to_owned()), ("settled".into(), "discussion d1".into()), ("started".into(), "discussion d2".into()), ("started".into(), "discussion d1".into())], "who changed what; a move by its thing's owner isn't anyone's");
        }
        let db = StateDb::open(dir.path(), &settings()).unwrap();
        assert_eq!(db.number().unwrap(), 3, "a number given (one deleted too) isn't given again, after a restart too");
        // One whole, as written: what's said, what an answer changed, its agents' names, where it came from.
        let mut whole = discussion(5, DiscussionStatus::Answered);
        whole.said = vec![
            nah_discussions::StoredSaid { by: "user".into(), at_ms: 1, kind: harness_contracts::SaidKind::Message, text: kept("said-5-0.log"), links: Vec::new(), changed: Vec::new() },
            nah_discussions::StoredSaid { by: "sub-1a2b".into(), at_ms: 2, kind: harness_contracts::SaidKind::Answer, text: kept("said-5-1.log"), links: Vec::new(), changed: vec![on.clone()] },
        ];
        (whole.asked, whole.from) = ("sub-1a2b".into(), Some("plan p comment c1".into()));
        whole.names.insert("sub-1a2b".into(), "reviewer".into());
        db.put(&whole, None).unwrap();
        assert_eq!(db.discussions().unwrap().into_iter().find(|d| d.id.as_str() == "d5"), Some(whole), "read back whole");
        db.remove(&discussion(5, DiscussionStatus::Answered), &change("deleted")).unwrap();
        db.put(&discussion(4, DiscussionStatus::Open), None).unwrap();
        db.transaction(|tx| tx.execute("INSERT INTO discussions (id, json) VALUES ('d9', '{\"id\":\"d9\",\"on\":{\"subject\":{\"kind\":\"later\"}},\"kept\":{\"file\":\"tagged-9.log\",\"start\":0}}')", []).map(|_| ()).map_err(crate::failed)).unwrap();
        let read: Vec<(String, DiscussionStatus)> = db.discussions().unwrap().into_iter().map(|d| (d.id.to_string(), d.status)).collect();
        assert_eq!(read, [("d1".to_owned(), DiscussionStatus::Settled), ("d4".into(), DiscussionStatus::Open)], "in the order started; one a later build kept is left alone");
        assert_eq!(db.unreadable().unwrap(), 1);
        let files: Vec<String> = db.kept_files().unwrap().into_iter().collect();
        assert_eq!(files, ["tagged-1.log", "tagged-4.log", "tagged-9.log"], "the logs they point into, a later build's too");
    }
}
