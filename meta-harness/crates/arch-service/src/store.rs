//! Storage: plans (the only persisted architecture), and where the facts
//! read from the code are kept between runs (a disposable cache, see
//! `arch-store`). Both live under `<repo>/.nah/`.

use std::collections::BTreeMap;
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

/// Ensures `.nah/.gitignore` keeps caches out of version control (plans stay
/// committed), without touching the repository's own .gitignore.
fn ensure_nah_gitignore(nah: &Path) -> Result<(), ServiceError> {
    let ignore = nah.join(".gitignore");
    if !ignore.exists() {
        write_atomic(&ignore, "# Caches are rebuilt from the code; plans are kept.\ncache/\nreq_status.json\n")?;
    }
    Ok(())
}

/// Plans as `<repo>/.nah/plans/<id>.json`, each kept in memory once read
/// and read again only when its file changes (by size or time), whoever
/// changed it.
pub struct FsPlans {
    dir: PathBuf,
    /// One writer at a time within this process (the server is the writer).
    write: Mutex<()>,
    read: Mutex<BTreeMap<PlanId, Read>>,
}

/// A plan as read, and the file it was read from as it was then.
struct Read {
    stamp: Stamp,
    plan: Arc<Plan>,
}

/// What tells a file changed without reading it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self { len: meta.len(), modified: meta.modified().ok() }
    }
}

impl FsPlans {
    pub fn new(repo_root: &Path) -> Result<Self, ServiceError> {
        let nah = repo_root.join(".nah");
        let dir = nah.join("plans");
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        ensure_nah_gitignore(&nah)?;
        Ok(Self { dir, write: Mutex::new(()), read: Mutex::new(BTreeMap::new()) })
    }

    fn path(&self, id: &PlanId) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

impl PlanRepository for FsPlans {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|e| io(&self.dir, e))? {
            let path = entry.map_err(|e| io(&self.dir, e))?.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            // A plan file whose name can't be a plan id was put there by hand.
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            let id = PlanId::new(stem).map_err(|_| ServiceError::Corrupt(format!("{}: the file name isn't a plan id", path.display())))?;
            ids.push(id);
        }
        ids.sort();
        Ok(ids)
    }

    fn get(&self, id: &PlanId) -> Result<Option<Arc<Plan>>, ServiceError> {
        let path = self.path(id);
        let stamp = match std::fs::metadata(&path) {
            Ok(meta) => Stamp::of(&meta),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.read.lock().map_err(ServiceError::poisoned)?.remove(id);
                return Ok(None);
            }
            Err(e) => return Err(io(&path, e)),
        };
        if let Some(known) = self.read.lock().map_err(ServiceError::poisoned)?.get(id)
            && known.stamp == stamp
        {
            return Ok(Some(known.plan.clone()));
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(&path, e)),
        };
        let plan = Plan::from_json(&text).map_err(|e| ServiceError::Corrupt(format!("{}: {e}", path.display())))?;
        if &plan.id != id {
            return Err(ServiceError::Corrupt(format!("{}: holds plan '{}', not '{id}'", path.display(), plan.id)));
        }
        let plan = Arc::new(plan);
        // Stamped as before the read: a write racing the read is seen next time.
        self.read.lock().map_err(ServiceError::poisoned)?.insert(id.clone(), Read { stamp, plan: plan.clone() });
        Ok(Some(plan))
    }

    fn save(&self, plan: &Plan) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        let text = plan.to_json().map_err(|e| ServiceError::Corrupt(e.to_string()))?;
        write_atomic(&self.path(&plan.id), &text)
    }

    fn delete(&self, id: &PlanId) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => sync_dir(&self.dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ServiceError::NotFound(format!("plan {id}"))),
            Err(e) => Err(io(&self.path(id), e)),
        }
    }
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

    // req: P3.1
    #[test]
    fn plan_repositories_share_one_contract() {
        exercise_plans(&MemoryPlans::default());
        let dir = tempfile::tempdir().unwrap();
        exercise_plans(&FsPlans::new(dir.path()).unwrap());
    }

    // req: M.3
    #[test]
    fn file_plans_are_read_once_and_again_when_their_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let repo = FsPlans::new(dir.path()).unwrap();
        repo.save(&plan("p")).unwrap();
        let id = PlanId::new("p").unwrap();
        let first = repo.get(&id).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &repo.get(&id).unwrap().unwrap()), "unchanged: the same plan, not read again");
        // Someone else (an agent, an editor) rewrites the file.
        let mut edited = plan("p");
        edited.title = "edited by hand, a longer title".into();
        std::fs::write(dir.path().join(".nah/plans/p.json"), edited.to_json().unwrap()).unwrap();
        let again = repo.get(&id).unwrap().unwrap();
        assert_eq!(again.title, "edited by hand, a longer title");
        assert!(!Arc::ptr_eq(&first, &again));
        std::fs::remove_file(dir.path().join(".nah/plans/p.json")).unwrap();
        assert!(repo.get(&id).unwrap().is_none(), "a removed file is a removed plan");
    }

    // req: P3.3
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
        std::fs::write(dir.path().join(".nah/plans/broken.json"), "{ not json").unwrap();
        let err = repo.get(&PlanId::new("broken").unwrap()).unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("broken.json")), "{err}");
        std::fs::remove_file(dir.path().join(".nah/plans/broken.json")).unwrap();
        // A plan copied under another name holds the wrong id.
        std::fs::copy(dir.path().join(".nah/plans/shared.json"), dir.path().join(".nah/plans/copy.json")).unwrap();
        let err = repo.get(&PlanId::new("copy").unwrap()).unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("holds plan 'shared', not 'copy'")), "{err}");
        std::fs::remove_file(dir.path().join(".nah/plans/copy.json")).unwrap();
        // A file whose name can't be a plan id is reported, not skipped.
        std::fs::write(dir.path().join(".nah/plans/my plan.json"), "{}").unwrap();
        let err = repo.list().unwrap_err();
        assert!(matches!(err, ServiceError::Corrupt(ref m) if m.contains("my plan.json")), "{err}");
        std::fs::remove_file(dir.path().join(".nah/plans/my plan.json")).unwrap();
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
}

/// The facts read from the code, kept in `<repo>/.nah/cache/index.sqlite`.
/// The JSON folder older versions kept them in is removed.
pub fn open_facts(repo_root: &Path, limits: &crate::Limits) -> Result<arch_store::SqliteStore, ServiceError> {
    let nah = repo_root.join(".nah");
    ensure_nah_gitignore(&nah)?;
    let old = nah.join("cache").join("index");
    if old.is_dir() {
        std::fs::remove_dir_all(&old).map_err(|e| io(&old, e))?;
    }
    let settings = arch_store::Settings { synchronous: limits.store.synchronous.clone(), cache_kib: limits.store.cache_kib };
    Ok(arch_store::SqliteStore::open(&nah.join("cache").join("index.sqlite"), &settings)?)
}
