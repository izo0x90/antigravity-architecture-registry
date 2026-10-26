//! The rebuild's structural gates, on a copy of this
//! repository's sources. What a change writes is counted by triggers the
//! test puts on every table (each insert, update and delete, by the file
//! that owns the row), not reported by the code under test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use arch_graph::TypeSystem;
use arch_service::{ArchitectureService, MemoryPlans};

mod common;

/// The plan's tables.
const PLAN_TABLES: [&str; 11] =
    ["meta", "files", "components", "refs", "relations", "flows", "placeholders", "rule_hits", "highlights", "servers", "gate_writes"];
/// Rows no file owns: the servers' status, and in
/// meta only the plan's two keys (the schema version, the packs'
/// fingerprints). Any other row without a file is a write "for no file".
const SERVERS: &str = "servers";
const META_KEYS: [&str; 2] = ["schema_version", "fingerprints"];

fn packs() -> code_analyzer::PackSet {
    common::shipped_packs()
}

fn open(root: &Path) -> ArchitectureService {
    let facts = arch_service::open_facts(root, &arch_service::Limits::defaults().unwrap()).unwrap();
    ArchitectureService::open(root, packs(), TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), facts).unwrap()
}

/// This repository's sources and manifests in a temp dir, dated an hour back, read once.
fn this_repo() -> (tempfile::TempDir, ArchitectureService) {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
    let dir = tempfile::tempdir().unwrap();
    common::copy_sources(&here, dir.path(), &packs(), SystemTime::now() - Duration::from_secs(3600));
    let service = open(dir.path());
    service.ingest(dir.path()).unwrap();
    (dir, service)
}

fn db(root: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(root.join(".nah/cache/index.sqlite")).unwrap()
}

fn tables(db: &rusqlite::Connection) -> BTreeSet<String> {
    db.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn columns(db: &rusqlite::Connection, table: &str) -> Vec<String> {
    db.prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// One logged write: table, insert/update/delete, the owning file ("*"
/// when the table has no file column), and for an update the columns that
/// changed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Write {
    table: String,
    op: String,
    owner: String,
    changed: String,
}

/// Puts a trigger on every table that logs each write into `gate_writes`.
fn log_writes(db: &rusqlite::Connection) {
    db.execute_batch("CREATE TABLE IF NOT EXISTS gate_writes (tbl TEXT, op TEXT, owner TEXT, changed TEXT)").unwrap();
    for table in tables(db).into_iter().filter(|t| t != "gate_writes") {
        let cols = columns(db, &table);
        // meta's rows are owned by their key.
        let owner = |row: &str| {
            cols.iter()
                .find(|c| *c == "file" || *c == "path" || (table == "meta" && *c == "key"))
                .map_or("'*'".to_owned(), |c| format!("{row}.\"{c}\""))
        };
        let changed = cols
            .iter()
            .map(|c| format!("CASE WHEN OLD.\"{c}\" IS NOT NEW.\"{c}\" THEN '{c},' ELSE '' END"))
            .collect::<Vec<_>>()
            .join(" || ");
        let changed = if changed.is_empty() { "''".to_owned() } else { changed };
        db.execute_batch(&format!(
            "CREATE TRIGGER IF NOT EXISTS gate_{table}_i AFTER INSERT ON \"{table}\" BEGIN INSERT INTO gate_writes VALUES ('{table}', 'insert', {}, ''); END;
             CREATE TRIGGER IF NOT EXISTS gate_{table}_d AFTER DELETE ON \"{table}\" BEGIN INSERT INTO gate_writes VALUES ('{table}', 'delete', {}, ''); END;
             CREATE TRIGGER IF NOT EXISTS gate_{table}_u AFTER UPDATE ON \"{table}\" BEGIN INSERT INTO gate_writes VALUES ('{table}', 'update', {}, {changed}); END;",
            owner("NEW"),
            owner("OLD"),
            owner("NEW"),
        ))
        .unwrap();
    }
}

/// Every table still has its three triggers (a table rebuilt during the
/// change would lose them, and its writes would go unseen).
fn triggers_intact(db: &rusqlite::Connection) {
    for table in tables(db).into_iter().filter(|t| t != "gate_writes") {
        let n: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?1 AND name LIKE 'gate_%'",
                [&table],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 3, "{table} was rebuilt during the change: its writes weren't all seen");
    }
}

fn writes(db: &rusqlite::Connection) -> Vec<Write> {
    triggers_intact(db);
    db.prepare("SELECT tbl, op, owner, changed FROM gate_writes")
        .unwrap()
        .query_map([], |r| {
            Ok(Write {
                table: r.get(0)?,
                op: r.get(1)?,
                owner: r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "*".into()),
                changed: r.get(3)?,
            })
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn clear_writes(db: &rusqlite::Connection) {
    db.execute("DELETE FROM gate_writes", []).unwrap();
}

/// The plan's schema, exactly.
fn plan_schema(db: &rusqlite::Connection) {
    let have = tables(db);
    let want: BTreeSet<String> = PLAN_TABLES.iter().map(|t| t.to_string()).collect();
    assert_eq!(have, want, "the tables aren't the plan's");
}

/// Rust files by how much other files use what they define (from the graph).
fn files_by_use(service: &ArchitectureService) -> Vec<String> {
    let graph = service.observed().unwrap();
    let file_of = |id| graph.component(id).and_then(|c| c.anchor.as_ref()).map(|a| a.file.as_str().to_owned());
    let mut uses: BTreeMap<String, usize> = BTreeMap::new();
    for r in graph.relations() {
        if let (Some(from), Some(to)) = (file_of(&r.source), file_of(&r.target))
            && from != to
            && to.ends_with(".rs")
        {
            *uses.entry(to).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = uses.into_iter().collect();
    ranked.sort_by_key(|(f, n)| (std::cmp::Reverse(*n), f.clone()));
    ranked.into_iter().map(|(f, _)| f).collect()
}

/// What a change to `file` may re-link:
/// itself, and the files with refs whose target was in it, or whose name it
/// defines now or defined before.
fn may_relink(before: &rusqlite::Connection, names_after: &BTreeSet<String>, file: &str) -> BTreeSet<String> {
    let ids: BTreeSet<String> = before
        .prepare("SELECT id FROM components WHERE file = ?1")
        .unwrap()
        .query_map([file], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut names: BTreeSet<String> = before
        .prepare("SELECT name FROM components WHERE file = ?1")
        .unwrap()
        .query_map([file], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    names.extend(names_after.iter().cloned());
    let mut out = BTreeSet::from([file.to_owned()]);
    let mut refs = before.prepare("SELECT file, name, target FROM refs").unwrap();
    for row in refs.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))).unwrap() {
        let (from, name, target) = row.unwrap();
        if names.contains(&name) || target.is_some_and(|t| ids.contains(&t)) {
            out.insert(from);
        }
    }
    out
}

#[test]
fn a_one_file_change_writes_only_its_rows_and_those_of_the_files_it_relinks() {
    let (dir, service) = this_repo();
    let conn = db(dir.path());
    log_writes(&conn);
    // The most-used file whose change can re-link at most half the repo
    // (else a whole-repo rewrite couldn't be told apart); before the
    // plan's tables exist, simply the most-used one.
    let ranked = files_by_use(&service);
    let file = if tables(&conn).contains("refs") {
        let all = conn.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap() as usize;
        ranked
            .iter()
            .find(|f| may_relink(&conn, &BTreeSet::new(), f).len() * 2 <= all)
            .cloned()
            .expect("a file whose change can re-link at most half the repo")
    } else {
        ranked[0].clone()
    };
    // The database as it was, to say what the change may re-link.
    let kept = tempfile::tempdir().unwrap();
    let snapshot = kept.path().join("before.sqlite");
    conn.execute("VACUUM INTO ?1", [snapshot.to_str().unwrap()]).unwrap();
    clear_writes(&conn);
    let path = dir.path().join(&file);
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{text}\npub fn added_by_the_gate() {{}}\n")).unwrap();
    service.refresh(&[path]).unwrap();
    let written = writes(&conn);
    assert!(written.iter().any(|w| w.owner == file), "the changed file's own rows weren't written");
    let not_a_file = |w: &&Write| w.table == SERVERS || (w.table == "meta" && META_KEYS.contains(&w.owner.as_str()));
    let no_file: BTreeSet<_> =
        written.iter().filter(|w| (w.owner == "*" || w.table == "meta") && !not_a_file(w)).map(|w| (&w.table, &w.op, &w.owner)).collect();
    assert!(no_file.is_empty(), "rows that belong to no one file were written: {no_file:?}");
    plan_schema(&conn);
    let names_after: BTreeSet<String> = conn
        .prepare("SELECT name FROM components WHERE file = ?1")
        .unwrap()
        .query_map([&file], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let allowed = may_relink(&rusqlite::Connection::open(&snapshot).unwrap(), &names_after, &file);
    let all = conn.query_row("SELECT count(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap() as usize;
    println!("gate one_file file={file} may_relink={} of {all} files", allowed.len());
    assert!(allowed.len() * 2 <= all, "{file} can re-link {} of {all} files: too wide to tell a whole-repo rewrite apart", allowed.len());
    let extra: BTreeSet<_> = written
        .iter()
        .filter(|w| w.owner != "*" && w.table != "meta" && w.table != SERVERS)
        .map(|w| w.owner.clone())
        .filter(|by| !allowed.contains(by))
        .collect();
    assert!(
        extra.is_empty(),
        "{} files' rows written that {file} can't re-link (of {} allowed), e.g. {:?}",
        extra.len(),
        allowed.len(),
        extra.iter().take(5).collect::<Vec<_>>()
    );
}

#[test]
fn a_content_identical_touch_relinks_nothing_writes_only_its_stamp_and_says_nothing() {
    let (dir, service) = this_repo();
    let conn = db(dir.path());
    log_writes(&conn);
    let events = Arc::new(AtomicUsize::new(0));
    let seen = events.clone();
    service.subscribe(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });
    let file = files_by_use(&service)[0].clone();
    let path = dir.path().join(&file);
    let graph = service.observed().unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
    service.refresh(&[path]).unwrap();
    assert!(Arc::ptr_eq(&graph, &service.observed().unwrap()), "nothing was re-linked: the graph is the same one");
    assert_eq!(events.load(Ordering::SeqCst), 0, "no event for a touch");
    for w in writes(&conn) {
        assert_eq!(
            (w.table.as_str(), w.op.as_str(), w.owner.as_str()),
            ("files", "update", file.as_str()),
            "only the touched file's stamp may be written: {w:?}"
        );
        assert!(
            w.changed.split(',').filter(|c| !c.is_empty()).all(|c| c == "size" || c == "modified"),
            "more than the stamp changed: {w:?}"
        );
    }
    plan_schema(&conn);
}

#[test]
fn a_restart_with_nothing_changed_reads_and_relinks_nothing_from_rows() {
    let (dir, service) = this_repo();
    let relations = service.observed().unwrap().relations().count();
    drop(service);
    let conn = db(dir.path());
    log_writes(&conn);
    // The real start-up: open, then ingest the repo.
    let service = open(dir.path());
    let report = service.ingest(dir.path()).unwrap();
    assert_eq!((report.read, report.parsed), (0, 0), "a restart with nothing changed read files: {report:?}");
    // Nothing re-linked: no file's rows were written again.
    let relinked: BTreeSet<_> =
        writes(&conn).into_iter().filter(|w| w.owner != "*" && w.table != "meta" && w.table != SERVERS).map(|w| w.owner).collect();
    assert!(relinked.is_empty(), "a restart with nothing changed re-linked {relinked:?}");
    assert_eq!(service.observed().unwrap().relations().count(), relations);
    // The graph came from the plan's rows.
    plan_schema(&conn);
}

#[test]
fn a_restart_builds_the_graph_from_component_and_relation_rows() {
    let (dir, service) = this_repo();
    let (components, relations) = {
        let graph = service.observed().unwrap();
        (graph.components().count(), graph.relations().count())
    };
    drop(service);
    // No source file is there now: the graph has to come from the store.
    for file in code_analyzer::walk(dir.path(), dir.path(), &packs()).unwrap() {
        std::fs::remove_file(&file.absolute).unwrap();
    }
    let service = open(dir.path());
    let graph = service.observed().unwrap();
    assert_eq!((graph.components().count(), graph.relations().count()), (components, relations));
    let conn = db(dir.path());
    log_writes(&conn);
    plan_schema(&conn);
    assert!(!columns(&conn, "files").contains(&"facts".to_owned()), "files still holds each file's reading as one blob");
}
