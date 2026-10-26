//! The timeline over a scratch repository and a store in memory: turns'
//! starts and ends take snapshots (one again when nothing changed), the
//! windows and who was working, each turn's changes, pruning.

use std::path::Path;

use harness_contracts::{ThreadId, Turn, TurnId};

use super::*;

#[derive(Default)]
struct Memory {
    snapshots: Mutex<Vec<Kept>>,
    turns: Mutex<Vec<TurnSpan>>,
}

impl TimelineStore for Memory {
    fn snapshots(&self) -> std::result::Result<Vec<Kept>, String> {
        Ok(self.snapshots.lock().unwrap().clone())
    }
    fn add_snapshot(&self, snapshot: &Kept) -> std::result::Result<(), String> {
        self.snapshots.lock().unwrap().push(snapshot.clone());
        Ok(())
    }
    fn turns(&self) -> std::result::Result<Vec<TurnSpan>, String> {
        Ok(self.turns.lock().unwrap().clone())
    }
    fn put_turn(&self, turn: &TurnSpan) -> std::result::Result<(), String> {
        let mut turns = self.turns.lock().unwrap();
        turns.retain(|t| t.turn != turn.turn);
        turns.push(turn.clone());
        Ok(())
    }
    fn thread_closed(&self, thread: &str, at_ms: u64) -> std::result::Result<(), String> {
        self.turns.lock().unwrap().iter_mut().filter(|t| t.thread == thread).for_each(|t| t.closed_ms = Some(at_ms));
        Ok(())
    }
    fn forget(&self, snapshots: &[String], turns: &[String]) -> std::result::Result<(), String> {
        self.snapshots.lock().unwrap().retain(|s| !snapshots.contains(&s.id));
        self.turns.lock().unwrap().retain(|t| !turns.contains(&t.turn));
        Ok(())
    }
}

fn write(root: &Path, path: &str, text: &str) {
    std::fs::write(root.join(path), text).unwrap();
}

/// A repository with one commit, its changes, a timeline over it.
fn timeline() -> (tempfile::TempDir, Arc<Changes>, Arc<Memory>, Timeline) {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    write(dir.path(), "a.txt", "one\n");
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("a.txt")).unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let who = git2::Signature::now("t", "t@t").unwrap();
    repo.commit(Some("HEAD"), &who, &who, "first", &tree, &[]).unwrap();
    let changes = Arc::new(Changes::open(dir.path()).unwrap());
    let memory = Arc::new(Memory::default());
    let timeline = Timeline::new(changes.clone(), memory.clone(), Policy::default()).unwrap();
    (dir, changes, memory, timeline)
}

fn turn(id: &str, thread: &str, status: TurnStatus) -> ClientEvent {
    ClientEvent::Session {
        event: AdapterEvent::TurnUpdated {
            turn: Turn { id: TurnId::new(id), thread_id: ThreadId::new(thread), ordinal: 1, status, failure: None, usage: None },
        },
    }
}

#[test]
fn turns_take_snapshots_and_windows_say_who_was_working() {
    let (dir, changes, memory, timeline) = timeline();
    let t0 = harness_core::now_ms();
    timeline.handle(&turn("t1", "a", TurnStatus::Running), t0);
    timeline.handle(&turn("t1", "a", TurnStatus::Running), t0 + 1);
    assert_eq!(memory.snapshots.lock().unwrap().len(), 1, "seen running again: not started again");
    write(dir.path(), "a.txt", "one\ntwo\n");
    timeline.handle(&turn("t2", "b", TurnStatus::Running), t0 + 10);
    write(dir.path(), "b.txt", "new\n");
    timeline.handle(&turn("t1", "a", TurnStatus::Completed), t0 + 20);
    let read = timeline.read(t0 + 30).unwrap();
    assert_eq!(read.windows.len(), 3, "three snapshots: two windows, and the open one: {read:#?}");
    fn whose(w: &Window) -> Vec<&str> {
        w.working.iter().map(|x| x.turn.as_str()).collect()
    }
    assert_eq!((whose(&read.windows[0]), read.windows[0].files), (vec!["t1"], 1), "t1 alone changed a.txt");
    assert_eq!((whose(&read.windows[1]), read.windows[1].files), (vec!["t1", "t2"], 1), "both, while b.txt came");
    assert_eq!((read.windows[2].to.as_ref(), whose(&read.windows[2])), (None, vec!["t2"]), "the open one: t2 still running");
    let t1 = read.turn("t1").unwrap();
    assert_eq!(t1.files, 2, "from its start to its end: a.txt and b.txt");
    assert_eq!(t1.alongside.iter().map(|w| w.turn.as_str()).collect::<Vec<_>>(), ["t2"], "t2 overlapped: said, not guessed apart");
    assert_eq!(changes.list(&t1.comparison, 0, 10).unwrap().total, 2, "its comparison reads");
    let t2 = read.turn("t2").unwrap();
    assert_eq!((t2.comparison.to.clone(), t2.files), (Point::Files, 1), "running: up to the files now");
    // Nothing changed since the last one: no new snapshot.
    let before = memory.snapshots.lock().unwrap().len();
    timeline.handle(&turn("t2", "b", TurnStatus::Completed), t0 + 40);
    timeline.handle(&turn("t3", "b", TurnStatus::Running), t0 + 50);
    assert_eq!(memory.snapshots.lock().unwrap().len(), before, "the same files: the same snapshot");
}

#[test]
fn a_closed_sessions_turns_and_old_snapshots_are_let_go() {
    let (dir, changes, memory, timeline) = timeline();
    let t0 = harness_core::now_ms();
    timeline.handle(&turn("t1", "a", TurnStatus::Running), t0);
    write(dir.path(), "a.txt", "two\n");
    timeline.handle(&turn("t1", "a", TurnStatus::Completed), t0 + 1);
    write(dir.path(), "a.txt", "three\n");
    timeline.handle(&turn("t2", "b", TurnStatus::Running), t0 + 2);
    assert_eq!(memory.snapshots.lock().unwrap().len(), 3);
    timeline.handle(&ClientEvent::ThreadClosed { thread_id: ThreadId::new("a") }, t0 + 3);
    timeline.prune(t0 + 3 + DAY_MS / 2).unwrap();
    assert_eq!(memory.turns.lock().unwrap().len(), 2, "closed half a day ago: kept");
    timeline.prune(t0 + 3 + DAY_MS + 1).unwrap();
    let turns: Vec<String> = memory.turns.lock().unwrap().iter().map(|t| t.turn.clone()).collect();
    assert_eq!(turns, ["t2"], "closed a day ago: let go");
    let kept: Vec<String> = memory.snapshots.lock().unwrap().iter().map(|s| s.id.clone()).collect();
    assert_eq!(kept.len(), 1, "only t2's start (the latest) is still needed");
    assert_eq!(changes.snapshots().unwrap().len(), 1, "and its ref is gone from git too");
    // The window counts still read with the ones before it gone.
    assert_eq!(timeline.read(t0 + 3 + DAY_MS + 2).unwrap().windows.len(), 1);
    timeline.prune(t0 + 15 * DAY_MS).unwrap();
    assert_eq!(memory.turns.lock().unwrap().len(), 0, "past keep_days: everything but the latest snapshot");
    assert_eq!(memory.snapshots.lock().unwrap().len(), 1);
}

#[test]
fn turns_left_running_by_a_server_that_stopped_are_let_go_and_the_policy_reads_from_toml() {
    let (_dir, changes, memory, timeline) = timeline();
    timeline.handle(&turn("t1", "a", TurnStatus::Running), harness_core::now_ms());
    drop(timeline);
    let again = Timeline::new(changes, memory.clone(), Policy::default()).unwrap();
    assert!(memory.turns.lock().unwrap().is_empty(), "its end was never seen");
    assert!(again.read(harness_core::now_ms()).unwrap().turns.is_empty());
    assert_eq!(Policy::from_toml("[timeline]\nkeep_days = 3\n").unwrap(), Policy { keep_days: 3, closed_session_days: 1 });
    assert_eq!(Policy::from_toml("").unwrap(), Policy::default());
    assert!(Policy::from_toml("[timeline]\nkeep = 3\n").is_err());
}
