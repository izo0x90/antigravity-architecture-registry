//! The one writer: a single pipeline
//! thread owns every write, every link and every swap of what readers see,
//! and the generation counter. Ingests (and, later, the servers' answers)
//! are jobs sent to it, so they can't race; whoever sent one waits for its
//! answer. Reads never go through it: they take the published snapshot,
//! and the store's own read connections.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Instant, SystemTime};

use arch_graph::{ArchEvent, ChangeReason, Graph, IndexState, IndexStatus, IndexSummary, RepoPath, TypeSystem};
use arch_store::{SqliteStore, Stamp};
use code_analyzer::rows::{FileRow, read_version};
use code_analyzer::{AmbiguousCall, Change, CodeRules, ExternalCall, FileExtract, Model, PackSet, UnverifiedFlow, Written, content_hash, extract_file};
use rayon::prelude::*;

use crate::ServiceError;
use crate::perf::Timing;
use crate::service::IngestReport;

pub(crate) type Listener = Arc<dyn Fn(&ArchEvent) + Send + Sync>;

/// The code as last linked: read-only once published. Each change makes a
/// new one; readers hold the one they started with.
#[derive(Default)]
pub(crate) struct Snapshot {
    pub graph: Arc<Graph>,
    /// Every file's rows (shared with the writer: nothing is copied).
    pub files: code_analyzer::Files,
    pub ambiguous: Vec<AmbiguousCall>,
    pub unproven: Vec<AmbiguousCall>,
    pub external: Vec<ExternalCall>,
    /// Data handed from one call to another that types couldn't confirm.
    pub unverified: Vec<UnverifiedFlow>,
    /// Counts up with every publish (a plan's overlay is built for one).
    pub generation: u64,
}

/// What every part of the service shares with the pipeline thread.
pub(crate) struct Core {
    pub root: PathBuf,
    pub packs: PackSet,
    pub types: TypeSystem,
    pub rules: CodeRules,
    pub limits: crate::Limits,
    /// What ingestion never reads (the one definition; it reads an ignore
    /// file again when it changes).
    pub ignores: Arc<code_analyzer::Ignores>,
    pub store: SqliteStore,
    /// What readers see (shared with the servers' confirmer, which reads it).
    pub snapshot: Arc<RwLock<Arc<Snapshot>>>,
    /// Snapshots replaced while a reader may still hold them: kept here so
    /// the last one to let go is the writer, which frees them (a reader
    /// would otherwise pay for freeing a whole graph when it finishes).
    pub retired: Mutex<Vec<Arc<Snapshot>>>,
    pub status: RwLock<IndexStatus>,
    pub listeners: RwLock<Vec<Listener>>,
    /// The last answers' timings.
    pub recent: Mutex<crate::perf::Recent>,
}

impl Core {
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.snapshot.read().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn emit(&self, event: ArchEvent) {
        let listeners: Vec<Listener> = self.listeners.read().map(|l| l.clone()).unwrap_or_default();
        for listener in listeners {
            listener(&event);
        }
    }

    /// Changes the status and tells clients.
    pub fn set_status(&self, change: impl FnOnce(&mut IndexStatus)) {
        let status = self.change_status(change);
        self.emit(ArchEvent::Index { status: Box::new(status) });
    }

    /// Changes the status without telling anyone (yet).
    fn change_status(&self, change: impl FnOnce(&mut IndexStatus)) -> IndexStatus {
        let mut s = self.status.write().unwrap_or_else(PoisonError::into_inner);
        change(&mut s);
        s.clone()
    }

    pub fn timed<T>(&self, what: &'static str, asked: String, answer: impl FnOnce(&mut Timing) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        let mut t = Timing::start(what, asked);
        let out = answer(&mut t);
        let timed = t.done(self.limits.slow_request_ms, crate::service::now_rfc3339());
        if timed.slow {
            let stages: Vec<String> = timed.stages.iter().map(|s| format!("{} {:.1} ms", s.stage, s.ms)).collect();
            tracing::warn!(ms = timed.ms as u64, size = ?timed.size, stages = %stages.join(", "), "slow {}: {}", timed.what, timed.asked);
        }
        self.recent.lock().unwrap_or_else(PoisonError::into_inner).push(timed, self.limits.perf_kept);
        out
    }

    /// What linking depends on beyond the files: the types, the language
    /// packs and the linker's version. Rows linked under others are linked
    /// again.
    pub fn fingerprints(&self) -> String {
        let packs: Vec<&str> = self.packs.packs.iter().map(|p| p.fingerprint.as_str()).collect();
        content_hash(&format!("{}|{}|{}", code_analyzer::LINKER_VERSION, serde_json::to_string(&self.types).unwrap_or_default(), packs.join(",")))
    }

    /// `scope` as a repo-relative prefix ("" for the whole repo).
    pub fn scope_prefix(&self, scope: &Path) -> String {
        let absolute = if scope.is_absolute() { scope.to_path_buf() } else { self.root.join(scope) };
        let relative = absolute.strip_prefix(&self.root).unwrap_or(&absolute).to_string_lossy().into_owned();
        RepoPath::new(&relative).map(|p| p.as_str().to_owned()).unwrap_or_default()
    }

    pub fn under(file: &RepoPath, prefix: &str) -> bool {
        prefix.is_empty() || file.as_str() == prefix || file.as_str().starts_with(&format!("{prefix}/"))
    }

    /// Makes `model` what readers see. The old snapshot is let go after the
    /// lock is, and freed here once no reader holds it (freeing it takes
    /// time readers mustn't spend).
    pub fn publish(&self, model: &Model, t: &mut Timing) {
        let keeper = model.keeper();
        let mut next = t.stage("graph", || Snapshot {
            graph: Arc::new(keeper.graph()),
            files: model.shared(),
            ambiguous: keeper.ambiguous(),
            unproven: keeper.unproven(),
            external: keeper.external(),
            unverified: keeper.unverified(),
            generation: 0,
        });
        let old = {
            let mut slot = self.snapshot.write().unwrap_or_else(PoisonError::into_inner);
            next.generation = slot.generation + 1;
            std::mem::replace(&mut *slot, Arc::new(next))
        };
        self.retire(old);
    }

    /// Keeps `old` until no reader holds it or its graph, and frees the
    /// ones no reader holds any more.
    pub fn retire(&self, old: Arc<Snapshot>) {
        let freed: Vec<Arc<Snapshot>> = {
            let mut retired = self.retired.lock().unwrap_or_else(PoisonError::into_inner);
            retired.push(old);
            let (held, free): (Vec<_>, Vec<_>) = std::mem::take(&mut *retired).into_iter().partition(|s| Arc::strong_count(s) > 1 || Arc::strong_count(&s.graph) > 1);
            *retired = held;
            free
        };
        drop(freed);
    }

    /// Writes what a change wrote, in one transaction.
    pub fn write(&self, written: &Written, t: &mut Timing) -> Result<(), ServiceError> {
        if written.writes.is_empty() && written.removed.is_empty() {
            return Ok(());
        }
        on_the_writer();
        let fingerprints = self.fingerprints();
        t.stage("store", || self.store.commit(&written.writes, &written.removed, Some(&fingerprints)))?;
        Ok(())
    }

    /// Replaces every row with a full read's, in one transaction.
    pub fn replace_all(&self, written: &Written, t: &mut Timing) -> Result<(), ServiceError> {
        on_the_writer();
        t.stage("store", || self.store.replace_all(&written.writes, &self.fingerprints()))?;
        Ok(())
    }

    /// Keeps the languages' servers' states.
    pub fn keep_servers(&self, statuses: &[arch_graph::ServerStatus], at: &str) -> Result<(), ServiceError> {
        on_the_writer();
        Ok(self.store.put_servers(statuses, at)?)
    }
}

pub(crate) type Job = Box<dyn FnOnce(&Core, &mut Model) + Send>;

/// The pipeline thread's name (its pool's threads add a number).
const PIPELINE_THREAD: &str = "nah-pipeline";

/// Every store write is the pipeline thread's (RB.7): one anywhere else is
/// a bug, caught where tests run.
fn on_the_writer() {
    debug_assert!(std::thread::current().name().is_some_and(|n| n.starts_with(PIPELINE_THREAD)), "a store write off the pipeline thread");
}

/// Where jobs for the pipeline thread go, once it has started (others, like
/// the servers' answers, send theirs here too).
/// None once the pipeline has stopped.
pub(crate) type Jobs = Arc<OnceLock<Mutex<Option<Sender<Job>>>>>;

/// Sends `job` to the pipeline thread without waiting; false when the
/// thread hasn't started (nothing has changed yet) or has stopped.
pub(crate) fn send(jobs: &Jobs, job: Job) -> bool {
    jobs.get().is_some_and(|tx| tx.lock().unwrap_or_else(PoisonError::into_inner).as_ref().is_some_and(|tx| tx.send(job).is_ok()))
}

/// The pipeline thread: started at the first use, after the service is
/// built (its settings no longer change), and stopped with the service. It
/// opens the kept rows first, so every write, link and swap is its own.
/// Dropping it stops the thread and waits for it, so what the thread holds
/// (the store) is let go before the service's drop returns.
#[derive(Default)]
pub(crate) struct Pipeline {
    pub jobs: Jobs,
    /// How opening the kept rows went (set once, before any job runs).
    opened: Arc<OnceLock<Result<(), String>>>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(tx) = self.jobs.get() {
            tx.lock().unwrap_or_else(PoisonError::into_inner).take();
        }
        if let Some(thread) = self.thread.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = thread.join();
        }
    }
}

impl Pipeline {
    /// Waits until the kept rows are open and published; why not, if they
    /// couldn't be (the thread then starts from no rows).
    pub fn open(&self, core: &Arc<Core>) -> Result<(), ServiceError> {
        if self.opened.get().is_none() {
            self.run(core, |_, _| ())?;
        }
        match self.opened.get() {
            Some(Err(e)) => Err(ServiceError::Corrupt(e.clone())),
            _ => Ok(()),
        }
    }

    /// Runs `job` on the pipeline thread and waits for its answer.
    pub fn run<T: Send + 'static>(&self, core: &Arc<Core>, job: impl FnOnce(&Core, &mut Model) -> T + Send + 'static) -> Result<T, ServiceError> {
        let jobs = self.jobs.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let core = core.clone();
            let opened = self.opened.clone();
            let spawned = std::thread::Builder::new().name(PIPELINE_THREAD.into()).spawn(move || {
                // Its parallel work takes a share of the cores, so reads (and
                // everything else) never wait for one.
                let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
                let threads = ((cores as f64 * core.limits.pipeline.cores_share) as usize).max(1);
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("{PIPELINE_THREAD}-{i}")).build();
                let on_pool = |work: &mut (dyn FnMut() + Send)| match &pool {
                    Ok(pool) => pool.install(work),
                    Err(_) => work(),
                };
                let mut model = Model::default();
                on_pool(&mut || {
                    let _ = opened.set(match core.timed("open", "the kept rows".to_owned(), |t| start(&core, t)) {
                        Ok(m) => {
                            model = m;
                            Ok(())
                        }
                        Err(e) => Err(e.to_string()),
                    });
                });
                while let Ok(job) = rx.recv() {
                    let mut job = Some(job);
                    let mut panicked = false;
                    on_pool(&mut || {
                        let job = job.take().expect("run once");
                        panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(&core, &mut model))).is_err();
                    });
                    // A job that panicked may have left the model half
                    // changed: it's opened again from the kept rows (every
                    // write is one transaction, so they're whole).
                    if panicked {
                        tracing::error!("a pipeline job panicked; opening the kept rows again");
                        on_pool(&mut || match core.timed("open", "the kept rows, after a change failed".to_owned(), |t| start(&core, t)) {
                            Ok(m) => model = m,
                            Err(e) => {
                                tracing::error!("couldn't open the kept rows again: {e}");
                                model = Model::default();
                            }
                        });
                    }
                }
            });
            match spawned {
                Ok(thread) => *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread),
                Err(e) => tracing::error!("couldn't start the pipeline thread: {e}"),
            }
            Mutex::new(Some(tx))
        });
        let (reply, answer) = channel();
        let job: Job = Box::new(move |core, model| {
            let _ = reply.send(job(core, model));
        });
        let stopped = || ServiceError::Corrupt("the pipeline thread stopped".into());
        jobs.lock().unwrap_or_else(PoisonError::into_inner).as_ref().ok_or_else(stopped)?.send(job).map_err(|_| stopped())?;
        answer.recv().map_err(|_| ServiceError::Corrupt("the change failed (see the log); the kept rows were opened again".into()))
    }
}

/// Opens the store's rows: the model as kept, linked again only when what
/// linking depends on changed; then published. No file is read.
pub(crate) fn start(core: &Core, t: &mut Timing) -> Result<Model, ServiceError> {
    let loaded = t.stage("load", || core.store.load())?;
    let fresh = loaded.fingerprints.as_deref() != Some(core.fingerprints().as_str());
    let mut model = t.stage("rows", || Model::load(&core.packs, &core.types, loaded.files));
    if fresh && !model.is_empty() {
        let written = t.stage("link", || model.relink_all(&core.packs, &core.types));
        core.write(&written, t)?;
    }
    core.publish(&model, t);
    t.size(model.len(), "files");
    Ok(model)
}

/// What reading one file came to.
enum Read {
    /// Not a file any pack reads.
    Skipped,
    /// Unchanged by size and time: not read.
    Unread,
    /// Read, and found the same.
    Same,
    /// The same content, a new stamp.
    Touched(FileRow),
    Changed(FileRow, Box<FileExtract>),
}

/// Brings the rows up to date for `scopes` (paths inside the repo): files
/// whose size and time match what was kept aren't read; the rest are read
/// in parallel and compared by content; files under a scope that are gone
/// leave. Then what changed is linked, written and published. A run someone
/// asked for (`asked`) tells clients as it starts and ends; one the watcher
/// started says something only once it finds a change.
pub(crate) fn ingest(core: &Core, model: &mut Model, scopes: &[PathBuf], fresh: bool, asked: bool) -> Result<IngestReport, ServiceError> {
    // A full re-index starts from no rows (the store's are replaced in one
    // transaction when it's done), keeping the servers' answers, unconfirmed,
    // for files that read the same.
    if fresh {
        model.reset_keeping_answers();
    }
    let what = scopes.iter().map(|s| core.scope_prefix(s)).map(|p| if p.is_empty() { "the repo".to_owned() } else { p }).collect::<Vec<_>>().join(", ");
    let out = core.timed("ingest", what, |t| ingest_timed(core, model, t, scopes, asked, fresh));
    if let Err(e) = &out {
        core.set_status(|s| {
            s.state = IndexState::Failed;
            s.error = Some(e.to_string());
        });
    }
    out
}

fn ingest_timed(core: &Core, model: &mut Model, t: &mut Timing, scopes: &[PathBuf], asked: bool, fresh: bool) -> Result<IngestReport, ServiceError> {
    let started = Instant::now();
    let mut report = IngestReport::default();
    // Every file first, so progress has a total.
    let found = t.stage("find files", || -> Result<Vec<_>, ServiceError> {
        let mut found = Vec::new();
        for scope in scopes {
            let files = match scope.exists() {
                true => code_analyzer::walk_with(&core.ignores, &core.root, scope, &core.packs).map_err(|e| ServiceError::Index(e.to_string()))?,
                false => Vec::new(),
            };
            found.push((scope.clone(), files));
        }
        Ok(found)
    })?;
    let total = found.iter().map(|(_, f)| f.len()).sum();
    let start = |s: &mut IndexStatus| {
        s.state = IndexState::Indexing;
        s.done = 0;
        s.total = total;
        s.error = None;
    };
    let before = core.status.read().unwrap_or_else(PoisonError::into_inner).clone();
    let announced = AtomicBool::new(asked);
    match asked {
        true => core.set_status(start),
        false => {
            core.change_status(start);
        }
    }
    let change = read_files(core, model, t, &found, &mut report, &announced)?;
    let written = t.stage("link", || model.apply(&core.packs, &core.types, change));
    match fresh {
        true => core.replace_all(&written, t)?,
        false => core.write(&written, t)?,
    }
    // A touch (same content, new time) only updates its stamp: nothing is
    // linked again, nothing is published, and nobody is told.
    let graph_changed = written.graph_changed();
    if graph_changed {
        core.publish(model, t);
    }
    let snapshot = core.snapshot();
    (report.components, report.relations) = snapshot.graph.len();
    report.ambiguous = snapshot.ambiguous.len();
    report.unproven = snapshot.unproven.len();
    report.external = snapshot.external.len();
    report.millis = started.elapsed().as_millis() as u64;
    drop(snapshot);
    t.size(report.files, "files");
    report.linked = written.linked.iter().cloned().collect();
    // What was read is fresh again.
    let read: Vec<String> = scopes.iter().map(|s| core.scope_prefix(s)).collect();
    let after = core.change_status(|s| {
        s.state = IndexState::Idle;
        s.done = total;
        s.last = Some(IndexSummary {
            millis: report.millis,
            files: report.files,
            parsed: report.parsed,
            components: report.components,
            relations: report.relations,
            unresolved: report.ambiguous + report.unproven,
            at: crate::service::now_rfc3339(),
        });
        s.stale.retain(|stale| !read.iter().any(|scope| scope.is_empty() || stale == scope || stale.starts_with(&format!("{scope}/"))));
    });
    if graph_changed || announced.load(Ordering::Relaxed) || after.stale != before.stale || before.error.is_some() {
        core.emit(ArchEvent::Index { status: Box::new(after) });
    }
    if graph_changed {
        core.emit(ArchEvent::Changed { reason: ChangeReason::Ingested, plans: Vec::new(), components: Vec::new() });
    }
    Ok(report)
}

/// Reads the files found under each scope as needed; what changed.
fn read_files(core: &Core, model: &Model, t: &mut Timing, found: &[(PathBuf, Vec<code_analyzer::SourceFile>)], report: &mut IngestReport, announced: &AtomicBool) -> Result<Change, ServiceError> {
    let mut unique: BTreeMap<&RepoPath, &code_analyzer::SourceFile> = BTreeMap::new();
    for (_, files) in found {
        report.files += files.len();
        unique.extend(files.iter().map(|f| (&f.path, f)));
    }
    let recent = std::time::Duration::from_millis(core.limits.store.recent_change_ms);
    let progress_every = std::time::Duration::from_millis(core.limits.pipeline.progress_ms);
    let trusted_before = SystemTime::now().checked_sub(recent).unwrap_or(SystemTime::UNIX_EPOCH).duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
    let done = AtomicUsize::new(0);
    let last = Mutex::new(Instant::now());
    let read: Vec<Result<Read, ServiceError>> = t.stage("read", || {
        unique
            .values()
            .collect::<Vec<_>>()
            .par_iter()
            .map(|file| {
                let read = read_file(core, model, file, trusted_before);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                if let Ok(mut at) = last.try_lock()
                    && at.elapsed() >= progress_every
                {
                    *at = Instant::now();
                    announced.store(true, Ordering::Relaxed);
                    core.set_status(|s| s.done = s.done.max(n));
                }
                read
            })
            .collect()
    });
    let mut change = Change::default();
    for r in read {
        match r? {
            Read::Skipped => {}
            Read::Unread => report.cached += 1,
            Read::Same => {
                report.cached += 1;
                report.read += 1;
            }
            Read::Touched(file) => {
                report.cached += 1;
                report.read += 1;
                change.touched.push(file);
            }
            Read::Changed(file, extract) => {
                // Reading changed code: clients hear of the run now.
                if !announced.swap(true, Ordering::Relaxed) {
                    core.set_status(|_| {});
                }
                report.read += 1;
                report.parsed += 1;
                report.changed.push(file.path.clone());
                if model.get(&file.path).is_none() {
                    report.created.push(file.path.clone());
                }
                change.read.push((file, *extract));
            }
        }
    }
    // Files under a scope that weren't found there leave.
    let prefixes: Vec<String> = found.iter().map(|(scope, _)| core.scope_prefix(scope)).collect();
    for rows in model.files().filter(|f| !unique.contains_key(&f.file.path) && prefixes.iter().any(|p| Core::under(&f.file.path, p))) {
        report.removed += 1;
        report.changed.push(rows.file.path.clone());
        change.removed.push(rows.file.path.clone());
    }
    report.changed.sort();
    report.created.sort();
    Ok(change)
}

/// One file: skipped if its size and time match what was kept (and that
/// time was trusted), else read and compared by content.
fn read_file(core: &Core, model: &Model, file: &code_analyzer::SourceFile, trusted_before: i64) -> Result<Read, ServiceError> {
    let Some(pack) = core.packs.for_path(&file.absolute) else { return Ok(Read::Skipped) };
    let io = |e: std::io::Error| ServiceError::Io(format!("{}: {e}", file.path));
    let mut stamp = Stamp::of(&std::fs::metadata(&file.absolute).map_err(io)?);
    // Changed too recently to tell a later change in the same tick of the clock.
    if stamp.modified > trusted_before {
        stamp.modified = 0;
    }
    let version = read_version(&pack.fingerprint, core.rules.fingerprint(pack.name()));
    // Kept rows count only if read the same way: the same reader, pack, code
    // rules and module.
    let kept = model.get(&file.path).filter(|rows| rows.file.read_version == version && rows.module().is_some_and(|m| m.detail.symbol == file.module));
    if let Some(rows) = kept
        && rows.file.modified != 0
        && rows.file.size == stamp.size
        && rows.file.modified == stamp.modified
    {
        return Ok(Read::Unread);
    }
    let content = std::fs::read_to_string(&file.absolute).map_err(io)?;
    let hash = content_hash(&content);
    let row = FileRow { path: file.path.clone(), language: pack.name().to_owned(), size: stamp.size, modified: stamp.modified, content_hash: hash.clone(), read_version: version };
    Ok(match kept {
        Some(rows) if rows.file.content_hash == hash => match rows.file == row {
            true => Read::Same,
            false => Read::Touched(row),
        },
        _ => Read::Changed(row, Box::new(extract_file(pack, &core.rules, file.path.clone(), file.module.clone(), &content))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArchitectureService, MemoryPlans};

    /// A project of two Rust files, with the packs shipped in this repo.
    fn service() -> (tempfile::TempDir, ArchitectureService) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "mod a;\npub fn run() { a::go(); }\n").unwrap();
        std::fs::write(dir.path().join("src/a.rs"), "pub fn go() {}\n").unwrap();
        let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
        let places = nah_packs::Places::isolated(&dir.path().join(".packs"));
        let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&shipped).unwrap(), &places).unwrap();
        let packs = PackSet::from_catalog(&catalog).unwrap();
        let store = crate::temporary_facts(&crate::Limits::defaults().unwrap()).unwrap();
        let service = ArchitectureService::open(dir.path(), packs, TypeSystem::defaults().unwrap(), Box::new(MemoryPlans::default()), store).unwrap();
        (dir, service)
    }

    #[test]
    fn a_change_that_panics_leaves_the_writer_running_on_the_kept_rows() {
        let (dir, service) = service();
        service.ingest(dir.path()).unwrap();
        let before = service.observed().unwrap().len();
        assert!(before.1 > 0 && service.file_rows().len() == 2);
        let failed = service.pipeline.run(&service.core, |_, model| {
            *model = Model::default();
            panic!("a bug in a change");
        });
        assert!(failed.is_err(), "the caller hears it failed");
        // The next change runs, on the rows as kept (not the half-changed model).
        let files = service.pipeline.run(&service.core, |_, model| model.len()).unwrap();
        assert_eq!(files, service.file_rows().len());
        service.ingest(dir.path()).unwrap();
        assert_eq!(service.observed().unwrap().len(), before);
    }
}
