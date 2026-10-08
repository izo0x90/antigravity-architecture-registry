//! The architecture service: the code (observed, from the index) with plans
//! laid over it. Plans change only through operations; the observed graph
//! changes only through ingestion. Every change is announced to listeners.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arch_store::{Change, FactStore, Stamp, Stored};
use rayon::prelude::*;

use arch_graph::tree::{Tree, TreeRequest};
use arch_graph::{
    Alignment, Approval, ArchEvent, ChangeReason, Component, ComponentId, Finding, FindingCode, Graph, Plan, PlanId, Report, Requester,
    RepoPath, Severity, Source, Stage, Target, TypeSystem, ViewRequest, can_transition, ComponentDetail, schema::Dsl,
    IndexState, IndexStatus, IndexSummary,
};
use arch_index::{AmbiguousCall, CodeRules, ExternalCall, FileExtract, PackSet, content_hash, extract_file, resolve, walk, UnverifiedFlow};
use serde::{Deserialize, Serialize};

use crate::progress::{Downgrade, cascade};
use crate::import::ImportReport;
use crate::contract::{ContractSlice, slice};
use crate::ops::{PlanOp, apply};
use crate::review::{ReviewProvider, ReviewerCatalog, ReviewsReport, Subject};
use crate::store::PlanRepository;
use crate::tools::{ProcessRunner, ToolCatalog, ToolRunner, ToolsReport};
use crate::{ServiceError, chain, checks, review, tools, verify};

mod comments;
mod search;
pub use comments::{NewComment, approval_blocker};

/// Who is asking. Approvals are for users only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    User { name: String },
    Agent { session: String },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IngestReport {
    pub files: usize,
    pub parsed: usize,
    /// Files not read for facts again (unchanged).
    pub cached: usize,
    /// Files whose content was read (changed by size or time).
    #[serde(default)]
    pub read: usize,
    pub removed: usize,
    pub components: usize,
    pub relations: usize,
    pub ambiguous: usize,
    /// Calls not linked to anything: the receiver's type or the name isn't
    /// known from the code (never guessed).
    #[serde(default)]
    pub unproven: usize,
    pub external: usize,
    pub millis: u64,
    /// Files whose content changed (parsed anew or removed).
    #[serde(default)]
    pub changed: Vec<RepoPath>,
}

/// How far an indexing run got (progress is announced at most this often).
/// Files are read on many threads at once, so both are shared.
struct Progress {
    done: AtomicUsize,
    last: Mutex<Instant>,
}

/// Every source file's facts as last read: what the graph is linked from.
#[derive(Default)]
struct Files {
    /// Sorted by file.
    extracts: Vec<FileExtract>,
    stamps: HashMap<RepoPath, Stamp>,
}

impl Files {
    fn new(stored: Vec<Stored>) -> Self {
        let mut files = Self::default();
        files.apply(stored.into_iter().map(|s| Change::Put(Box::new(s))).collect());
        files
    }

    fn get(&self, file: &RepoPath) -> Option<(&FileExtract, Stamp)> {
        let at = self.extracts.binary_search_by(|e| e.file.cmp(file)).ok()?;
        Some((&self.extracts[at], self.stamps.get(file).copied().unwrap_or_default()))
    }

    fn apply(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        let mut by_file: BTreeMap<RepoPath, FileExtract> = std::mem::take(&mut self.extracts).into_iter().map(|e| (e.file.clone(), e)).collect();
        for change in changes {
            match change {
                Change::Put(stored) => {
                    self.stamps.insert(stored.extract.file.clone(), stored.stamp);
                    by_file.insert(stored.extract.file.clone(), stored.extract);
                }
                Change::Restamp { file, stamp } => {
                    self.stamps.insert(file, stamp);
                }
                Change::Remove(file) => {
                    self.stamps.remove(&file);
                    by_file.remove(&file);
                }
            }
        }
        self.extracts = by_file.into_values().collect();
    }
}

/// What reading one file came to.
enum Read {
    /// Not a file any pack reads.
    Skipped,
    /// Unchanged by size and time: not read.
    Unread,
    /// Read, and found the same.
    Same,
    Changed(Change),
}

const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_millis(150);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verification {
    pub report: Report,
    /// The stage it moved to, when verification passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced_to: Option<Stage>,
}

/// A component re-verified because code it depends on changed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reverified {
    pub plan: PlanId,
    pub component: ComponentId,
    /// The stage it was re-verified for (its current one).
    pub stage: Stage,
    pub report: Report,
    /// Where it was sent back to because the code no longer holds up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demoted_to: Option<Stage>,
}

/// What a refresh read and which components it re-verified.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Refresh {
    pub ingest: IngestReport,
    pub reverified: Vec<Reverified>,
    /// Planned components sent back because code they build on changed shape.
    pub downgraded: Vec<(PlanId, Downgrade)>,
}

/// The code as last indexed: read-only once built. Each index run builds a
/// new one and swaps it in; readers hold the one they started with.
#[derive(Default)]
struct Observed {
    graph: Arc<Graph>,
    ambiguous: Vec<AmbiguousCall>,
    unproven: Vec<AmbiguousCall>,
    external: Vec<ExternalCall>,
    /// Data handed from one call to another that types couldn't confirm.
    unverified: Vec<UnverifiedFlow>,
    /// Counts up with every swap (a plan's overlay is built for one).
    generation: u64,
}

/// A plan laid over the code, kept until the plan or the code changes.
struct Merged {
    generation: u64,
    plan: Arc<Plan>,
    graph: Arc<Graph>,
    used: Instant,
}

type Listener = Arc<dyn Fn(&ArchEvent) + Send + Sync>;

pub struct ArchitectureService {
    root: PathBuf,
    packs: PackSet,
    types: TypeSystem,
    dsl: Dsl,
    trees: arch_graph::tree::TreeConfig,
    rules: CodeRules,
    plans: Box<dyn PlanRepository>,
    /// Where every file's facts are kept between runs.
    facts: Box<dyn FactStore>,
    /// Every file's facts, as last read.
    files: RwLock<Files>,
    /// The repo's file list for searching files and lines (read on first use).
    repo_files: RwLock<Option<arch_search::RepoFiles>>,
    observed: RwLock<Arc<Observed>>,
    /// Plans laid over the code, by plan (the most recently used few).
    merged: Mutex<HashMap<PlanId, Merged>>,
    limits: crate::Limits,
    listeners: RwLock<Vec<Listener>>,
    tools: ToolCatalog,
    runner: Arc<dyn ToolRunner>,
    reviewers: ReviewerCatalog,
    review_provider: Option<Arc<dyn ReviewProvider>>,
    /// One lock per plan, held across load -> change -> save.
    plan_locks: Mutex<HashMap<PlanId, Arc<Mutex<()>>>>,
    /// Indexing as clients see it (state, progress, last run, stale files).
    index_status: RwLock<IndexStatus>,
    /// One indexing run at a time (the watcher, agents and the user can all
    /// start one).
    ingest_lock: Mutex<()>,
}

fn change_file(change: &Change) -> &RepoPath {
    match change {
        Change::Put(stored) => &stored.extract.file,
        Change::Restamp { file, .. } | Change::Remove(file) => file,
    }
}

fn now_rfc3339() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl ArchitectureService {
    /// Opens the service over `root`, loading the facts kept from the last
    /// run (no file is read). Code rules start as the defaults; see `with_rules`.
    pub fn open(root: &Path, packs: PackSet, types: TypeSystem, plans: Box<dyn PlanRepository>, facts: Box<dyn FactStore>) -> Result<Self, ServiceError> {
        let files = RwLock::new(Files::new(facts.load()?));
        // Packs map onto types: every one they name must exist.
        packs.check_types(&types).map_err(|e| ServiceError::Index(e.to_string()))?;
        let rules = CodeRules::defaults(&packs, &types).map_err(|e| ServiceError::Index(e.to_string()))?;
        let service = Self {
            root: root.to_path_buf(),
            packs,
            types,
            dsl: Dsl::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in schema DSL: {e}")))?,
            trees: arch_graph::tree::TreeConfig::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in tree config: {e}")))?,
            rules,
            plans,
            facts,
            files,
            repo_files: RwLock::new(None),
            observed: RwLock::new(Arc::new(Observed::default())),
            merged: Mutex::new(HashMap::new()),
            limits: crate::Limits::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in limits: {e}")))?,
            listeners: RwLock::new(Vec::new()),
            tools: ToolCatalog::default(),
            runner: Arc::new(ProcessRunner),
            reviewers: ReviewerCatalog::defaults()?,
            review_provider: None,
            plan_locks: Mutex::new(HashMap::new()),
            index_status: RwLock::new(IndexStatus::default()),
            ingest_lock: Mutex::new(()),
        };
        service.rebuild_observed()?;
        Ok(service)
    }

    /// Replaces the schema DSL's aliases (a project's own shorthand types).
    pub fn with_dsl(mut self, dsl: Dsl) -> Self {
        self.dsl = dsl;
        self
    }

    pub fn dsl(&self) -> &Dsl {
        &self.dsl
    }

    /// Replaces the code rules (e.g. defaults plus the project's .nah/rules).
    pub fn with_rules(mut self, rules: CodeRules) -> Self {
        self.rules = rules;
        self
    }

    /// The project's validation tools and how they're run.
    pub fn with_tools(mut self, tools: ToolCatalog, runner: Arc<dyn ToolRunner>) -> Self {
        self.tools = tools;
        self.runner = runner;
        self
    }

    /// The reviewers and the provider that runs session reviews.
    pub fn with_reviewers(mut self, reviewers: ReviewerCatalog, provider: Option<Arc<dyn ReviewProvider>>) -> Self {
        self.reviewers = reviewers;
        self.review_provider = provider;
        self
    }

    pub fn tools(&self) -> &ToolCatalog {
        &self.tools
    }

    pub fn reviewers(&self) -> &ReviewerCatalog {
        &self.reviewers
    }

    fn check_cx(&self) -> checks::CheckCx<'_> {
        checks::CheckCx { root: &self.root, packs: &self.packs, types: &self.types, tools: &self.tools }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn types(&self) -> &TypeSystem {
        &self.types
    }


    pub fn subscribe(&self, listener: impl Fn(&ArchEvent) + Send + Sync + 'static) {
        if let Ok(mut l) = self.listeners.write() {
            l.push(Arc::new(listener));
        }
    }

    fn emit(&self, event: ArchEvent) {
        let listeners: Vec<Listener> = self.listeners.read().map(|l| l.clone()).unwrap_or_default();
        for listener in listeners {
            listener(&event);
        }
    }

    fn plan_lock(&self, id: &PlanId) -> Arc<Mutex<()>> {
        self.plan_locks.lock().unwrap_or_else(PoisonError::into_inner).entry(id.clone()).or_default().clone()
    }

    /// Loads plan `id`, lets `change` edit it, and saves it if it changed,
    /// all under the plan's lock: concurrent edits (the user, agents, the
    /// file watcher) never overwrite each other. `change` must not edit the
    /// same plan again; announce events after this returns.
    pub(crate) fn edit_plan<T>(&self, id: &PlanId, change: impl FnOnce(&mut Plan) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        let lock = self.plan_lock(id);
        let _held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut plan = self.plan(id)?;
        let before = plan.clone();
        let out = change(&mut plan)?;
        if plan != before {
            self.plans.save(&plan)?;
        }
        Ok(out)
    }

    fn rebuild_observed(&self) -> Result<(), ServiceError> {
        let resolved = resolve(&self.files.read().map_err(ServiceError::poisoned)?.extracts, &self.packs, &self.types);
        let generation = self.snapshot()?.generation + 1;
        let next = Arc::new(Observed {
            graph: Arc::new(resolved.graph),
            ambiguous: resolved.ambiguous,
            unproven: resolved.unproven,
            external: resolved.external,
            unverified: resolved.unverified_flows,
            generation,
        });
        *self.observed.write().map_err(ServiceError::poisoned)? = next;
        Ok(())
    }

    /// The code as last indexed (shared, never copied).
    fn snapshot(&self) -> Result<Arc<Observed>, ServiceError> {
        Ok(self.observed.read().map_err(ServiceError::poisoned)?.clone())
    }

    /// The service's limits (from config).
    pub fn limits(&self) -> &crate::Limits {
        &self.limits
    }

    /// The three views' settings.
    pub fn trees(&self) -> &arch_graph::tree::TreeConfig {
        &self.trees
    }

    /// Replaces the views' settings (a project's own).
    pub fn with_trees(mut self, trees: arch_graph::tree::TreeConfig) -> Self {
        self.trees = trees;
        self
    }

    /// Replaces the limits (a project's own, or a test's).
    pub fn with_limits(mut self, limits: crate::Limits) -> Self {
        self.limits = limits;
        self
    }

    /// Re-reads `scope` (a path inside the repo): parses changed files, drops
    /// deleted ones, then re-resolves the whole graph.
    pub fn ingest(&self, scope: &Path) -> Result<IngestReport, ServiceError> {
        self.ingest_scopes(&[scope.to_path_buf()])
    }

    /// Rebuilds the whole index from scratch: the cache is dropped and every
    /// file is read again.
    pub fn reindex(&self) -> Result<IngestReport, ServiceError> {
        let _one = self.ingest_lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.facts.clear()?;
        self.forget_files();
        *self.files.write().map_err(ServiceError::poisoned)? = Files::default();
        self.ingest_locked(&[self.root.clone()])
    }

    fn ingest_scopes(&self, scopes: &[PathBuf]) -> Result<IngestReport, ServiceError> {
        let _one = self.ingest_lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.ingest_locked(scopes)
    }

    fn ingest_locked(&self, scopes: &[PathBuf]) -> Result<IngestReport, ServiceError> {
        match self.run_ingest(scopes) {
            Ok(report) => Ok(report),
            Err(e) => {
                self.set_index_status(|s| {
                    s.state = IndexState::Failed;
                    s.error = Some(e.to_string());
                });
                Err(e)
            }
        }
    }

    fn run_ingest(&self, scopes: &[PathBuf]) -> Result<IngestReport, ServiceError> {
        let started = Instant::now();
        let mut report = IngestReport::default();
        // Every file first, so progress has a total.
        let mut found = Vec::new();
        for scope in scopes {
            let files = match scope.exists() {
                true => walk(&self.root, scope, &self.packs).map_err(|e| ServiceError::Index(e.to_string()))?,
                false => Vec::new(),
            };
            found.push((scope.clone(), files));
        }
        let total = found.iter().map(|(_, f)| f.len()).sum();
        self.set_index_status(|s| {
            s.state = IndexState::Indexing;
            s.done = 0;
            s.total = total;
            s.error = None;
        });
        self.read_files(&found, &mut report)?;
        self.rebuild_observed()?;
        let observed = self.snapshot()?;
        (report.components, report.relations) = observed.graph.len();
        report.ambiguous = observed.ambiguous.len();
        report.unproven = observed.unproven.len();
        report.external = observed.external.len();
        report.millis = started.elapsed().as_millis() as u64;
        drop(observed);
        // What was read is fresh again.
        let read: Vec<String> = scopes.iter().map(|s| self.scope_prefix(s)).collect();
        self.set_index_status(|s| {
            s.state = IndexState::Idle;
            s.done = total;
            s.last = Some(IndexSummary {
                millis: report.millis,
                files: report.files,
                parsed: report.parsed,
                components: report.components,
                relations: report.relations,
                unresolved: report.ambiguous + report.unproven,
                at: now_rfc3339(),
            });
            s.stale.retain(|stale| !read.iter().any(|scope| scope.is_empty() || stale == scope || stale.starts_with(&format!("{scope}/"))));
        });
        self.emit(ArchEvent::Changed { reason: ChangeReason::Ingested, plans: Vec::new(), components: Vec::new() });
        Ok(report)
    }

    /// Indexing as clients see it.
    pub fn index_status(&self) -> IndexStatus {
        self.index_status.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Changes the status and tells clients.
    fn set_index_status(&self, change: impl FnOnce(&mut IndexStatus)) {
        let status = {
            let mut s = self.index_status.write().unwrap_or_else(PoisonError::into_inner);
            change(&mut s);
            s.clone()
        };
        self.emit(ArchEvent::Index { status: Box::new(status) });
    }

    /// Files changed on disk (the watcher saw them) that haven't been read
    /// since: shown as out of date until they are.
    pub fn mark_stale(&self, paths: &[PathBuf]) -> Result<(), ServiceError> {
        let mut changed: Vec<String> = Vec::new();
        for path in paths {
            if self.watches(path)? {
                let prefix = self.scope_prefix(path);
                if !prefix.is_empty() {
                    changed.push(prefix);
                }
            }
        }
        let known = self.index_status().stale;
        changed.retain(|p| !known.contains(p));
        changed.sort();
        changed.dedup();
        if changed.is_empty() {
            return Ok(());
        }
        self.set_index_status(|s| {
            s.stale.extend(changed);
            s.stale.sort();
            s.stale.dedup();
        });
        Ok(())
    }

    /// Whether `file` is out of date: changed on disk since it was indexed
    /// (by the watcher's word, or its content no longer matching the index).
    pub fn is_stale(&self, file: &str) -> Result<bool, ServiceError> {
        if self.index_status().is_stale(file) {
            return Ok(true);
        }
        let Ok(path) = RepoPath::new(file) else { return Ok(false) };
        let absolute = self.root.join(path.as_str());
        let files = self.files.read().map_err(ServiceError::poisoned)?;
        let Some((extract, stamp)) = files.get(&path) else { return Ok(false) };
        let Ok(meta) = std::fs::metadata(&absolute) else { return Ok(false) };
        if stamp.modified != 0 && Stamp::of(&meta) == stamp {
            return Ok(false);
        }
        let Ok(content) = std::fs::read_to_string(&absolute) else { return Ok(false) };
        Ok(extract.content_hash != content_hash(&content))
    }

    /// Brings the index up to date for one scope (a scope that no longer
    /// exists just drops what was indexed under it).
    /// `scope` as a repo-relative prefix ("" for the whole repo), spelled the
    /// way index entries are.
    fn scope_prefix(&self, scope: &Path) -> String {
        let absolute = if scope.is_absolute() { scope.to_path_buf() } else { self.root.join(scope) };
        let relative = absolute.strip_prefix(&self.root).unwrap_or(&absolute).to_string_lossy().into_owned();
        RepoPath::new(&relative).map(|p| p.as_str().to_owned()).unwrap_or_default()
    }

    fn under(file: &RepoPath, prefix: &str) -> bool {
        prefix.is_empty() || file.as_str() == prefix || file.as_str().starts_with(&format!("{prefix}/"))
    }

    /// Brings every file's facts up to date for the scopes walked: a file
    /// whose size and time match what was kept isn't read; the rest are read
    /// on many threads at once, and only those whose content changed are
    /// read for facts again. Files under a scope that are gone (or now
    /// ignored) are dropped. The changes are kept together, all or none.
    fn read_files(&self, found: &[(PathBuf, Vec<arch_index::SourceFile>)], report: &mut IngestReport) -> Result<(), ServiceError> {
        let mut unique: BTreeMap<&RepoPath, &arch_index::SourceFile> = BTreeMap::new();
        for (_, files) in found {
            report.files += files.len();
            unique.extend(files.iter().map(|f| (&f.path, f)));
        }
        let recent = std::time::Duration::from_millis(self.limits.store.recent_change_ms);
        let trusted_before = SystemTime::now().checked_sub(recent).unwrap_or(UNIX_EPOCH).duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
        let progress = Progress { done: AtomicUsize::new(0), last: Mutex::new(Instant::now()) };
        let mut changes: Vec<Change> = {
            let files = self.files.read().map_err(ServiceError::poisoned)?;
            let read: Vec<Result<Read, ServiceError>> = unique.values().collect::<Vec<_>>().par_iter().map(|file| {
                let read = self.read_file(file, &files, trusted_before);
                self.tick(&progress);
                read
            }).collect();
            let mut changes = Vec::new();
            for r in read {
                match r? {
                    Read::Skipped => {}
                    Read::Unread => report.cached += 1,
                    Read::Same => {
                        report.cached += 1;
                        report.read += 1;
                    }
                    Read::Changed(change) => {
                        report.read += 1;
                        if let Change::Put(stored) = &change {
                            report.parsed += 1;
                            report.changed.push(stored.extract.file.clone());
                        }
                        changes.push(change);
                    }
                }
            }
            // Files under a scope that weren't found there leave.
            let prefixes: Vec<String> = found.iter().map(|(scope, _)| self.scope_prefix(scope)).collect();
            for extract in files.extracts.iter().filter(|e| !unique.contains_key(&e.file) && prefixes.iter().any(|p| Self::under(&e.file, p))) {
                report.removed += 1;
                report.changed.push(extract.file.clone());
                changes.push(Change::Remove(extract.file.clone()));
            }
            changes
        };
        report.changed.sort();
        changes.sort_by(|a, b| change_file(a).cmp(change_file(b)));
        self.facts.apply(&changes)?;
        self.files.write().map_err(ServiceError::poisoned)?.apply(changes);
        Ok(())
    }

    /// One file: skipped if its size and time match what was kept (and that
    /// time was trusted), else read and compared by content.
    fn read_file(&self, file: &arch_index::SourceFile, files: &Files, trusted_before: i64) -> Result<Read, ServiceError> {
        let Some(pack) = self.packs.for_path(&file.absolute) else { return Ok(Read::Skipped) };
        let io = |e: std::io::Error| ServiceError::Io(format!("{}: {e}", file.path));
        let mut stamp = Stamp::of(&std::fs::metadata(&file.absolute).map_err(io)?);
        // Changed too recently to tell a later change in the same tick of the clock.
        if stamp.modified > trusted_before {
            stamp.modified = 0;
        }
        let kept = files.get(&file.path).filter(|(e, _)| e.module == file.module && e.pack_fingerprint == pack.fingerprint);
        if let Some((_, was)) = kept
            && was.modified != 0
            && was == stamp
        {
            return Ok(Read::Unread);
        }
        let content = std::fs::read_to_string(&file.absolute).map_err(io)?;
        Ok(match kept {
            Some((e, was)) if e.content_hash == content_hash(&content) => match was == stamp {
                true => Read::Same,
                false => Read::Changed(Change::Restamp { file: file.path.clone(), stamp }),
            },
            _ => Read::Changed(Change::Put(Box::new(Stored { extract: extract_file(pack, file.path.clone(), file.module.clone(), &content), stamp }))),
        })
    }

    /// Counts one file done, and tells clients now and then.
    fn tick(&self, progress: &Progress) {
        let done = progress.done.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut last) = progress.last.try_lock()
            && last.elapsed() >= PROGRESS_EVERY
        {
            *last = Instant::now();
            self.set_index_status(|s| s.done = s.done.max(done));
        }
    }

    /// Reacts to changed paths (from a file watcher, say): re-reads the ones
    /// that are source files the walk wouldn't ignore, then re-verifies only
    /// the planned components (already in the code) that the changed files
    /// affect: their own file, or the file of something they're planned to
    /// call. Re-verification checks the code (location, calls, signature,
    /// call chain); tools and reviewers run on explicit verification only.
    pub fn refresh(&self, paths: &[PathBuf]) -> Result<Refresh, ServiceError> {
        self.files_changed(paths);
        let mut scopes: Vec<PathBuf> = Vec::new();
        for p in paths {
            if self.watches(p)? {
                scopes.push(p.clone());
            }
        }
        if scopes.is_empty() {
            return Ok(Refresh::default());
        }
        let before: BTreeMap<PlanId, Arc<Graph>> = {
            let mut before = BTreeMap::new();
            for id in self.plans.list()? {
                before.insert(id.clone(), self.graph(&Source::Plan(id))?);
            }
            before
        };
        let ingest = self.ingest_scopes(&scopes)?;
        let changed: BTreeSet<&str> = ingest.changed.iter().map(RepoPath::as_str).collect();
        let observed = self.observed()?;
        let mut reverified = Vec::new();
        let mut downgraded = Vec::new();
        for (plan_id, before) in &before {
            // Under the plan's lock, on the plan as it is now (an approval or
            // edit made while the files were read isn't lost).
            let (sent_back, checked) = self.edit_plan(plan_id, |plan| {
                let after = Graph::merge((*observed).clone(), std::slice::from_ref(plan));
                // Existing code the plan builds on changed its signature: what
                // was designed against the old one needs another look.
                let reshaped: Vec<ComponentId> = observed
                    .components()
                    .filter(|c| plan.component(&c.id).is_none())
                    .filter(|c| before.component(&c.id).is_some_and(|b| b.signature.is_some() && b.signature != c.signature))
                    .map(|c| c.id.clone())
                    .collect();
                let mut sent_back = Vec::new();
                for id in &reshaped {
                    sent_back.extend(cascade(plan, &after, &self.types, id));
                }
                revoke(plan, &sent_back);
                let mut checked = Vec::new();
                for id in affected(plan, before, &after, &changed) {
                    let Some(planned) = plan.component(&id).cloned() else { continue };
                    let report = self.check(plan, &planned, &observed, planned.stage, false)?;
                    // Code that no longer holds up loses the stage it earned.
                    let demoted_to = demotion(plan, &planned, &report, &observed);
                    if let (Some(to), Some(c)) = (demoted_to, plan.component_mut(&id)) {
                        c.stage = to;
                    }
                    checked.push(Reverified { plan: plan.id.clone(), component: id, stage: planned.stage, report, demoted_to });
                }
                Ok((sent_back, checked))
            })?;
            let mut moved: Vec<ComponentId> = sent_back.iter().map(|d| d.id.clone()).collect();
            moved.extend(checked.iter().filter(|r| r.demoted_to.is_some()).map(|r| r.component.clone()));
            if !moved.is_empty() {
                self.emit(ArchEvent::Changed { reason: ChangeReason::StageChanged, plans: vec![plan_id.clone()], components: moved });
            }
            for r in &checked {
                self.emit_validated(plan_id, Some(&r.component), &r.report);
            }
            downgraded.extend(sent_back.into_iter().map(|d| (plan_id.clone(), d)));
            reverified.extend(checked);
        }
        Ok(Refresh { ingest, reverified, downgraded })
    }

    /// Whether a changed path is something ingestion reads: inside the repo,
    /// not ignored by the walk, and a source file a pack handles, a directory,
    /// or a vanished path with indexed files under it (a renamed directory).
    fn watches(&self, path: &Path) -> Result<bool, ServiceError> {
        if !path.starts_with(&self.root) || arch_index::ignored(&self.root, path) {
            return Ok(false);
        }
        if path.is_dir() || self.packs.for_path(path).is_some() {
            return Ok(true);
        }
        // A vanished directory (renamed or deleted) with indexed files under it.
        if path.exists() {
            return Ok(false);
        }
        let prefix = self.scope_prefix(path);
        Ok(!prefix.is_empty() && self.files.read().map_err(ServiceError::poisoned)?.extracts.iter().any(|e| Self::under(&e.file, &prefix)))
    }

    /// The code as last indexed (shared: reading it copies nothing).
    pub fn observed(&self) -> Result<Arc<Graph>, ServiceError> {
        Ok(self.snapshot()?.graph.clone())
    }

    pub fn ambiguous_calls(&self) -> Result<Vec<AmbiguousCall>, ServiceError> {
        Ok(self.snapshot()?.ambiguous.clone())
    }

    /// Calls the code makes that couldn't be tied to a component (unknown
    /// receiver type, or a name neither local nor imported), with candidates.
    pub fn unproven_calls(&self) -> Result<Vec<AmbiguousCall>, ServiceError> {
        Ok(self.snapshot()?.unproven.clone())
    }

    /// The code alone, or the code with one plan laid over it. A plan's
    /// overlay is built once and kept (shared) until the plan or the code
    /// changes; only the most recently used few are kept.
    pub fn graph(&self, source: &Source) -> Result<Arc<Graph>, ServiceError> {
        let id = match source {
            Source::Code => return self.observed(),
            Source::Plan(id) => id,
        };
        let plan = self.plan_shared(id)?;
        let observed = self.snapshot()?;
        let mut merged = self.merged.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(m) = merged.get_mut(id)
            && m.generation == observed.generation
            && Arc::ptr_eq(&m.plan, &plan)
        {
            m.used = Instant::now();
            return Ok(m.graph.clone());
        }
        let graph = Arc::new(Graph::merge((*observed.graph).clone(), std::slice::from_ref(&*plan)));
        merged.insert(id.clone(), Merged { generation: observed.generation, plan, graph: graph.clone(), used: Instant::now() });
        while merged.len() > self.limits.plans_kept_laid_over.max(1) {
            let Some(oldest) = merged.iter().min_by_key(|(_, m)| m.used).map(|(id, _)| id.clone()) else { break };
            merged.remove(&oldest);
        }
        Ok(graph)
    }

    pub fn plans(&self) -> Result<Vec<Plan>, ServiceError> {
        self.plans.list()?.iter().filter_map(|id| self.plans.get(id).transpose()).map(|p| p.map(|p| (*p).clone())).collect()
    }

    pub fn plan(&self, id: &PlanId) -> Result<Plan, ServiceError> {
        Ok((*self.plan_shared(id)?).clone())
    }

    /// The plan as saved (shared, not copied).
    pub fn plan_shared(&self, id: &PlanId) -> Result<Arc<Plan>, ServiceError> {
        self.plans.get(id)?.ok_or_else(|| ServiceError::NotFound(format!("plan {id}")))
    }

    pub fn create_plan(&self, id: &str, title: &str, description: &str) -> Result<Plan, ServiceError> {
        let plan_id = PlanId::new(id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        let lock = self.plan_lock(&plan_id);
        let held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if self.plans.get(&plan_id)?.is_some() {
            return Err(ServiceError::InvalidOp(format!("plan {id} already exists")));
        }
        let mut plan = Plan::new(id, title);
        plan.description = description.to_owned();
        self.plans.save(&plan)?;
        drop(held);
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id], components: Vec::new() });
        Ok(plan)
    }

    /// Creates plan `id` from the old registry document (`json`), matched
    /// against the code. `language` places components that have neither a
    /// location nor code to match.
    pub fn import_registry(&self, id: &str, title: &str, json: &str, language: Option<&str>) -> Result<(Plan, ImportReport), ServiceError> {
        let plan_id = PlanId::new(id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        let lock = self.plan_lock(&plan_id);
        let held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if self.plans.get(&plan_id)?.is_some() {
            return Err(ServiceError::InvalidOp(format!("plan {id} already exists")));
        }
        let input = crate::import::ImportInput { plan: plan_id.clone(), title: title.to_owned(), json, language: language.map(str::to_owned) };
        let (plan, report) = crate::import::import(input, &*self.observed()?, &self.root, &self.packs, &self.types)?;
        self.plans.save(&plan)?;
        drop(held);
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id], components: plan.components.iter().map(|c| c.id.clone()).collect() });
        Ok((plan, report))
    }

    /// `import_registry` from a repo file (relative to the root).
    pub fn import_registry_file(&self, id: &str, title: &str, file: &str, language: Option<&str>) -> Result<(Plan, ImportReport), ServiceError> {
        let path = arch_graph::RepoPath::new(file).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        let json = std::fs::read_to_string(self.root.join(path.as_str())).map_err(|e| ServiceError::NotFound(format!("{file}: {e}")))?;
        self.import_registry(id, title, &json, language)
    }

    pub fn delete_plan(&self, id: &PlanId) -> Result<(), ServiceError> {
        let lock = self.plan_lock(id);
        let held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.plans.delete(id)?;
        drop(held);
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![id.clone()], components: Vec::new() });
        Ok(())
    }

    /// Applies one operation to a plan.
    pub fn apply(&self, plan_id: &PlanId, op: PlanOp) -> Result<Plan, ServiceError> {
        let observed = self.observed()?;
        let (plan, touched) = self.edit_plan(plan_id, |plan| {
            let graph = Graph::merge((*observed).clone(), std::slice::from_ref(plan));
            let changed = op.design_change().cloned();
            let removes = matches!(op, PlanOp::Remove { .. });
            let mut touched = apply(plan, op, &graph, &self.root, &self.packs, &self.types, &self.dsl)?;
            // A design change sends it (if approved) and what builds on it back.
            if let Some(changed) = changed {
                // A removed component's dependents are only visible before removal.
                let basis = if removes { graph } else { Graph::merge((*observed).clone(), std::slice::from_ref(plan)) };
                let downgrades = cascade(plan, &basis, &self.types, &changed);
                revoke(plan, &downgrades);
                touched.extend(downgrades.into_iter().map(|d| d.id));
            }
            Ok((plan.clone(), touched))
        })?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id.clone()], components: touched });
        Ok(plan)
    }

    /// The components a plan covers: its own plus referenced neighborhoods.
    pub fn scope(&self, plan_id: &PlanId) -> Result<Vec<ComponentId>, ServiceError> {
        let plan = self.plan(plan_id)?;
        let graph = self.graph(&Source::Plan(plan_id.clone()))?;
        let mut ids: BTreeSet<ComponentId> = plan.components.iter().map(|c| c.id.clone()).collect();
        for reference in &plan.references {
            ids.insert(reference.id.clone());
            if reference.depth > 0 && !reference.kinds.is_empty() {
ids.extend(graph.reach(&reference.id, &reference.kinds, reference.depth));
            }
        }
        Ok(ids.into_iter().collect())
    }

    /// Plan checks (before any code is written).
    pub fn validate(&self, plan_id: &PlanId) -> Result<Report, ServiceError> {
        let plan = self.plan(plan_id)?;
        let graph = self.graph(&Source::Plan(plan_id.clone()))?;
        let mut report = checks::check_plan(&plan, &graph, &self.check_cx());
        // Code rules over the plan's scope as it exists in code.
        let scope = self.scope(plan_id)?;
        report.findings.extend(self.rules.check(scope.iter().filter_map(|id| graph.component(id)), &graph, &self.root, &self.packs, &self.types));
        // Logic blocks are tested on their own.
        let planned = plan.components.iter().filter_map(|c| graph.component(&c.id));
        report.findings.extend(tools::run_logic_checks(planned, &self.tools.logic_checks, self.runner.as_ref(), &self.root));
        let report = report.sorted();
        self.emit_validated(plan_id, None, &report);
        Ok(report)
    }

    fn emit_validated(&self, plan: &PlanId, component: Option<&ComponentId>, report: &Report) {
        let count = |s: Severity| report.findings.iter().filter(|f| f.severity == s).count();
        self.emit(ArchEvent::Validated { plan: plan.clone(), component: component.cloned(), errors: count(Severity::Error), warnings: count(Severity::Warning) });
    }

    /// Design approvals (ArchApproved, PlanApproved). Only a user can approve,
    /// and only a component whose checks pass.
    pub fn approve(&self, plan_id: &PlanId, id: &ComponentId, to: Stage, actor: &Actor) -> Result<Approval, ServiceError> {
        let Actor::User { name } = actor else {
            return Err(ServiceError::Forbidden("approvals come from the user in the cockpit; present the contract and wait".into()));
        };
        if !matches!(to, Stage::ArchApproved | Stage::PlanApproved) {
            return Err(ServiceError::InvalidOp(format!("{to:?} is reached by verification, not approval")));
        }
        let observed = self.observed()?;
        let approval = self.edit_plan(plan_id, |plan| {
            let from = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.stage;
            if !can_transition(from, to) {
                return Err(ServiceError::InvalidOp(format!("can't go from {from:?} to {to:?}")));
            }
            if let Some(blocked) = approval_blocker(plan, id) {
                return Err(ServiceError::InvalidOp(blocked));
            }
            // Check as if approved (location rules depend on the stage).
            if let Some(c) = plan.component_mut(id) {
                c.stage = to;
            }
            let graph = Graph::merge((*observed).clone(), std::slice::from_ref(plan));
            let mut report = checks::check_plan(plan, &graph, &self.check_cx());
            // The same blocking set validation shows for it: code rules on its
            // code and its logic steps' own checks.
            report.findings.extend(self.rules.check(graph.component(id), &graph, &self.root, &self.packs, &self.types));
            report.findings.extend(tools::run_logic_checks(graph.component(id).into_iter(), &self.tools.logic_checks, self.runner.as_ref(), &self.root));
            // Errors on it, on its relations, or on the plan as a whole.
            let blocking: Vec<String> = checks::blocking_for(&report, plan, id).into_iter().map(|f| f.message.clone()).collect();
            if !blocking.is_empty() {
                return Err(ServiceError::InvalidOp(format!("{id} can't be approved yet: {}", blocking.join("; "))));
            }
            let approval = Approval { component: id.clone(), from, to, by: name.clone(), at: now_rfc3339(), kind: arch_graph::ApprovalKind::Approved };
            plan.approvals.push(approval.clone());
            Ok(approval)
        })?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::Approved, plans: vec![plan_id.clone()], components: vec![id.clone()] });
        Ok(approval)
    }

    pub fn contract(&self, plan_id: &PlanId, id: &ComponentId) -> Result<ContractSlice, ServiceError> {
        let plan = self.plan(plan_id)?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?;
        let graph = self.graph(&Source::Plan(plan_id.clone()))?;
        Ok(slice(&plan, planned, &graph, &checks::schema_set(&graph, &self.types)))
    }

    /// Re-reads the component's file (and its call chain's), verifies it for
    /// `goal` (Skeleton or Implemented) and, when clean, moves it there. The
    /// whole planned call chain through it is re-checked; Implemented also
    /// runs its validation tools and reviewers. Nothing else moves a
    /// component to Implemented.
    pub fn verify(&self, plan_id: &PlanId, id: &ComponentId, goal: Stage) -> Result<Verification, ServiceError> {
        if !matches!(goal, Stage::Skeleton | Stage::Implemented) {
            return Err(ServiceError::InvalidOp(format!("verification targets Skeleton or Implemented, not {goal:?}")));
        }
        let plan = self.plan(plan_id)?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.clone();
        // Re-read everything the checks look at, including files that are gone
        // (re-reading a vanished file drops it from the index).
        let merged = self.graph(&Source::Plan(plan_id.clone()))?;
        let scopes: Vec<PathBuf> = match verification_files(&plan, id, &merged) {
            Some(files) => files.iter().map(|f| self.root.join(f)).collect(),
            // Something it depends on isn't where the index last saw it: re-read
            // the repo (incrementally: only changed files are parsed).
            None => vec![self.root.clone()],
        };
        if !scopes.is_empty() {
            self.ingest_scopes(&scopes)?;
        }
        let observed = self.observed()?;
        let mut report = self.check(&plan, &planned, &observed, goal, goal == Stage::Implemented)?;
        let merged = Graph::merge((*observed).clone(), std::slice::from_ref(&plan));
        for f in checks::stage_gate(&plan, &merged, &self.types, id, goal) {
            report.push(f);
        }
        if goal == Stage::Implemented && planned.stage != Stage::Implemented && !plan.plan_approved(id) {
            report.push(
                Finding::error(
                    FindingCode::ApprovalRequired,
                    Target::Component { id: id.clone() },
                    format!("{id} can only be implemented once the user approves its plan (PlanApproved)"),
                )
                .suggest("present the contract and wait for the user's plan approval in the cockpit"),
            );
        }
        let mut advanced_to = None;
        if !report.has_errors() {
            // Move it under the plan's lock, against the plan as it is now.
            let moved = self.edit_plan(plan_id, |plan| {
                let Some(c) = plan.component_mut(id) else { return Ok(Err(format!("{id} left the plan while it was being verified"))) };
                if c.stage != planned.stage {
                    return Ok(Err(format!("{id} moved to {:?} while it was being verified; verify again", c.stage)));
                }
                if c.stage == goal {
                    return Ok(Ok(false));
                }
                if !can_transition(c.stage, goal) {
                    return Ok(Err(format!("verified, but {:?} can't move to {goal:?} (approve the design first)", c.stage)));
                }
                c.stage = goal;
                Ok(Ok(true))
            })?;
            match moved {
                Ok(changed) => {
                    advanced_to = Some(goal);
                    if changed {
                        self.emit(ArchEvent::Changed { reason: ChangeReason::StageChanged, plans: vec![plan_id.clone()], components: vec![id.clone()] });
                    }
                }
                Err(why) => report.push(Finding::error(FindingCode::IllegalTransition, Target::Component { id: id.clone() }, why)),
            }
        }
        // Code that no longer holds up loses the stage it had earned.
        if report.has_errors() && planned.stage >= Stage::Skeleton {
            let observed = self.observed()?;
            let demoted = self.edit_plan(plan_id, |plan| {
                let Some(current) = plan.component(id).cloned() else { return Ok(None) };
                let Some(to) = demotion(plan, &current, &report, &observed) else { return Ok(None) };
                // Its design still stands: the plan approval isn't taken back.
                if let Some(c) = plan.component_mut(id) {
                    c.stage = to;
                }
                Ok(Some(to))
            })?;
            if demoted.is_some() {
                self.emit(ArchEvent::Changed { reason: ChangeReason::StageChanged, plans: vec![plan_id.clone()], components: vec![id.clone()] });
            }
        }
        self.emit_validated(plan_id, Some(id), &report);
        Ok(Verification { report, advanced_to })
    }

    /// The code checks for one planned component against `observed`; `deep`
    /// adds its validation tools and reviewers (once the code checks pass).
    fn check(&self, plan: &Plan, planned: &Component, observed: &Graph, goal: Stage, deep: bool) -> Result<Report, ServiceError> {
        let mut report = verify::verify_component(plan, planned, observed, goal, &self.packs, &self.types);
        report.findings.extend(self.rules.check(observed.component(&planned.id), observed, &self.root, &self.packs, &self.types));
        report.extend(chain::recheck(plan, &planned.id, observed));
        let unverified = self.observed.read().map_err(ServiceError::poisoned)?.unverified.clone();
        report.findings.extend(verify::planned_flows(plan, &planned.id, observed, &unverified));
        if deep && !report.has_errors() {
            let merged = Graph::merge(observed.clone(), std::slice::from_ref(plan));
            let component = merged.component(&planned.id).unwrap_or(planned);
            report.extend(self.tools_for(component).report);
            report.extend(self.review_with(plan, component, &merged)?.report);
        }
        Ok(report.sorted())
    }

    fn tools_for(&self, component: &Component) -> ToolsReport {
        let (invocations, mut problems) = tools::invocations(&self.tools, &self.types, component, &self.root);
        let mut out = tools::run(invocations, self.runner.as_ref(), &self.root);
        problems.extend(out.report);
        out.report = problems;
        out
    }

    fn review_with(&self, plan: &Plan, component: &Component, merged: &Graph) -> Result<ReviewsReport, ServiceError> {
        let contract = plan.component(&component.id).map(|p| slice(plan, p, merged, &checks::schema_set(merged, &self.types)).to_markdown()).unwrap_or_default();
        let code = self.component(&component.id, &Source::Plan(plan.id.clone()))?.code;
        let subject = Subject { plan, component, observed: merged, contract, code };
        Ok(review::review(&self.reviewers, &self.types, self.review_provider.as_deref(), &subject))
    }

    /// Runs the validation tools for one planned component, or every planned
    /// component that's in the code.
    pub fn run_tools(&self, plan_id: &PlanId, only: Option<&ComponentId>) -> Result<ToolsReport, ServiceError> {
        let plan = self.plan(plan_id)?;
        let merged = self.graph(&Source::Plan(plan_id.clone()))?;
        let mut out = ToolsReport::default();
        for planned in plan.components.iter().filter(|c| only.is_none_or(|id| &c.id == id)) {
            let component = merged.component(&planned.id).unwrap_or(planned);
            if component.anchor.is_none() {
                if only.is_some() {
                    return Err(ServiceError::InvalidOp(format!("{} isn't in the code yet; tools run on code", planned.id)));
                }
                continue;
            }
            let one = self.tools_for(component);
            out.runs.extend(one.runs);
            out.report.extend(one.report);
        }
        if let Some(id) = only
            && plan.component(id).is_none()
        {
            return Err(ServiceError::NotFound(format!("{id} in plan {plan_id}")));
        }
        out.report = out.report.sorted();
        self.emit_validated(plan_id, only, &out.report);
        Ok(out)
    }

    /// Runs the reviewers that apply to one planned component.
    pub fn review(&self, plan_id: &PlanId, id: &ComponentId) -> Result<ReviewsReport, ServiceError> {
        let plan = self.plan(plan_id)?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?;
        let merged = self.graph(&Source::Plan(plan_id.clone()))?;
        let component = merged.component(id).unwrap_or(planned);
        let mut out = self.review_with(&plan, component, &merged)?;
        out.report = out.report.sorted();
        self.emit_validated(plan_id, Some(id), &out.report);
        Ok(out)
    }

    pub fn component(&self, id: &ComponentId, source: &Source) -> Result<ComponentDetail, ServiceError> {
        let graph = self.graph(source)?;
        let component = graph.component(id).cloned().ok_or_else(|| ServiceError::NotFound(id.to_string()))?;
        let code = component.anchor.as_ref().and_then(|a| {
            let text = std::fs::read_to_string(self.root.join(a.file.as_str())).ok()?;
            let lines: Vec<&str> = text.lines().collect();
            let (start, end) = (a.span.start_line.saturating_sub(1) as usize, (a.span.end_line as usize).min(lines.len()));
            (start < end).then(|| lines[start..end].join("\n"))
        });
        let stale = match &component.anchor {
            Some(a) => self.is_stale(a.file.as_str())?,
            None => false,
        };
        Ok(ComponentDetail {
            alignment: graph.alignment(id).unwrap_or(Alignment::Observed),
            outgoing: graph.outgoing(id, None).cloned().collect(),
            incoming: graph.incoming(id, None).cloned().collect(),
            component,
            code,
            stale,
        })
    }

    /// Every component in `file` (in code or planned there) with the relations
    /// leaving them.
    pub fn file_view(&self, file: &str, source: &Source) -> Result<arch_graph::FileView, ServiceError> {
        let graph = self.graph(source)?;
        let components: Vec<(Component, Alignment)> = graph
            .in_file(file)
            .map(|c| (c.clone(), graph.alignment(&c.id).unwrap_or(Alignment::Observed)))
            .collect();
        let relations = components
            .iter()
            .flat_map(|(c, _)| graph.outgoing(&c.id, None))
            .map(|r| (r.clone(), graph.relation_alignment(r)))
            .collect();
        Ok(arch_graph::FileView { file: file.to_owned(), components, relations, stale: self.is_stale(file)? })
    }

    /// The innermost component whose code contains `line` of `file`.
    pub fn component_at(&self, file: &str, line: u32, source: &Source) -> Result<Component, ServiceError> {
        let graph = self.graph(source)?;
        graph.enclosing(file, line).cloned().ok_or_else(|| ServiceError::NotFound(format!("no component at {file}:{line}")))
    }

    /// One of the three views as a tree, cut to the card budget and laid out.
    pub fn tree(&self, request: &TreeRequest) -> Result<Tree, ServiceError> {
        self.with_tree_input(request, |input| {
            let started = Instant::now();
            let tree = arch_graph::tree::build(input, request);
            self.note_slow("tree", request, started, tree.cards.len());
            Ok(tree)
        })
    }

    /// The cards opening `card` `levels` further adds (only those).
    pub fn expand(&self, request: &TreeRequest, card: &str, levels: u32) -> Result<Tree, ServiceError> {
        self.with_tree_input(request, |input| {
            self.check_card(input, request, card)?;
            Ok(arch_graph::tree::expand(input, request, card, levels))
        })
    }

    /// Where `card` sits: what holds it, what else is in there, its lines to those.
    pub fn context(&self, request: &TreeRequest, card: &str) -> Result<Tree, ServiceError> {
        self.with_tree_input(request, |input| {
            self.check_card(input, request, card)?;
            Ok(arch_graph::tree::context(input, request, card))
        })
    }

    /// Everything about one card: its component's details (a data card's
    /// type, a flow's start or end function).
    pub fn card(&self, key: &str, source: &Source) -> Result<ComponentDetail, ServiceError> {
        let id = match key.split_once(':') {
            Some(("type", name)) => {
                let graph = self.graph(source)?;
                let data = checks::is_data_type(&self.types);
                let mut named = graph.named(name).filter(|c| data(c));
                match (named.next(), named.next()) {
                    (Some(c), None) => c.id.clone(),
                    (Some(_), Some(_)) => return Err(ServiceError::NotFound(format!("{name}: more than one data type has that name"))),
                    _ => return Err(ServiceError::NotFound(format!("{name}: no data type in the code has that name"))),
                }
            }
            Some(("start" | "end", function)) => ComponentId::new(function),
            _ => ComponentId::new(key),
        };
        self.component(&id, source)
    }

    /// NotFound unless the view `request` asks for can show `card`.
    fn check_card(&self, input: &arch_graph::tree::Input<'_, Graph>, request: &TreeRequest, card: &str) -> Result<(), ServiceError> {
        let probe = TreeRequest { root: Some(card.to_owned()), depth: Some(0), opened: Vec::new(), closed: Vec::new(), ..request.clone() };
        if arch_graph::tree::build(input, &probe).cards.is_empty() {
            return Err(ServiceError::NotFound(format!("{card} isn't in the {} view ({})", request.view.name(), request.source)));
        }
        Ok(())
    }

    /// Runs `f` with what building a tree for `request` needs.
    fn with_tree_input<T>(&self, request: &TreeRequest, f: impl FnOnce(&arch_graph::tree::Input<'_, Graph>) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        let graph = self.graph(&request.source)?;
        let plan = match &request.source {
            Source::Plan(id) => Some(self.plan_shared(id)?),
            Source::Code => None,
        };
        // Which other plans change each component (shown as badges).
        let mut other_plans: BTreeMap<ComponentId, Vec<PlanId>> = BTreeMap::new();
        for id in self.plans.list()? {
            if request.source.plan() == Some(&id) {
                continue;
            }
            if let Some(other) = self.plans.get(&id)? {
                for c in other.components.iter().filter(|c| c.status != arch_graph::Status::Existing) {
                    other_plans.entry(c.id.clone()).or_default().push(id.clone());
                }
            }
        }
        let started = Instant::now();
        let limit = std::time::Duration::from_millis(self.limits.tree_ms);
        let out_of_time = move || started.elapsed() > limit;
        let input = arch_graph::tree::Input {
            graph: &*graph,
            plan: plan.as_deref(),
            other_plans: &other_plans,
            config: &self.trees,
            types: &self.types,
            budget: self.limits.cards_per_answer,
            link_budget: self.limits.links_per_answer,
            out_of_time: &out_of_time,
        };
        if let Some(root) = &request.root {
            self.check_card(&input, request, root)?;
        }
        f(&input)
    }

    /// Logs a request that went over its time limit, with what was asked.
    fn note_slow(&self, what: &str, request: &TreeRequest, started: Instant, cards: usize) {
        let ms = started.elapsed().as_millis() as u64;
        if ms > self.limits.slow_request_ms {
            tracing::warn!(ms, cards, view = request.view.name(), source = %request.source, root = ?request.root, depth = ?request.depth, "slow {what}");
        }
    }

    /// Asks clients to show something (the agent driving the user's view).
    pub fn request_view(&self, request: ViewRequest, by: Requester) -> Result<(), ServiceError> {
        // What it points at has to be there: the root in the view asked for,
        // the card to select anywhere in the graph.
        let source = request.source.clone().unwrap_or_default();
        if let Some(root) = &request.root {
            let tree = TreeRequest { view: request.view.unwrap_or_default(), source: source.clone(), root: Some(root.clone()), depth: Some(0), ..TreeRequest::default() };
            self.with_tree_input(&tree, |_| Ok(()))?;
        }
        if let Some(select) = &request.select
            && !select.starts_with("type:")
        {
            let id = ComponentId::new(select.strip_prefix("start:").or(select.strip_prefix("end:")).unwrap_or(select));
            if self.graph(&source)?.component(&id).is_none() {
                return Err(ServiceError::NotFound(format!("{select} ({source})")));
            }
        }
        self.emit(ArchEvent::ViewRequested { request: Box::new(request), by });
        Ok(())
    }
}

/// Planned components already in the code (Skeleton or Implemented) that a
/// change to `changed` files affects: they live in one (before or after the
/// change), or something they're planned to call does, or unplanned code
/// they lean on does.
pub fn affected(plan: &Plan, before: &Graph, after: &Graph, changed: &BTreeSet<&str>) -> Vec<ComponentId> {
    let touches = |id: &ComponentId| {
        let files = [before.component(id).and_then(|c| c.file()), after.component(id).and_then(|c| c.file()), plan.component(id).and_then(|c| c.file())];
        files.into_iter().flatten().any(|f| changed.contains(f.as_str()))
    };
    plan.components
        .iter()
        .filter(|c| c.stage.expects_code())
        .filter(|c| {
            touches(&c.id)
                || plan.relations.iter().any(|r| r.kind == arch_graph::RelationKind::Calls && r.source == c.id && touches(&r.target))
                // Unplanned helpers it leans on changed too.
                || [before, after].into_iter().any(|g| verify::hidden_reach(plan, &c.id, g).iter().any(|(h, _)| touches(h)))
        })
        .map(|c| c.id.clone())
        .collect()
}

/// Files whose current contents verifying `id` depends on: its own (planned
/// and found), its call chain's callers, what it's planned to call, and the
/// other ends of its planned data flows. Vanished files are included. None
/// when a planned callee or flow end that should exist isn't in the index,
/// since its file can't be known then.
fn verification_files(plan: &Plan, id: &ComponentId, merged: &Graph) -> Option<BTreeSet<String>> {
    let mut ids: BTreeSet<&ComponentId> = BTreeSet::from([id]);
    for r in &plan.relations {
        match r.kind {
            arch_graph::RelationKind::Calls if &r.source == id => {
                ids.insert(&r.target);
            }
            arch_graph::RelationKind::DataFlow if &r.source == id || &r.target == id => {
                ids.insert(&r.source);
                ids.insert(&r.target);
            }
            _ => {}
        }
    }
    let mut files: BTreeSet<String> = chain::caller_files(plan, id, merged);
    // The unplanned helpers it leans on (their placeholders count against it).
    files.extend(verify::hidden_reach(plan, id, merged).iter().filter_map(|(h, _)| merged.component(h).and_then(|c| c.file()).map(|f| f.as_str().to_owned())));
    for c in ids {
        let found = merged.component(c).and_then(|c| c.file()).map(|f| f.as_str().to_owned());
        let planned = plan.component(c).and_then(|c| c.file()).map(|f| f.as_str().to_owned());
        if found.is_none() && planned.is_none() {
            return None;
        }
        files.extend(found);
        files.extend(planned);
    }
    Some(files)
}

/// Records in the approval log every design taken back below its plan
/// approval, so that approval no longer counts. (Code falling back while its
/// design stands keeps it.)
fn revoke(plan: &mut Plan, downgrades: &[Downgrade]) {
    let at = now_rfc3339();
    for d in downgrades.iter().filter(|d| d.to < Stage::PlanApproved) {
        plan.revoke(&d.id, d.from, d.to, &at);
    }
}

/// Where a component in the code goes when re-verification finds errors of
/// its own (not a broken hop elsewhere in its chain): back to PlanApproved
/// if its code is gone, Implemented back to Skeleton otherwise.
fn demotion(plan: &Plan, planned: &Component, report: &Report, observed: &Graph) -> Option<Stage> {
    let own = report.findings.iter().any(|f| {
        f.severity == Severity::Error
            && match &f.target {
                Target::Component { id } => id == &planned.id,
                Target::Relation { id } => plan.relations.iter().any(|r| &r.id() == id && r.source == planned.id),
                Target::Plan { .. } => false,
            }
    });
    if !own {
        return None;
    }
    match (planned.stage, observed.component(&planned.id).is_some()) {
        (Stage::Skeleton | Stage::Implemented, false) => Some(Stage::PlanApproved),
        (Stage::Implemented, true) => Some(Stage::Skeleton),
        _ => None,
    }
}
