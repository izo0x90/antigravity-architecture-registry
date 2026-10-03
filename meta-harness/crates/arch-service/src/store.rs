//! Storage: plans (the only persisted architecture) and the code index (a
//! disposable cache of per-file extracts). Traits are storage-agnostic; the
//! file implementations live under `<repo>/.nah/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use arch_graph::{Plan, PlanId, RepoPath};
use arch_index::FileExtract;

use crate::ServiceError;

pub trait PlanRepository: Send + Sync {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError>;
    fn get(&self, id: &PlanId) -> Result<Option<Plan>, ServiceError>;
    /// Saves the whole plan atomically.
    fn save(&self, plan: &Plan) -> Result<(), ServiceError>;
    fn delete(&self, id: &PlanId) -> Result<(), ServiceError>;
}

/// The observed code, one extract per source file, keyed by content hash.
pub trait CodeIndexStore: Send + Sync {
    /// The cached extract for `file`, if it was made from content with `hash`.
    fn get(&self, file: &RepoPath, hash: &str) -> Result<Option<FileExtract>, ServiceError>;
    fn put(&self, extract: &FileExtract) -> Result<(), ServiceError>;
    fn remove(&self, file: &RepoPath) -> Result<(), ServiceError>;
    /// Every cached extract.
    fn all(&self) -> Result<Vec<FileExtract>, ServiceError>;
    fn clear(&self) -> Result<(), ServiceError>;
}

/// In-memory plans (embedding and tests).
#[derive(Default)]
pub struct MemoryPlans(Mutex<BTreeMap<PlanId, Plan>>);

impl PlanRepository for MemoryPlans {
    fn list(&self) -> Result<Vec<PlanId>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.keys().cloned().collect())
    }
    fn get(&self, id: &PlanId) -> Result<Option<Plan>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.get(id).cloned())
    }
    fn save(&self, plan: &Plan) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.insert(plan.id.clone(), plan.clone());
        Ok(())
    }
    fn delete(&self, id: &PlanId) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.remove(id);
        Ok(())
    }
}

/// In-memory index (embedding and tests).
#[derive(Default)]
pub struct MemoryIndex(Mutex<BTreeMap<RepoPath, FileExtract>>);

impl CodeIndexStore for MemoryIndex {
    fn get(&self, file: &RepoPath, hash: &str) -> Result<Option<FileExtract>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.get(file).filter(|e| e.content_hash == hash).cloned())
    }
    fn put(&self, extract: &FileExtract) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.insert(extract.file.clone(), extract.clone());
        Ok(())
    }
    fn remove(&self, file: &RepoPath) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.remove(file);
        Ok(())
    }
    fn all(&self) -> Result<Vec<FileExtract>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.values().cloned().collect())
    }
    fn clear(&self) -> Result<(), ServiceError> {
        self.0.lock().map_err(ServiceError::poisoned)?.clear();
        Ok(())
    }
}

fn io(path: &Path, err: std::io::Error) -> ServiceError {
    ServiceError::Io(format!("{}: {err}", path.display()))
}

/// Writes via a temp file and rename, so readers never see a partial file.
fn write_atomic(path: &Path, text: &str) -> Result<(), ServiceError> {
    let dir = path.parent().ok_or_else(|| ServiceError::Io(format!("{} has no parent", path.display())))?;
    std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    let tmp = dir.join(format!(".{}.tmp", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    std::fs::write(&tmp, text).map_err(|e| io(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| io(path, e))
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

/// Plans as `<repo>/.nah/plans/<id>.json`.
pub struct FsPlans {
    dir: PathBuf,
    /// One writer at a time within this process (the server is the writer).
    write: Mutex<()>,
}

impl FsPlans {
    pub fn new(repo_root: &Path) -> Result<Self, ServiceError> {
        let nah = repo_root.join(".nah");
        let dir = nah.join("plans");
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        ensure_nah_gitignore(&nah)?;
        Ok(Self { dir, write: Mutex::new(()) })
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
            if path.extension().is_some_and(|e| e == "json")
                && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
                && let Ok(id) = PlanId::new(stem)
            {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn get(&self, id: &PlanId) -> Result<Option<Plan>, ServiceError> {
        let path = self.path(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => Plan::from_json(&text).map(Some).map_err(|e| ServiceError::Corrupt(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(&path, e)),
        }
    }

    fn save(&self, plan: &Plan) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        let text = plan.to_json().map_err(|e| ServiceError::Corrupt(e.to_string()))?;
        write_atomic(&self.path(&plan.id), &text)
    }

    fn delete(&self, id: &PlanId) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        match std::fs::remove_file(self.path(id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(&self.path(id), e)),
        }
    }
}

/// Extracts as `<repo>/.nah/cache/index/<path with / as __>.json`.
pub struct FsIndex {
    dir: PathBuf,
    write: Mutex<()>,
}

impl FsIndex {
    pub fn new(repo_root: &Path) -> Result<Self, ServiceError> {
        let nah = repo_root.join(".nah");
        let dir = nah.join("cache").join("index");
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        ensure_nah_gitignore(&nah)?;
        Ok(Self { dir, write: Mutex::new(()) })
    }

    fn path(&self, file: &RepoPath) -> PathBuf {
        self.dir.join(format!("{}.json", file.as_str().replace('/', "__")))
    }

    fn read(&self, path: &Path) -> Result<Option<FileExtract>, ServiceError> {
        match std::fs::read_to_string(path) {
            // A cache entry that no longer parses (older format) is a miss.
            Ok(text) => Ok(FileExtract::from_json(&text).ok()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(path, e)),
        }
    }
}

impl CodeIndexStore for FsIndex {
    fn get(&self, file: &RepoPath, hash: &str) -> Result<Option<FileExtract>, ServiceError> {
        Ok(self.read(&self.path(file))?.filter(|e| e.content_hash == hash))
    }

    fn put(&self, extract: &FileExtract) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        let text = extract.to_json().map_err(|e| ServiceError::Corrupt(e.to_string()))?;
        write_atomic(&self.path(&extract.file), &text)
    }

    fn remove(&self, file: &RepoPath) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        match std::fs::remove_file(self.path(file)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(&self.path(file), e)),
        }
    }

    fn all(&self) -> Result<Vec<FileExtract>, ServiceError> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|e| io(&self.dir, e))? {
            let path = entry.map_err(|e| io(&self.dir, e))?.path();
            if path.extension().is_some_and(|e| e == "json")
                && let Some(extract) = self.read(&path)?
            {
                out.push(extract);
            }
        }
        out.sort_by(|a, b| a.file.cmp(&b.file));
        Ok(out)
    }

    fn clear(&self) -> Result<(), ServiceError> {
        let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
        std::fs::remove_dir_all(&self.dir).map_err(|e| io(&self.dir, e))?;
        std::fs::create_dir_all(&self.dir).map_err(|e| io(&self.dir, e))
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
        assert_eq!(repo.get(&PlanId::new("a").unwrap()).unwrap().unwrap(), plan("a"));
        repo.delete(&PlanId::new("a").unwrap()).unwrap();
        repo.delete(&PlanId::new("a").unwrap()).unwrap();
        assert!(repo.get(&PlanId::new("a").unwrap()).unwrap().is_none());
    }

    // req: P3.1
    #[test]
    fn plan_repositories_share_one_contract() {
        exercise_plans(&MemoryPlans::default());
        let dir = tempfile::tempdir().unwrap();
        exercise_plans(&FsPlans::new(dir.path()).unwrap());
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
        assert!(std::fs::read_to_string(dir.path().join(".nah/.gitignore")).unwrap().contains("cache/"));
    }

    fn extract(file: &str, hash: &str) -> FileExtract {
        FileExtract {
            schema_version: arch_graph::SCHEMA_VERSION,
            file: RepoPath::new(file).unwrap(),
            language: "rust".into(),
            content_hash: hash.into(),
            module: "m".into(),
            lines: 1,
            bytes: 1,
            definitions: vec![],
            calls: vec![],
            imports: vec![],
            placeholders: vec![],
        }
    }

    fn exercise_index(index: &dyn CodeIndexStore) {
        let f = RepoPath::new("src/a.rs").unwrap();
        index.put(&extract("src/a.rs", "h1")).unwrap();
        assert!(index.get(&f, "h1").unwrap().is_some(), "hit by hash");
        assert!(index.get(&f, "h2").unwrap().is_none(), "content changed: miss");
        index.put(&extract("src/b/c.rs", "h3")).unwrap();
        assert_eq!(index.all().unwrap().len(), 2);
        index.remove(&f).unwrap();
        assert_eq!(index.all().unwrap().len(), 1);
        index.clear().unwrap();
        assert!(index.all().unwrap().is_empty());
    }

    // req: P3.2
    #[test]
    fn index_stores_share_one_contract() {
        exercise_index(&MemoryIndex::default());
        let dir = tempfile::tempdir().unwrap();
        exercise_index(&FsIndex::new(dir.path()).unwrap());
    }

    // req: P3.4
    #[test]
    fn file_index_persists_under_nah_cache_and_skips_stale_formats() {
        let dir = tempfile::tempdir().unwrap();
        let index = FsIndex::new(dir.path()).unwrap();
        index.put(&extract("src/a.rs", "h1")).unwrap();
        let path = dir.path().join(".nah/cache/index/src__a.rs.json");
        assert!(path.is_file());
        let reopened = FsIndex::new(dir.path()).unwrap();
        assert!(reopened.get(&RepoPath::new("src/a.rs").unwrap(), "h1").unwrap().is_some(), "survives a restart");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("\"schema_version\": 1", "\"schema_version\": 0")).unwrap();
        assert!(reopened.get(&RepoPath::new("src/a.rs").unwrap(), "h1").unwrap().is_none(), "old format is a miss, not an error");
    }
}
