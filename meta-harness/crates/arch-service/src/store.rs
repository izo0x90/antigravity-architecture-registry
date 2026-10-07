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
    /// Removes a plan; `NotFound` if there is none.
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
        match self.0.lock().map_err(ServiceError::poisoned)?.remove(id) {
            Some(_) => Ok(()),
            None => Err(ServiceError::NotFound(format!("plan {id}"))),
        }
    }
}

/// In-memory index (embedding and tests).
#[derive(Default)]
pub struct MemoryIndex(Mutex<BTreeMap<RepoPath, FileExtract>>);

impl CodeIndexStore for MemoryIndex {
    fn get(&self, file: &RepoPath, hash: &str) -> Result<Option<FileExtract>, ServiceError> {
        Ok(self.0.lock().map_err(ServiceError::poisoned)?.get(file).filter(|e| &e.file == file && e.content_hash == hash).cloned())
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

    fn get(&self, id: &PlanId) -> Result<Option<Plan>, ServiceError> {
        let path = self.path(id);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let plan = Plan::from_json(&text).map_err(|e| ServiceError::Corrupt(format!("{}: {e}", path.display())))?;
                if &plan.id != id {
                    return Err(ServiceError::Corrupt(format!("{}: holds plan '{}', not '{id}'", path.display(), plan.id)));
                }
                Ok(Some(plan))
            }
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
            Ok(()) => sync_dir(&self.dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(ServiceError::NotFound(format!("plan {id}"))),
            Err(e) => Err(io(&self.path(id), e)),
        }
    }
}

/// Extracts as `<repo>/.nah/cache/index/<encoded path>.json`; the encoding
/// (`_` -> `__`, `/` -> `_s`) is one-to-one, so no two files share an entry.
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
        self.dir.join(format!("{}.json", encode(file.as_str())))
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
        Ok(self.read(&self.path(file))?.filter(|e| &e.file == file && e.content_hash == hash))
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
                // Only an entry under its file's own name counts: one left
                // by an older naming (or moved by hand) would put an old
                // copy of the file's code beside the current one.
                if path != self.path(&extract.file) {
                    let _guard = self.write.lock().map_err(ServiceError::poisoned)?;
                    std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
                    continue;
                }
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

/// A repo path as one file name: every `_` doubles, every `/` becomes `_s`.
/// Each `_` in the result starts a two-character token, so decoding is
/// unambiguous and distinct paths never collide.
/// Long paths would exceed file-name limits: those use `_h` plus a hash of
/// the path (`_h` never occurs in an encoded path; `get` checks the file).
fn encode(path: &str) -> String {
    const MAX: usize = 180;
    let mut out = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        match c {
            '_' => out.push_str("__"),
            '/' => out.push_str("_s"),
            c => out.push(c),
        }
    }
    if out.len() > MAX { format!("_h{}", arch_index::content_hash(path)) } else { out }
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

    fn extract(file: &str, hash: &str) -> FileExtract {
        FileExtract {
            schema_version: arch_graph::SCHEMA_VERSION,
            file: RepoPath::new(file).unwrap(),
            language: "rust".into(),
            content_hash: hash.into(),
            pack_fingerprint: "p".into(),
            module: "m".into(),
            root_kind: "source_file".into(),
            lines: 1,
            bytes: 1,
            definitions: vec![],
            calls: vec![],
            imports: vec![],
            aliases: vec![],
            placeholders: vec![],
            macro_uses: vec![],
            macro_defs: vec![],
            flows: vec![],
        }
    }

    fn exercise_index(index: &dyn CodeIndexStore) {
        // Paths past file-name limits still round-trip (hashed names).
        let deep = format!("src/{}/x.rs", ["very_long_directory_name"; 12].join("/"));
        index.put(&extract(&deep, "hd")).unwrap();
        assert!(index.get(&RepoPath::new(&deep).unwrap(), "hd").unwrap().is_some());
        index.remove(&RepoPath::new(&deep).unwrap()).unwrap();
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
        let path = dir.path().join(".nah/cache/index/src_sa.rs.json");
        assert!(path.is_file());
        let reopened = FsIndex::new(dir.path()).unwrap();
        assert!(reopened.get(&RepoPath::new("src/a.rs").unwrap(), "h1").unwrap().is_some(), "survives a restart");
        std::fs::write(&path, std::fs::read_to_string(&path).unwrap().replace("\"schema_version\": 1", "\"schema_version\": 0")).unwrap();
        assert!(reopened.get(&RepoPath::new("src/a.rs").unwrap(), "h1").unwrap().is_none(), "old format is a miss, not an error");
        // Paths that a lossy encoding would merge keep separate entries.
        index.put(&extract("src/a__b.rs", "h1")).unwrap();
        index.put(&extract("src/a/b.rs", "h2")).unwrap();
        index.put(&extract("src/a_/b.rs", "h3")).unwrap();
        index.put(&extract("src/a/_b.rs", "h4")).unwrap();
        for (file, hash) in [("src/a__b.rs", "h1"), ("src/a/b.rs", "h2"), ("src/a_/b.rs", "h3"), ("src/a/_b.rs", "h4")] {
            let got = index.get(&RepoPath::new(file).unwrap(), hash).unwrap();
            assert_eq!(got.map(|e| e.file.as_str().to_owned()), Some(file.to_owned()), "{file}");
        }
        assert_eq!(index.all().unwrap().len(), 4, "src/a.rs's stale entry is skipped, the four others stay");
        // An entry that belongs to another file (moved by hand) is a miss.
        std::fs::copy(dir.path().join(".nah/cache/index/src_sa____b.rs.json"), dir.path().join(".nah/cache/index/src_sc.rs.json")).unwrap();
        assert!(index.get(&RepoPath::new("src/c.rs").unwrap(), "h1").unwrap().is_none());
        // Nor does it come back as a second copy of a__b.rs: it's dropped.
        assert_eq!(index.all().unwrap().len(), 4, "no duplicate from a misnamed entry");
        assert!(!dir.path().join(".nah/cache/index/src_sc.rs.json").exists());
        // The same for one left by an older naming (`/` as `__`).
        std::fs::copy(dir.path().join(".nah/cache/index/src_sa_sb.rs.json"), dir.path().join(".nah/cache/index/src__a__b.rs.json")).unwrap();
        assert_eq!(index.all().unwrap().iter().filter(|e| e.file.as_str() == "src/a/b.rs").count(), 1);
    }
}
