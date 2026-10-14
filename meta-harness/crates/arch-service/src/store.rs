//! Storage: plans (the only persisted architecture), where the facts read
//! from the code are kept between runs (a disposable cache, see
//! `arch-store`), and the cockpit's state. All live under `<repo>/.nah/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arch_graph::{Plan, PlanId};

use crate::ServiceError;

pub trait PlanRepository: Send + Sync {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError>;
    /// The plan as last saved. The same `Arc` comes back until it changes,
    /// so callers can tell an unchanged plan by pointer.
    fn get(&self, id: &PlanId) -> Result<Option<Arc<Plan>>, ServiceError>;
    /// Saves the whole plan atomically.
    fn save(&self, plan: &Plan) -> Result<(), ServiceError>;
    /// Removes a plan; `NotFound` if there is none.
    fn delete(&self, id: &PlanId) -> Result<(), ServiceError>;
    /// Whether `path` is where a plan is kept (the watcher hands those in).
    fn holds(&self, path: &Path) -> bool;
    /// Reads the plans among `paths` again (someone else changed their
    /// files); returns the plans that changed.
    fn changed(&self, paths: &[PathBuf]) -> Result<Vec<PlanId>, ServiceError>;
}

/// In-memory plans (embedding and tests).
#[derive(Default)]
pub struct MemoryPlans(Mutex<BTreeMap<PlanId, Arc<Plan>>>);

impl PlanRepository for MemoryPlans {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.keys().cloned().collect())
    }
    fn get(&self, id: &PlanId) -> Result<Option<Arc<Plan>>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.get(id).cloned())
    }
    fn save(&self, plan: &Plan) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.insert(plan.id.clone(), Arc::new(plan.clone()));
        Ok(())
    }
    fn delete(&self, id: &PlanId) -> Result<(), ServiceError> {
        match self.0.lock().map_err(ServiceError::poisoned)?.remove(id) {
            Some(_) => Ok(()),
            None => Err(ServiceError::NotFound(format!("plan {id}"))),
        }
    }
    fn holds(&self, _path: &Path) -> bool {
        false
    }
    fn changed(&self, _paths: &[PathBuf]) -> Result<Vec<PlanId>, ServiceError> {
        Ok(Vec::new())
    }
}

fn io(path: &Path, err: std::io::Error) -> ServiceError {
    ServiceError::Io(format!("{}: {err}", path.display()))
}

/// Writes to a temp file of its own (unique per process and write), flushes
/// it to disk, renames it into place and flushes the directory, so readers
/// (and other writers) never see a partial file and a crash leaves the old
/// one or the new one.
fn write_atomic(path: &Path, text: &str) -> Result<(), ServiceError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| ServiceError::Io(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.{}.tmp", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(&tmp, e));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io(path, e));
    }
    sync_dir(dir)
}

/// Makes a rename in `dir` durable (directories can't be opened for this on
/// every platform; where they can't, the rename is as durable as it gets).
#[cfg(unix)]
fn sync_dir(dir: &Path) -> Result<(), ServiceError> {
    std::fs::File::open(dir).and_then(|d| d.sync_all()).map_err(|e| io(dir, e))
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> Result<(), ServiceError> {
    Ok(())
}

/// What `.nah/.gitignore` keeps out of version control: the cache (rebuilt
/// from the code) and the cockpit's state (one person's screen). Plans stay
/// committed.
const NAH_IGNORED: [&str; 2] = [CACHE_DIR, UI_DIR];
const CACHE_DIR: &str = "cache";
const UI_DIR: &str = "ui";

/// Ensures `.nah/.gitignore` lists what's kept out of version control (adding
/// what an older one lacks), without touching the repository's own .gitignore.
fn ensure_nah_gitignore(nah: &Path) -> Result<(), ServiceError> {
    let ignore = nah.join(".gitignore");
    let text = match std::fs::read_to_string(&ignore) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "# Caches are rebuilt from the code and the cockpit's state is one person's; plans are kept.\n".to_owned(),
        Err(e) => return Err(io(&ignore, e)),
    };
    let missing: Vec<String> = NAH_IGNORED.into_iter().map(|dir| format!("{dir}/")).filter(|entry| !text.lines().any(|l| l.trim() == entry)).collect();
    if !missing.is_empty() || !ignore.exists() {
        let mut text = text;
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for entry in missing {
            text.push_str(&entry);
            text.push('\n');
        }
        write_atomic(&ignore, &text)?;
    }
    Ok(())
}

/// The cockpit's state (`<repo>/.nah/ui/state.json`): what it shows and
/// where, kept by the server so a reload, or another browser, comes back
/// to it. The server keeps it as the cockpit wrote it; its shape is the
/// cockpit's.
pub struct UiStateFile {
    path: PathBuf,
}

impl UiStateFile {
    pub fn open(root: &Path) -> Result<Self, ServiceError> {
        let nah = root.join(".nah");
        ensure_nah_gitignore(&nah)?;
        Ok(Self { path: nah.join(UI_DIR).join("state.json") })
    }

    /// The state as last written, if there is one.
    pub fn read(&self) -> Result<Option<serde_json::Value>, ServiceError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| ServiceError::Io(format!("{}: {e}", self.path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(&self.path, e)),
        }
    }

    /// Replaces the state (a JSON object).
    pub fn write(&self, state: &serde_json::Value) -> Result<(), ServiceError> {
        if !state.is_object() {
            return Err(ServiceError::InvalidOp("the cockpit's state is a JSON object".into()));
        }
        let text = serde_json::to_string_pretty(state).map_err(|e| ServiceError::Io(e.to_string()))?;
        write_atomic(&self.path, &text)
    }
}

/// Plans as `<repo>/.nah/plans/<id>.json`, all kept in memory: read once
/// when opened, and a file again only when the watcher says it changed.
/// Nothing is listed or stat'ed per
/// request.
pub struct FsPlans {
    dir: PathBuf,
    /// One writer at a time within this process (the server is the writer).
    write: Mutex<()>,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    /// Each plan file as last read or saved: the plan, or why it can't be read.
    plans: BTreeMap<PlanId, Result<Arc<Plan>, ServiceError>>,
    /// Files in the folder whose names can't be plan ids (put there by hand).
    misnamed: BTreeSet<String>,
}

impl FsPlans {
    pub fn new(repo_root: &Path) -> Result<Self, ServiceError> {
        let nah = repo_root.join(".nah");
        let dir = nah.join("plans");
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        ensure_nah_gitignore(&nah)?;
        let plans = Self { dir, write: Mutex::new(()), held: Mutex::new(Held::default()) };
        let files: Vec<PathBuf> = std::fs::read_dir(&plans.dir).map_err(|e| io(&plans.dir, e))?.map(|e| e.map(|e| e.path()).map_err(|e| io(&plans.dir, e))).collect::<Result<_, _>>()?;
        plans.changed(&files)?;
        Ok(plans)
    }

    fn path(&self, id: &PlanId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// The plan file at `path` as it is now: none if it's gone.
    fn read(&self, path: &Path, id: &PlanId) -> Option<Result<Plan, ServiceError>> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => return Some(Err(io(path, e))),
        };
        Some(Plan::from_json(&text).map_err(|e| ServiceError::Corrupt(format!("{}: {e}", path.display()))).and_then(|plan| {
            if &plan.id == id { Ok(plan) } else { Err(ServiceError::Corrupt(format!("{}: holds plan '{}', not '{id}'", path.display(), plan.id))) }
        }))
    }
}

impl PlanRepository for FsPlans {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError> {
        let held = self.held.lock().map_err(ServiceError::poisoned)?;
        // A plan file whose name can't be a plan id is reported, not skipped.
        if let Some(name) = held.misnamed.first() {
            return Err(ServiceError::Corrupt(format!("{}: the file name isn't a plan id", self.dir.join(name).display())));
        }
        Ok(held.plans.keys().cloned().collect())
    }

    fn get(&self, id: &PlanId) -> Result<Option<Arc<Plan>>, ServiceError> {
        match self.held.lock().map_err(ServiceError::poisoned)?.plans.get(id) {
            None => Ok(None),
            Some(Ok(plan)) => Ok(Some(plan.clone())),
            Some(Err(why)) => Err(why.clone()),
        }
    }

    fn save(&self, plan: &Plan) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        let text = plan.to_json().map_err(|e| ServiceError::Corrupt(e.to_string()))?;
        write_atomic(&self.path(&plan.id), &text)?;
        self.held.lock().map_err(ServiceError::poisoned)?.plans.insert(plan.id.clone(), Ok(Arc::new(plan.clone())));
        Ok(())
    }

    fn delete(&self, id: &PlanId) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => {
                self.held.lock().map_err(ServiceError::poisoned)?.plans.remove(id);
                sync_dir(&self.dir)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ServiceError::NotFound(format!("plan {id}"))),
            Err(e) => Err(io(&self.path(id), e)),
        }
    }

    fn holds(&self, path: &Path) -> bool {
        path.parent() == Some(self.dir.as_path()) && path.extension().is_some_and(|e| e == "json")
    }

    fn changed(&self, paths: &[PathBuf]) -> Result<Vec<PlanId>, ServiceError> {
        let mut changed = Vec::new();
        for path in paths.iter().filter(|p| self.holds(p)) {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let Ok(id) = PlanId::new(stem) else {
                let mut held = self.held.lock().map_err(ServiceError::poisoned)?;
                if path.exists() {
                    held.misnamed.insert(name);
                } else {
                    held.misnamed.remove(&name);
                }
                continue;
            };
            let now = self.read(path, &id);
            let mut held = self.held.lock().map_err(ServiceError::poisoned)?;
            let before = held.plans.get(&id);
            let same = match (before, &now) {
                (None, None) => true,
                (Some(Ok(old)), Some(Ok(new))) => **old == *new,
                (Some(Err(old)), Some(Err(new))) => old == new,
                _ => false,
            };
            if same {
                continue;
            }
            match now {
                Some(read) => held.plans.insert(id.clone(), read.map(Arc::new)),
                None => held.plans.remove(&id),
            };
            changed.push(id);
        }
        Ok(changed)
    }
}

/// The analysis, kept in `<repo>/.nah/cache/index.sqlite`.
pub fn open_facts(repo_root: &Path, limits: &crate::Limits) -> Result<arch_store::SqliteStore, ServiceError> {
    let nah = repo_root.join(".nah");
    ensure_nah_gitignore(&nah)?;
    Ok(arch_store::SqliteStore::open(&nah.join(CACHE_DIR).join("index.sqlite"), &settings(limits)))
}

/// A store for one run, in a temporary file removed when it's dropped
/// (`meta-harness analyze`, tests).
pub fn temporary_facts(limits: &crate::Limits) -> Result<arch_store::SqliteStore, ServiceError> {
    Ok(arch_store::SqliteStore::temporary(&settings(limits))?)
}

fn settings(limits: &crate::Limits) -> arch_store::Settings {
    let store = &limits.store;
    arch_store::Settings { synchronous: store.synchronous.clone(), cache_kib: store.cache_kib, busy_retry_us: store.busy_retry_us, busy_give_up_ms: store.busy_give_up_ms }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arch_graph::Plan;

    fn plan(id: &str) -> Plan {
        let mut p = Plan::new(id, "Title");
        p.description = "d".into();
        p
    }

    fn exercise_plans(repo: &dyn PlanRepository) {
        assert!(repo.list().unwrap().is_empty());
        repo.save(&plan("b")).unwrap();
        repo.save(&plan("a")).unwrap();
        assert_eq!(repo.list().unwrap(), vec![PlanId::new("a").unwrap(), PlanId::new("b").unwrap()]);
        assert_eq!(*repo.get(&PlanId::new("a").unwrap()).unwrap().unwrap(), plan("a"));
        repo.delete(&PlanId::new("a").unwrap()).unwrap();
        assert!(matches!(repo.delete(&PlanId::new("a").unwrap()), Err(ServiceError::NotFound(_))), "nothing left to delete");
        assert!(repo.get(&PlanId::new("a").unwrap()).unwrap().is_none());
    }

    #[test]
    fn plan_repositories_share_one_contract() {
        exercise_plans(&MemoryPlans::default());
        let dir = tempfile::tempdir().unwrap();
        exercise_plans(&FsPlans::new(dir.path()).unwrap());
    }

    #[test]
    fn file_plans_are_kept_in_memory_and_read_again_when_the_watcher_says() {
        let dir = tempfile::tempdir().unwrap();
        let repo = FsPlans::new(dir.path()).unwrap();
        repo.save(&plan("p")).unwrap();
        let id = PlanId::new("p").unwrap();
        let file = dir.path().join(".nah/plans/p.json");
        let first = repo.get(&id).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &repo.get(&id).unwrap().unwrap()), "unchanged: the same plan, not read again");
        // Someone else (an agent, an editor) rewrites the file: nothing is
        // read until the watcher says so.
        let mut edited = plan("p");
        edited.title = "edited by hand, a longer title".into();
        std::fs::write(&file, edited.to_json().unwrap()).unwrap();
        assert!(Arc::ptr_eq(&first, &repo.get(&id).unwrap().unwrap()), "no stat per request");
        assert!(repo.holds(&file) && !repo.holds(&dir.path().join("src/p.json")));
        assert_eq!(repo.changed(std::slice::from_ref(&file)).unwrap(), std::slice::from_ref(&id));
        let again = repo.get(&id).unwrap().unwrap();
        assert_eq!(again.title, "edited by hand, a longer title");
        // Our own save, seen again by the watcher, changes nothing.
        repo.save(&edited).unwrap();
        let saved = repo.get(&id).unwrap().unwrap();
        assert!(repo.changed(std::slice::from_ref(&file)).unwrap().is_empty());
        assert!(Arc::ptr_eq(&saved, &repo.get(&id).unwrap().unwrap()));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(repo.changed(&[file]).unwrap(), std::slice::from_ref(&id));
        assert!(repo.get(&id).unwrap().is_none(), "a removed file is a removed plan");
        // Plans already there are read once, when opened.
        repo.save(&plan("q")).unwrap();
        let reopened = FsPlans::new(dir.path()).unwrap();
        assert_eq!(reopened.list().unwrap(), [PlanId::new("q").unwrap()]);
    }

    #[test]
    fn file_plans_are_atomic_serialized_and_report_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let repo = std::sync::Arc::new(FsPlans::new(dir.path()).unwrap());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let repo = repo.clone();
                std::thread::spawn(move || {
                    let mut p = plan("shared");
                    p.title = format!("writer {i}");
                    repo.save(&p).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let got = repo.get(&PlanId::new("shared").unwrap()).unwrap().unwrap();
        assert!(got.title.starts_with("writer "), "one complete write wins");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join(".nah/plans")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
        let file = |name: &str| dir.path().join(".nah/plans").join(name);
        std::fs::write(file("broken.json"), "{ not json").unwrap();
        repo.changed(&[file("broken.json")]).unwrap();
        let err = repo.get(&PlanId::new("broken").unwrap()).unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("broken.json")), "{err}");
        std::fs::remove_file(file("broken.json")).unwrap();
        repo.changed(&[file("broken.json")]).unwrap();
        // A plan copied under another name holds the wrong id.
        std::fs::copy(file("shared.json"), file("copy.json")).unwrap();
        repo.changed(&[file("copy.json")]).unwrap();
        let err = repo.get(&PlanId::new("copy").unwrap()).unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("holds plan 'shared', not 'copy'")), "{err}");
        std::fs::remove_file(file("copy.json")).unwrap();
        repo.changed(&[file("copy.json")]).unwrap();
        // A file whose name can't be a plan id is reported, not skipped.
        std::fs::write(file("my plan.json"), "{}").unwrap();
        repo.changed(&[file("my plan.json")]).unwrap();
        let err = repo.list().unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("my plan.json")), "{err}");
        std::fs::remove_file(file("my plan.json")).unwrap();
        repo.changed(&[file("my plan.json")]).unwrap();
        assert_eq!(repo.list().unwrap(), vec![PlanId::new("shared").unwrap()]);
        // Two stores on the same directory (e.g. two processes) don't trip over
        // each other's temp files.
        let other = std::sync::Arc::new(FsPlans::new(dir.path()).unwrap());
        let writers: Vec<_> = (0..8)
            .map(|i| {
                let repo = if i % 2 == 0 { repo.clone() } else { other.clone() };
                std::thread::spawn(move || {
                    let mut p = plan("shared");
                    p.title = format!("store {i}");
                    repo.save(&p).unwrap();
                })
            })
            .collect();
        for h in writers {
            h.join().unwrap();
        }
        assert!(repo.get(&PlanId::new("shared").unwrap()).unwrap().unwrap().title.starts_with("store "));
        let leftovers = std::fs::read_dir(dir.path().join(".nah/plans")).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).count();
        assert_eq!(leftovers, 0);
        assert!(std::fs::read_to_string(dir.path().join(".nah/.gitignore")).unwrap().contains("cache/"));
    }

    #[test]
    fn the_cockpits_state_is_kept_as_written_and_out_of_version_control() {
        let dir = tempfile::tempdir().unwrap();
        // An older .nah/.gitignore gets what it lacks, and keeps the rest.
        std::fs::create_dir_all(dir.path().join(".nah")).unwrap();
        std::fs::write(dir.path().join(".nah/.gitignore"), "# mine\ncache/").unwrap();
        let state = UiStateFile::open(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".nah/.gitignore")).unwrap(), "# mine\ncache/\nui/\n");
        assert_eq!(state.read().unwrap(), None);
        let written = serde_json::json!({ "version": 1, "columns": { "shown": ["chat", "graph"] } });
        state.write(&written).unwrap();
        assert_eq!(state.read().unwrap(), Some(written));
        assert!(state.write(&serde_json::json!([1, 2])).is_err(), "only an object");
        UiStateFile::open(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(".nah/.gitignore")).unwrap(), "# mine\ncache/\nui/\n", "nothing added twice");
    }
}
