//! The architecture service: the code (observed, from the index) with plans
//! laid over it. Plans change only through operations; the observed graph
//! changes only through ingestion. Every change is announced to listeners.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Instant;

use arch_store::{SqliteStore, Stamp};

use arch_graph::tree::{Tree, TreeRequest};
use arch_graph::{
    Alignment, Approval, ArchEvent, ChangeReason, Component, ComponentId, Finding, FindingCode, Graph, Plan, PlanId, PlanKind, Report, Requester,
    RepoPath, Severity, Source, Stage, Target, TypeSystem, ViewRequest, can_transition, ComponentDetail, schema::Dsl,
    IndexStatus,
};
use code_analyzer::{AmbiguousCall, CodeRules, Occurrence, PackSet, content_hash};
use serde::{Deserialize, Serialize};

use crate::progress::{Downgrade, cascade};
use crate::import::ImportReport;
use crate::contract::{ContractSlice, slice};
use crate::ops::{PlanOp, apply};
use crate::review::{ReviewProvider, ReviewerCatalog, ReviewsReport, Subject};
use crate::perf::{GraphSize, PerfReport, StoreSize, Timing};
use crate::pipeline::{Core, Pipeline, Snapshot};
use crate::store::PlanRepository;
use crate::tools::{ProcessRunner, ToolCatalog, ToolRunner, ToolsReport};
use crate::{ServiceError, chain, checks, review, tools, verify};

mod reviewed;
mod search;

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
    /// Of those, the files that weren't there before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<RepoPath>,
    /// Files linked again (the changed ones and those using them): what the
    /// languages' servers are asked about.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked: Vec<RepoPath>,
}

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

/// A plan laid over the code, kept until the plan or the code changes.
struct Merged {
    generation: u64,
    plan: Arc<Plan>,
    graph: Arc<Graph>,
    used: Instant,
}

pub struct ArchitectureService {
    /// The languages' servers (none until `with_servers`). Dropped first:
    /// their threads stop before the pipeline does.
    servers: crate::servers::Servers,
    /// The one writer; dropping it waits for its thread.
    pub(crate) pipeline: Pipeline,
    /// What the pipeline thread shares (settings, the store, what readers see).
    pub(crate) core: Arc<Core>,
    dsl: Dsl,
    trees: arch_graph::tree::TreeConfig,
    plans: Box<dyn PlanRepository>,
    /// The repo's file list for searching files and lines (read on first use).
    repo_files: RwLock<Option<arch_search::RepoFiles>>,
    /// Plans laid over the code, by plan (the most recently used few).
    merged: Mutex<HashMap<PlanId, Merged>>,
    tools: ToolCatalog,
    runner: Arc<dyn ToolRunner>,
    reviewers: ReviewerCatalog,
    review_provider: Option<Arc<dyn ReviewProvider>>,
    /// One lock per plan, held across load -> change -> save.
    plan_locks: Mutex<HashMap<PlanId, Arc<Mutex<()>>>>,
    /// Built once per graph (the code's snapshot, or a plan's overlay) and
    /// kept while it's in use.
    flow_indexes: Mutex<Vec<Kept<arch_graph::tree::FlowIndex>>>,
    /// Where the code is entered, found once per snapshot of the code.
    entry_points: Mutex<Vec<Kept<Vec<arch_graph::tree::EntryPoint>>>>,
    symbol_lists: Mutex<Vec<Kept<SymbolList>>>,
    /// How many symbol lists were built (counted for the tests).
    #[cfg(any(test, feature = "testing"))]
    symbol_lists_built: std::sync::atomic::AtomicUsize,
    /// Which other plans change each component (the plan badges), per
    /// version of the plans and plan shown.
    badges: Mutex<Option<(BadgesKey, Badges)>>,
    /// Trees sent to clients (laid out), kept while their graph is: opening
    /// a card merges its branch into the tree the client holds.
    held: Mutex<std::collections::VecDeque<Held>>,
    /// Why a component can't be approved yet for its discussions (the
    /// discussion engine says; set once at start-up).
    discussed: std::sync::OnceLock<Blocker>,
}

/// Why a plan's component can't be approved yet: none when it can.
pub type Blocker = Box<dyn Fn(&PlanId, &ComponentId) -> Option<String> + Send + Sync>;

/// A tree a client holds, by the request it answers.
struct Held {
    graph: std::sync::Weak<Graph>,
    request: TreeRequest,
    tree: Arc<Tree>,
}

/// Something built from one graph, kept while the graph is.
struct Kept<T> {
    graph: std::sync::Weak<Graph>,
    value: Arc<T>,
}

/// What the symbol search matches against, per component of one graph.
pub(crate) struct SymbolList {
    pub ids: Vec<ComponentId>,
    /// Each one's name, and its whole symbol and file, in `ids`' order.
    pub symbols: arch_search::Symbols,
}

/// The plans as they are (each one's id and version) and the plan shown.
/// The plans the badges were built from (held, so none is freed and its
/// address reused while kept), and the plan shown.
type BadgesKey = (Vec<(PlanId, Arc<Plan>)>, Option<PlanId>);

/// The plans that change each component.
type Badges = Arc<BTreeMap<ComponentId, Vec<PlanId>>>;

fn same_badges_key(a: &BadgesKey, b: &BadgesKey) -> bool {
    a.1 == b.1 && a.0.len() == b.0.len() && a.0.iter().zip(&b.0).all(|((ia, pa), (ib, pb))| ia == ib && Arc::ptr_eq(pa, pb))
}

/// The one kept for `graph`, else `build`'s (kept from now on).
fn kept<T>(list: &Mutex<Vec<Kept<T>>>, graph: &Arc<Graph>, build: impl FnOnce() -> T) -> Arc<T> {
    let mut list = list.lock().unwrap_or_else(PoisonError::into_inner);
    list.retain(|k| k.graph.strong_count() > 0);
    if let Some(k) = list.iter().find(|k| std::ptr::eq(k.graph.as_ptr(), Arc::as_ptr(graph))) {
        return k.value.clone();
    }
    let value = Arc::new(build());
    list.push(Kept { graph: Arc::downgrade(graph), value: value.clone() });
    value
}

/// A tree request in words (for timings and the slow log).
fn asked(request: &TreeRequest, card: Option<&str>) -> String {
    let mut out = format!("{} {:?} from {} depth {:?} ({})", request.view.name(), request.way, request.root.as_deref().unwrap_or("the start"), request.depth, request.source);
    if let Some(card) = card {
        out.push_str(&format!(", card {card}"));
    }
    if !request.opened.is_empty() {
        out.push_str(&format!(", {} opened", request.opened.len()));
    }
    if let Some(only) = &request.only {
        out.push_str(&format!(", only {}", only.len()));
    }
    out
}

/// Approving, verifying and building are for plans that change the code; a
/// walkthrough only explains it, and a review is a list of items.
pub fn only_changes(plan: &Plan, what: &str) -> Result<(), ServiceError> {
    match plan.kind {
        PlanKind::Change => Ok(()),
        PlanKind::Walkthrough => Err(ServiceError::InvalidOp(format!("{} is a walkthrough: it explains the code, so nothing in it is {what}", plan.id))),
        PlanKind::Review => Err(ServiceError::InvalidOp(format!("{} is a review: a list of items, with nothing in it to be {what}", plan.id))),
    }
}

/// Now, as plans and the rest write a time (`arch_graph::when`).
pub(crate) fn now_rfc3339() -> String {
    arch_graph::when::now()
}

impl ArchitectureService {
    /// Opens the service over `root`, building the graph from the rows kept
    /// from the last run (no file is read). Code rules start as the
    /// defaults; see `with_rules`.
    pub fn open(root: &Path, packs: PackSet, types: TypeSystem, plans: Box<dyn PlanRepository>, store: SqliteStore) -> Result<Self, ServiceError> {
        // Packs map onto types: every one they name must exist.
        packs.check_types(&types).map_err(|e| ServiceError::Index(e.to_string()))?;
        let rules = CodeRules::defaults(&packs, &types).map_err(|e| ServiceError::Index(e.to_string()))?;
        let core = Core {
            root: root.to_path_buf(),
            ignores: Arc::new(code_analyzer::Ignores::new(root)),
            packs,
            types,
            rules,
            limits: crate::Limits::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in limits: {e}")))?,
            store,
            snapshot: Arc::new(RwLock::new(Arc::new(Snapshot::default()))),
            retired: Mutex::new(Vec::new()),
            status: RwLock::new(IndexStatus::default()),
            listeners: RwLock::new(Vec::new()),
            recent: Mutex::new(crate::perf::Recent::default()),
        };
        Ok(Self {
            core: Arc::new(core),
            pipeline: Pipeline::default(),
            dsl: Dsl::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in schema DSL: {e}")))?,
            trees: arch_graph::tree::TreeConfig::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in tree config: {e}")))?,
            plans,
            servers: crate::servers::Servers::default(),
            repo_files: RwLock::new(None),
            merged: Mutex::new(HashMap::new()),
            tools: ToolCatalog::default(),
            runner: Arc::new(ProcessRunner),
            reviewers: ReviewerCatalog::defaults()?,
            review_provider: None,
            plan_locks: Mutex::new(HashMap::new()),
            flow_indexes: Mutex::new(Vec::new()),
            entry_points: Mutex::new(Vec::new()),
            symbol_lists: Mutex::new(Vec::new()),
            #[cfg(any(test, feature = "testing"))]
            symbol_lists_built: std::sync::atomic::AtomicUsize::new(0),
            badges: Mutex::new(None),
            held: Mutex::default(),
            discussed: std::sync::OnceLock::new(),
        })
    }

    /// Comments plans' files hold (from before discussions) go to `mover`
    /// (the discussion engine), once, as the server starts; each plan's
    /// file is written without them once they've gone.
    pub fn move_comments_to(&self, mover: crate::Mover) -> Result<(), ServiceError> {
        self.plans.move_comments(mover)
    }

    /// Why `id`'s approval waits on discussions not settled, if it does.
    pub fn discussed(&self, plan: &PlanId, id: &ComponentId) -> Option<String> {
        self.discussed.get().and_then(|b| b(plan, id))
    }

    /// Approvals ask `blocker` (the discussion engine) whether a
    /// component's discussions are settled. Set once.
    pub fn approvals_ask(&self, blocker: Blocker) {
        if self.discussed.set(blocker).is_err() {
            tracing::warn!("Approvals already ask the discussions: asked again");
        }
    }

    /// The shared settings, to change while the service is being built
    /// (before the pipeline thread starts).
    fn core_mut(&mut self) -> &mut Core {
        Arc::get_mut(&mut self.core).expect("settings change only while the service is built, before its first change")
    }

    /// Replaces the schema DSL's aliases (a project's own shorthand types).
    pub fn with_dsl(mut self, dsl: Dsl) -> Self {
        self.dsl = dsl;
        self
    }

    /// Replaces the code rules (e.g. defaults plus the project's .nah/rules).
    pub fn with_rules(mut self, rules: CodeRules) -> Self {
        self.core_mut().rules = rules;
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

    fn check_cx<'a>(&'a self, ignores: &'a code_analyzer::Ignores) -> checks::CheckCx<'a> {
        checks::CheckCx { root: &self.core.root, ignores, packs: &self.core.packs, types: &self.core.types, tools: &self.tools }
    }

    /// What ingestion never reads (the one definition of "ignored").
    pub fn ignores(&self) -> Arc<code_analyzer::Ignores> {
        self.core.ignores.clone()
    }

    /// A changed ignore file changes which files search lists: the list is
    /// read again.
    fn ignore_rules_changed(&self, paths: &[PathBuf]) {
        if paths.iter().any(|p| self.core.ignores.is_rule_file(p)) {
            self.forget_files();
        }
    }

    pub fn root(&self) -> &Path {
        &self.core.root
    }

    pub fn types(&self) -> &TypeSystem {
        &self.core.types
    }


    pub fn subscribe(&self, listener: impl Fn(&ArchEvent) + Send + Sync + 'static) {
        if let Ok(mut l) = self.core.listeners.write() {
            l.push(Arc::new(listener));
        }
    }

    fn emit(&self, event: ArchEvent) {
        self.core.emit(event);
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

    /// Every file's rows, as last stored.
    pub fn file_rows(&self) -> Vec<Arc<code_analyzer::rows::FileRows>> {
        self.snapshot().map(|s| s.files.values().cloned().collect()).unwrap_or_default()
    }

    /// What each named place in `file` refers to, as of the last result.
    pub fn occurrences(&self, file: &RepoPath) -> Result<Vec<Occurrence>, ServiceError> {
        Ok(self.snapshot()?.files.get(file).map(|rows| code_analyzer::occurrences(rows)).unwrap_or_default())
    }

    /// One file for the code viewer, in one answer: its text and, for that
    /// same text (by its content hash), its colours and what each named place
    /// refers to, from what's kept. A file whose text isn't what was analysed
    /// (changed since, or never read) comes with its text only, marked stale.
    pub fn code_analysis(&self, file: &str) -> Result<arch_graph::code::CodeAnalysis, ServiceError> {
        self.timed("code", file.to_owned(), |t| {
            let path = RepoPath::new(file).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
            // Only a file inside the repo, however it's linked.
            let root = self.core.root.canonicalize().map_err(|e| ServiceError::Corrupt(format!("repository root: {e}")))?;
            let real = self.core.root.join(path.as_str()).canonicalize().map_err(|e| ServiceError::NotFound(format!("{file}: {e}")))?;
            if !real.starts_with(&root) || !real.is_file() {
                return Err(ServiceError::InvalidOp(format!("{file} isn't a file inside the repository")));
            }
            let text = t.stage("read", || std::fs::read_to_string(&real)).map_err(|e| ServiceError::NotFound(format!("{file}: {e}")))?;
            let hash = content_hash(&text);
            let mut out = arch_graph::code::CodeAnalysis { file: path.as_str().to_owned(), text, content_hash: hash.clone(), ..Default::default() };
            let kept = self.snapshot()?.files.get(&path).map(|rows| rows.file.content_hash.clone());
            out.stale = kept.as_deref() != Some(hash.as_str());
            let graph = self.observed()?;
            out.view = t.stage("components", || file_view_of(&graph, path.as_str(), kept.is_some() && out.stale));
            if out.stale {
                return Ok(out);
            }
            // The colours, from the store on a connection of their own.
            out.highlights = t.stage("highlights", || self.core.store.highlights(&path))?.unwrap_or_default();
            out.occurrences = t.stage("occurrences", || self.occurrences(&path))?;
            let ids: BTreeSet<&ComponentId> = out.occurrences.iter().filter_map(|o| o.target.as_ref()).collect();
            out.targets = ids
                .into_iter()
                .filter_map(|id| graph.component(id))
                .map(|c| arch_graph::code::CodeTarget {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    type_name: c.type_name.clone(),
                    at: c.anchor.as_ref().map(|a| (a.file.as_str().to_owned(), a.span.start_line)),
                    signature: c.signature.as_ref().map(|s| s.written(&c.name)),
                })
                .collect();
            t.size(out.occurrences.len(), "occurrences");
            Ok(out)
        })
    }

    /// Waits up to `timeout` for the languages' servers to be ready and to
    /// have answered for everything linked so far (for runs that want the
    /// confirmed result, like `meta-harness analyze`); false if it timed out.
    pub fn wait_for_servers(&self, timeout: std::time::Duration) -> Result<bool, ServiceError> {
        let done = self.servers.wait(timeout);
        // Their answers are jobs ahead of this one.
        self.pipeline.run(&self.core, |_, _| ())?;
        Ok(done)
    }

    /// Starts each language's server whose pack is installed, in the
    /// background (see `servers`).
    pub fn with_servers(mut self, catalog: &nah_packs::Catalog) -> Self {
        self.servers = crate::servers::Servers::start(catalog, &self.core.packs, &self.core.root, &self.core.limits.servers, self.core.snapshot.clone(), self.pipeline.jobs.clone());
        self
    }

    /// The code as last indexed (shared, never copied).
    fn snapshot(&self) -> Result<Arc<Snapshot>, ServiceError> {
        self.pipeline.open(&self.core)?;
        Ok(self.core.snapshot())
    }

    /// The service's limits (from config).
    pub fn limits(&self) -> &crate::Limits {
        &self.core.limits
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
        self.core_mut().limits = limits;
        self
    }

    /// Re-reads `scope` (a path inside the repo): reads changed files, drops
    /// deleted ones, and links again only what they can affect.
    pub fn ingest(&self, scope: &Path) -> Result<IngestReport, ServiceError> {
        self.ingest_scopes(&[scope.to_path_buf()], false, true)
    }

    /// Rebuilds everything from scratch: the rows are dropped and every
    /// file is read again.
    pub fn reindex(&self) -> Result<IngestReport, ServiceError> {
        self.forget_files();
        self.ingest_scopes(std::slice::from_ref(&self.core.root), true, true)
    }

    /// One job for the pipeline thread (`asked`: someone asked for it, and
    /// hears of it as it starts).
    fn ingest_scopes(&self, scopes: &[PathBuf], fresh: bool, asked: bool) -> Result<IngestReport, ServiceError> {
        let scopes = scopes.to_vec();
        let report = self.pipeline.run(&self.core, move |core, model| crate::pipeline::ingest(core, model, &scopes, fresh, asked))??;
        // The languages' servers hear which files changed (only those), then
        // confirm what was linked again, in the background.
        self.servers.changed(&self.core.packs, &report.changed, &report.created);
        self.servers.confirm(&report.linked);
        Ok(report)
    }

    /// Indexing as clients see it.
    pub fn index_status(&self) -> IndexStatus {
        // Opening the kept rows sets it (a failure says so there).
        let _ = self.pipeline.open(&self.core);
        let mut status = self.core.status.read().map(|s| s.clone()).unwrap_or_default();
        // Live, else as last kept.
        status.servers = match self.servers.is_empty() {
            true => self.core.store.servers().unwrap_or_default(),
            false => self.servers.statuses(),
        };
        status
    }

    /// Changes the status and tells clients.
    fn set_index_status(&self, change: impl FnOnce(&mut IndexStatus)) {
        self.core.set_status(change);
    }

    /// Files changed on disk (the watcher saw them) that haven't been read
    /// since: shown as out of date until they are.
    pub fn mark_stale(&self, paths: &[PathBuf]) -> Result<(), ServiceError> {
        self.ignore_rules_changed(paths);
        let mut changed: Vec<String> = Vec::new();
        let snapshot = self.snapshot()?;
        for path in paths {
            if !self.watches(path)? {
                continue;
            }
            let prefix = self.scope_prefix(path);
            // A file whose content is what was read (touched) isn't out of date.
            let same = RepoPath::new(&prefix).ok().and_then(|p| snapshot.files.get(&p).map(|rows| rows.file.content_hash.clone())).is_some_and(|kept| {
                path.is_file() && std::fs::read_to_string(path).is_ok_and(|text| content_hash(&text) == kept)
            });
            // Gone and never read (nor anything read under it): nothing to redo.
            let gone_unknown = !path.exists() && !snapshot.files.keys().any(|f| f.as_str() == prefix || Core::under(f, &prefix));
            if !prefix.is_empty() && !same && !gone_unknown {
                changed.push(prefix);
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
        let absolute = self.core.root.join(path.as_str());
        let snapshot = self.snapshot()?;
        let Some(rows) = snapshot.files.get(&path) else { return Ok(false) };
        let Ok(meta) = std::fs::metadata(&absolute) else { return Ok(false) };
        if rows.file.modified != 0 && Stamp::of(&meta) == (Stamp { size: rows.file.size, modified: rows.file.modified }) {
            return Ok(false);
        }
        let Ok(content) = std::fs::read_to_string(&absolute) else { return Ok(false) };
        Ok(rows.file.content_hash != content_hash(&content))
    }

    /// `scope` as a repo-relative prefix ("" for the whole repo), spelled the
    /// way index entries are.
    fn scope_prefix(&self, scope: &Path) -> String {
        self.core.scope_prefix(scope)
    }

    /// Reacts to changed paths (from a file watcher, say): re-reads the ones
    /// that are source files the walk wouldn't ignore, then re-verifies only
    /// the planned components (already in the code) that the changed files
    /// affect: their own file, or the file of something they're planned to
    /// call. Re-verification checks the code (location, calls, signature,
    /// call chain); tools and reviewers run on explicit verification only.
    pub fn refresh(&self, paths: &[PathBuf]) -> Result<Refresh, ServiceError> {
        // Plans someone else changed are read again (the only time they are).
        let edited = self.plans.changed(paths)?;
        if !edited.is_empty() {
            self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: edited, components: Vec::new() });
        }
        self.ignore_rules_changed(paths);
        self.files_changed(paths);
        // Files no language reads that a server watches (its project's manifest).
        let not_ignored: Vec<PathBuf> = paths.iter().filter(|p| p.starts_with(&self.core.root) && !self.ignores().is_ignored_as(p, p.is_dir())).cloned().collect();
        self.servers.others_changed(&self.core.packs, &not_ignored);
        let mut scopes: Vec<PathBuf> = Vec::new();
        for p in paths {
            // A changed rule file: what it covers is read again (files it
            // now ignores leave, files it no longer ignores come in).
            if let Some(scope) = self.core.ignores.rules_scope(p) {
                scopes.push(scope);
            } else if self.watches(p)? {
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
        let ingest = self.ingest_scopes(&scopes, false, false)?;
        let changed: BTreeSet<&str> = ingest.changed.iter().map(RepoPath::as_str).collect();
        let observed = self.observed()?;
        let mut reverified = Vec::new();
        let mut downgraded = Vec::new();
        for (plan_id, before) in &before {
            // Under the plan's lock, on the plan as it is now (an approval or
            // edit made while the files were read isn't lost).
            let (sent_back, checked) = self.edit_plan(plan_id, |plan| {
                let after = self.overlay(plan_id, plan)?;
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
                    sent_back.extend(cascade(plan, &after, &self.core.types, id));
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

    /// Whether `path` is a plan's file (the watcher hands those in, though
    /// the walk ignores their folder).
    pub fn is_plan_file(&self, path: &Path) -> bool {
        self.plans.holds(path)
    }

    /// Whether a changed path is something ingestion reads: inside the repo,
    /// not ignored by the walk, and a source file a pack handles, a directory,
    /// or a vanished path with indexed files under it (a renamed directory).
    fn watches(&self, path: &Path) -> Result<bool, ServiceError> {
        if !path.starts_with(&self.core.root) || self.ignores().is_ignored_as(path, path.is_dir()) {
            return Ok(false);
        }
        if path.is_dir() || self.core.packs.for_path(path).is_some() {
            return Ok(true);
        }
        // A vanished directory (renamed or deleted) with indexed files under it.
        if path.exists() {
            return Ok(false);
        }
        let prefix = self.scope_prefix(path);
        Ok(!prefix.is_empty() && self.snapshot()?.files.keys().any(|f| Core::under(f, &prefix)))
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

    /// The code alone, or the code with one plan laid over it.
    pub fn graph(&self, source: &Source) -> Result<Arc<Graph>, ServiceError> {
        match source {
            Source::Code => self.observed(),
            Source::Plan(id) => {
                let plan = self.plan_shared(id)?;
                self.overlay(id, &plan)
            }
        }
    }

    /// The code with `plan` (plan `id` as it is now, or as it's being edited)
    /// laid over it. Each plan's overlay is kept (shared) and, when the plan
    /// changes, laid again by what changed (a thin layer: nothing is copied);
    /// when the code changes, laid again whole. Only the most recently used
    /// few are kept.
    fn overlay(&self, id: &PlanId, plan: &Plan) -> Result<Arc<Graph>, ServiceError> {
        let observed = self.snapshot()?;
        let mut merged = self.merged.lock().unwrap_or_else(PoisonError::into_inner);
        let earlier = merged.get_mut(id).filter(|m| m.generation == observed.generation);
        if let Some(m) = &earlier
            && *m.plan == *plan
        {
            let graph = m.graph.clone();
            if let Some(m) = merged.get_mut(id) {
                m.used = Instant::now();
            }
            return Ok(graph);
        }
        let graph = Arc::new(observed.graph.laid_over(plan, earlier.as_ref().map(|m| (&*m.plan, &*m.graph))));
        merged.insert(id.clone(), Merged { generation: observed.generation, plan: Arc::new(plan.clone()), graph: graph.clone(), used: Instant::now() });
        while merged.len() > self.core.limits.plans_kept_laid_over.max(1) {
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

    pub fn create_plan(&self, id: &str, title: &str, description: &str, kind: PlanKind) -> Result<Plan, ServiceError> {
        let plan_id = PlanId::new(id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        let lock = self.plan_lock(&plan_id);
        let held = lock.lock().unwrap_or_else(PoisonError::into_inner);
        if self.plans.get(&plan_id)?.is_some() {
            return Err(ServiceError::InvalidOp(format!("plan {id} already exists")));
        }
        let mut plan = Plan::new(id, title);
        plan.description = description.to_owned();
        plan.kind = kind;
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
        let (plan, report) = crate::import::import(input, &*self.observed()?, &self.core.root, &self.core.packs, &self.core.types)?;
        self.plans.save(&plan)?;
        drop(held);
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id], components: plan.components.iter().map(|c| c.id.clone()).collect() });
        Ok((plan, report))
    }

    /// `import_registry` from a repo file (relative to the root).
    pub fn import_registry_file(&self, id: &str, title: &str, file: &str, language: Option<&str>) -> Result<(Plan, ImportReport), ServiceError> {
        let path = arch_graph::RepoPath::new(file).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        let json = std::fs::read_to_string(self.core.root.join(path.as_str())).map_err(|e| ServiceError::NotFound(format!("{file}: {e}")))?;
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
        let (plan, touched) = self.edit_plan(plan_id, |plan| {
            let graph = self.overlay(plan_id, plan)?;
            // A walkthrough has no stages to take back.
            let changed = op.design_change().filter(|_| plan.kind == PlanKind::Change).cloned();
            let removes = matches!(op, PlanOp::Remove { .. });
            let mut touched = apply(plan, op, &graph, &self.core.root, &self.ignores(), &self.core.packs, &self.core.types, &self.dsl)?;
            // A design change sends it (if approved) and what builds on it back.
            if let Some(changed) = changed {
                // A removed component's dependents are only visible before removal.
                let basis = if removes { graph } else { self.overlay(plan_id, plan)? };
                let downgrades = cascade(plan, &basis, &self.core.types, &changed);
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
        let mut report = checks::check_plan(&plan, &graph, &self.check_cx(&self.ignores()));
        // Code rules over the plan's scope as it exists in code.
        let scope = self.scope(plan_id)?;
        report.findings.extend(self.rule_findings(scope.iter().filter_map(|id| graph.component(id)), &graph));
        // Logic blocks are tested on their own.
        let planned = plan.components.iter().filter_map(|c| graph.component(&c.id));
        report.findings.extend(tools::run_logic_checks(planned, &self.tools.logic_checks, self.runner.as_ref(), &self.core.root));
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
        let approval = self.edit_plan(plan_id, |plan| {
            only_changes(plan, "approved")?;
            // The plan as it is: approving changes only its stage, which the
            // checks read from the plan.
            let graph = self.overlay(plan_id, plan)?;
            let from = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.stage;
            if !can_transition(from, to) {
                return Err(ServiceError::InvalidOp(format!("can't go from {from:?} to {to:?}")));
            }
            if self.plans.comments_waiting(plan_id) {
                return Err(ServiceError::InvalidOp(format!("plan {plan_id}'s file holds comments from before discussions: they become discussions when the server starts again, and hold its approvals until then")));
            }
            if let Some(blocked) = self.discussed.get().and_then(|b| b(plan_id, id)) {
                return Err(ServiceError::InvalidOp(blocked));
            }
            // Check as if approved (location rules depend on the stage).
            if let Some(c) = plan.component_mut(id) {
                c.stage = to;
            }
            let mut report = checks::check_plan(plan, &graph, &self.check_cx(&self.ignores()));
            // The same blocking set validation shows for it: code rules on its
            // code and its logic steps' own checks.
            report.findings.extend(self.rule_findings(graph.component(id), &graph));
            report.findings.extend(tools::run_logic_checks(graph.component(id).into_iter(), &self.tools.logic_checks, self.runner.as_ref(), &self.core.root));
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
        Ok(slice(&plan, planned, &graph, &checks::schema_set(&graph, &self.core.types)))
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
        only_changes(&plan, "verified")?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.clone();
        // Re-read everything the checks look at, including files that are gone
        // (re-reading a vanished file drops it from the index).
        let merged = self.graph(&Source::Plan(plan_id.clone()))?;
        let scopes: Vec<PathBuf> = match verification_files(&plan, id, &merged) {
            Some(files) => files.iter().map(|f| self.core.root.join(f)).collect(),
            // Something it depends on isn't where the index last saw it: re-read
            // the repo (incrementally: only changed files are parsed).
            None => vec![self.core.root.clone()],
        };
        if !scopes.is_empty() {
            self.ingest_scopes(&scopes, false, true)?;
        }
        let observed = self.observed()?;
        let mut report = self.check(&plan, &planned, &observed, goal, goal == Stage::Implemented)?;
        let merged = self.overlay(plan_id, &plan)?;
        for f in checks::stage_gate(&plan, &merged, &self.core.types, id, goal) {
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
        let mut report = verify::verify_component(plan, planned, observed, goal, &self.core.packs, &self.core.types);
        report.findings.extend(self.rule_findings(observed.component(&planned.id), observed));
        report.extend(chain::recheck(plan, &planned.id, observed));
        let snapshot = self.snapshot()?;
        report.findings.extend(verify::planned_flows(plan, &planned.id, observed, &snapshot.unverified));
        if deep && !report.has_errors() {
            let merged = self.overlay(&plan.id, plan)?;
            let component = merged.component(&planned.id).unwrap_or(planned);
            report.extend(self.tools_for(component).report);
            report.extend(self.review_with(plan, component, &merged)?.report);
        }
        Ok(report.sorted())
    }

    fn tools_for(&self, component: &Component) -> ToolsReport {
        let (invocations, mut problems) = tools::invocations(&self.tools, &self.core.types, component, &self.core.root);
        let mut out = tools::run(invocations, self.runner.as_ref(), &self.core.root);
        problems.extend(out.report);
        out.report = problems;
        out
    }

    fn review_with(&self, plan: &Plan, component: &Component, merged: &Graph) -> Result<ReviewsReport, ServiceError> {
        let contract = plan.component(&component.id).map(|p| slice(plan, p, merged, &checks::schema_set(merged, &self.core.types)).to_markdown()).unwrap_or_default();
        let code = self.component(&component.id, &Source::Plan(plan.id.clone()))?.code;
        let subject = Subject { plan, component, observed: merged, contract, code };
        Ok(review::review(&self.reviewers, &self.core.types, self.review_provider.as_deref(), &subject))
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
            let text = std::fs::read_to_string(self.core.root.join(a.file.as_str())).ok()?;
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
            references: self.references(id)?,
            component,
            code,
            stale,
        })
    }

    /// Every use of `id` in the code, across files: the refs that link to it,
    /// each with its line.
    pub fn references(&self, id: &ComponentId) -> Result<Vec<arch_graph::code::Reference>, ServiceError> {
        let rows = self.core.store.refs_to(id.as_str())?;
        let snapshot = self.snapshot()?;
        let mut out = Vec::with_capacity(rows.len());
        // Each file's line starts, when it's still the text that was read
        // (else where a use is now isn't known).
        let mut lines: Option<(String, Option<Vec<usize>>)> = None;
        for (file, start, end, kind, within, evidence) in rows {
            if lines.as_ref().is_none_or(|(f, _)| *f != file) {
                let kept = arch_graph::RepoPath::new(&file).ok().and_then(|p| snapshot.files.get(&p).map(|r| r.file.content_hash.clone()));
                let starts = std::fs::read_to_string(self.core.root.join(&file))
                    .ok()
                    .filter(|text| kept.as_deref() == Some(code_analyzer::content_hash(text).as_str()))
                    .map(|text| std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect());
                lines = Some((file.clone(), starts));
            }
            let starts = lines.as_ref().and_then(|(_, s)| s.as_ref());
            let line = starts.map_or(0, |starts| starts.partition_point(|s| *s <= start as usize)) as u32;
            out.push(arch_graph::code::Reference { file, line, start, end, kind, within: within.map(ComponentId::new), evidence, stale: starts.is_none() });
        }
        Ok(out)
    }

    /// Every component in `file` (in code or planned there) with the relations
    /// leaving them.
    pub fn file_view(&self, file: &str, source: &Source) -> Result<arch_graph::FileView, ServiceError> {
        Ok(file_view_of(&*self.graph(source)?, file, self.is_stale(file)?))
    }

    /// The innermost component whose code contains `line` of `file`.
    pub fn component_at(&self, file: &str, line: u32, source: &Source) -> Result<Component, ServiceError> {
        let graph = self.graph(source)?;
        graph.enclosing(file, line).cloned().ok_or_else(|| ServiceError::NotFound(format!("no component at {file}:{line}")))
    }

    /// One of the three views as a tree, cut to the card budget and laid out.
    pub fn tree(&self, request: &TreeRequest) -> Result<Tree, ServiceError> {
        self.timed("tree", asked(request, None), |t| self.tree_in(t, request))
    }

    /// The tree, as the JSON sent to the browser (encoding timed too).
    pub fn tree_json(&self, request: &TreeRequest) -> Result<Vec<u8>, ServiceError> {
        self.timed("tree", asked(request, None), |t| {
            let tree = self.tree_in(t, request)?;
            self.encode(t, &tree)
        })
    }

    fn tree_in(&self, t: &mut Timing, request: &TreeRequest) -> Result<Tree, ServiceError> {
        let mut tree = self.with_tree_input(t, request, |t, input| {
            let mut tree = t.stage("walk", || arch_graph::tree::walk_tree(input, request));
            t.stage("layout", || arch_graph::tree::lay_out(&mut tree, &input.config.layout, input.layout_out_of_time));
            Ok(tree)
        })?;
        t.size(tree.cards.len(), "cards");
        tree.slow_ms = self.slow(t);
        t.stage("keep", || self.keep_held(request, &tree))?;
        Ok(tree)
    }

    /// Keeps a tree a client now holds, under what it asked and under the
    /// request the tree answers (the most recent `trees_kept`).
    fn keep_held(&self, asked: &TreeRequest, tree: &Tree) -> Result<(), ServiceError> {
        let graph = Arc::downgrade(&self.graph(&asked.source)?);
        let shared = Arc::new(tree.clone());
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.retain(|h| h.graph.strong_count() > 0 && h.request != *asked && h.request != tree.request);
        held.push_back(Held { graph: graph.clone(), request: asked.clone(), tree: shared.clone() });
        if tree.request != *asked {
            held.push_back(Held { graph, request: tree.request.clone(), tree: shared });
        }
        let most = self.core.limits.trees_kept.max(1) * 2;
        while held.len() > most {
            held.pop_front();
        }
        Ok(())
    }

    /// The tree a client holding `request` was sent, if it's kept for the
    /// graph shown now.
    fn held_tree(&self, request: &TreeRequest) -> Result<Option<Arc<Tree>>, ServiceError> {
        let graph = Arc::downgrade(&self.graph(&request.source)?);
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(held.iter().rev().find(|h| h.request == *request && std::sync::Weak::ptr_eq(&h.graph, &graph)).map(|h| h.tree.clone()))
    }

    /// Opening `card` `levels` further in the tree a client holds (the one
    /// `request` was answered with): only its branch is walked, merged in,
    /// kept to `cards_held`, and the whole laid out again here. What
    /// changed comes back: the new cards, every card's place, the lines.
    pub fn expand(&self, request: &TreeRequest, card: &str, levels: u32) -> Result<arch_graph::tree::Expanded, ServiceError> {
        self.timed("expand", asked(request, Some(card)), |t| self.expand_in(t, request, card, levels))
    }

    pub fn expand_json(&self, request: &TreeRequest, card: &str, levels: u32) -> Result<Vec<u8>, ServiceError> {
        self.timed("expand", asked(request, Some(card)), |t| {
            let change = self.expand_in(t, request, card, levels)?;
            self.encode(t, &change)
        })
    }

    fn expand_in(&self, t: &mut Timing, request: &TreeRequest, card: &str, levels: u32) -> Result<arch_graph::tree::Expanded, ServiceError> {
        let kept = self.held_tree(request)?;
        let cap = self.core.limits.cards_held;
        let (now, mut change) = self.with_tree_input(t, request, |t, input| {
            t.stage("check", || self.check_card(input, request, card))?;
            // A tree not kept (sent before the code changed, or long ago)
            // is walked again first (laid out once, with the branch).
            let held = match kept {
                Some(tree) => tree,
                None => Arc::new(t.stage("held", || arch_graph::tree::walk_tree(input, request))),
            };
            Ok(t.stage("branch", || arch_graph::tree::expand(input, &held, card, levels, cap)))
        })?;
        t.size(change.cards.len(), "cards");
        change.slow_ms = self.slow(t);
        t.stage("keep", || self.keep_held(&now.request, &now))?;
        Ok(change)
    }

    /// Where `card` sits: what holds it, what else is in there, its lines to those.
    pub fn context(&self, request: &TreeRequest, card: &str) -> Result<Tree, ServiceError> {
        self.timed("context", asked(request, Some(card)), |t| self.context_in(t, request, card))
    }

    pub fn context_json(&self, request: &TreeRequest, card: &str) -> Result<Vec<u8>, ServiceError> {
        self.timed("context", asked(request, Some(card)), |t| {
            let tree = self.context_in(t, request, card)?;
            self.encode(t, &tree)
        })
    }

    fn context_in(&self, t: &mut Timing, request: &TreeRequest, card: &str) -> Result<Tree, ServiceError> {
        let mut tree = self.with_tree_input(t, request, |t, input| {
            t.stage("check", || self.check_card(input, request, card))?;
            Ok(t.stage("build", || arch_graph::tree::context(input, request, card)))
        })?;
        t.size(tree.cards.len(), "cards");
        tree.slow_ms = self.slow(t);
        Ok(tree)
    }

    /// An answer as JSON, its encoding timed and its size kept.
    fn encode(&self, t: &mut Timing, value: &impl Serialize) -> Result<Vec<u8>, ServiceError> {
        let bytes = t.stage("encode", || serde_json::to_vec(value)).map_err(|e| ServiceError::Corrupt(e.to_string()))?;
        t.size(bytes.len(), "bytes");
        Ok(bytes)
    }

    /// How long so far, if that's over the slow limit.
    fn slow(&self, t: &Timing) -> Option<u64> {
        let ms = t.elapsed_ms();
        (ms > self.core.limits.slow_request_ms as f64).then(|| ms.ceil() as u64)
    }

    /// Runs `answer` timed, and keeps (and, when slow, logs) its timing.
    pub(crate) fn timed<T>(&self, what: &'static str, asked: String, answer: impl FnOnce(&mut Timing) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        self.core.timed(what, asked, answer)
    }

    /// The code rules' findings on `components`, from the matches kept with
    /// each file's facts (no file is read or parsed again).
    fn rule_findings<'a>(&self, components: impl IntoIterator<Item = &'a Component>, graph: &Graph) -> Vec<arch_graph::Finding> {
        let snapshot = self.core.snapshot();
        let facts = |file: &RepoPath| snapshot.files.get(file).map(|rows| &**rows);
        self.core.rules.check(components, graph, &facts, &self.core.types)
    }

    /// The last answers' timings, the graph's and the store's size, and the
    /// last index run.
    pub fn perf(&self) -> Result<PerfReport, ServiceError> {
        let observed = self.snapshot()?;
        let (components, relations) = observed.graph.len();
        let overlays = self.merged.lock().unwrap_or_else(PoisonError::into_inner).len();
        let files = observed.files.len();
        let recent = self.core.recent.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(PerfReport {
            recent: recent.list(),
            last_index: recent.last_index.clone(),
            graph: GraphSize { components, relations, overlays },
            store: StoreSize { files, bytes: self.core.store.bytes() },
            memory_bytes: memory_stats::memory_stats().map(|m| m.physical_mem as u64),
            slow_request_ms: self.core.limits.slow_request_ms,
        })
    }

    /// Everything about one card: its component's details (a data card's
    /// type, a flow's start or end function).
    pub fn card(&self, key: &str, source: &Source) -> Result<ComponentDetail, ServiceError> {
        self.timed("card", format!("{key} ({source})"), |t| t.stage("details", || self.card_in(key, source)))
    }

    fn card_in(&self, key: &str, source: &Source) -> Result<ComponentDetail, ServiceError> {
        let id = match key.split_once(':') {
            Some(("type", name)) => {
                let graph = self.graph(source)?;
                let data = checks::is_data_type(&self.core.types);
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
        // The card alone, walked and not laid out.
        let probe = TreeRequest { root: Some(card.to_owned()), depth: Some(0), opened: Vec::new(), closed: Vec::new(), ..request.clone() };
        if arch_graph::tree::walk_tree(input, &probe).cards.is_empty() {
            return Err(ServiceError::NotFound(format!("{card} isn't in the {} view ({})", request.view.name(), request.source)));
        }
        Ok(())
    }

    /// Runs `f` with what building a tree for `request` needs.
    fn with_tree_input<T>(&self, t: &mut Timing, request: &TreeRequest, f: impl FnOnce(&mut Timing, &arch_graph::tree::Input<'_, Graph>) -> Result<T, ServiceError>) -> Result<T, ServiceError> {
        // The graph shown, and where the code is entered (found once per
        // snapshot of the code).
        let (graph, entries) = t.stage("source", || -> Result<_, ServiceError> {
            let code = self.snapshot()?;
            let entries = kept(&self.entry_points, &code.graph, || code_analyzer::entry_points(code.files.values().map(|f| &**f), &self.core.packs));
            Ok((self.graph(&request.source)?, entries))
        })?;
        let plan = match &request.source {
            Source::Plan(id) => Some(self.plan_shared(id)?),
            Source::Code => None,
        };
        let other_plans = self.badges(t, request.source.plan())?;
        let started = Instant::now();
        let limit = std::time::Duration::from_millis(self.core.limits.tree_ms);
        let out_of_time = move || started.elapsed() > limit;
        // The layout's own limit, counted from when it first asks.
        let layout_started = std::cell::OnceCell::new();
        let layout_limit = std::time::Duration::from_millis(self.core.limits.layout_ms);
        let layout_out_of_time = move || layout_started.get_or_init(Instant::now).elapsed() > layout_limit;
        let mut input = arch_graph::tree::Input {
            graph: &*graph,
            plan: plan.as_deref(),
            other_plans: &other_plans,
            config: &self.trees,
            types: &self.core.types,
            budget: self.core.limits.cards_per_answer,
            link_budget: self.core.limits.links_per_answer,
            out_of_time: &out_of_time,
            layout_out_of_time: &layout_out_of_time,
            flows: None,
            entries: &entries,
        };
        if request.view == arch_graph::tree::ViewKind::DataFlow {
            input.flows = Some(kept(&self.flow_indexes, &graph, || t.stage("flow index", || arch_graph::tree::flow_index(&input))));
        }
        if let Some(root) = &request.root {
            t.stage("check", || self.check_card(&input, request, root))?;
        }
        f(t, &input)
    }

    /// Which other plans than `shown` change each component (the badges),
    /// built once per version of the plans.
    fn badges(&self, t: &mut Timing, shown: Option<&PlanId>) -> Result<Badges, ServiceError> {
        let mut plans = Vec::new();
        for id in self.plans.list()? {
            if let Some(plan) = self.plans.get(&id)? {
                plans.push((id, plan));
            }
        }
        let key: BadgesKey = (plans.clone(), shown.cloned());
        let mut kept = self.badges.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((k, badges)) = kept.as_ref()
            && same_badges_key(k, &key)
        {
            return Ok(badges.clone());
        }
        let badges = Arc::new(t.stage("badges", || {
            let mut other_plans: BTreeMap<ComponentId, Vec<PlanId>> = BTreeMap::new();
            for (id, plan) in plans.iter().filter(|(id, _)| Some(id) != shown) {
                for c in plan.components.iter().filter(|c| c.status != arch_graph::Status::Existing) {
                    other_plans.entry(c.id.clone()).or_default().push(id.clone());
                }
            }
            other_plans
        }));
        *kept = Some((key, badges.clone()));
        Ok(badges)
    }

    /// How many symbol lists this service built (one per snapshot searched).
    #[cfg(any(test, feature = "testing"))]
    pub fn symbol_lists_built(&self) -> usize {
        self.symbol_lists_built.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// What the symbol search matches against in `graph`, built once per graph.
    pub(crate) fn symbol_list(&self, graph: &Arc<Graph>) -> Arc<SymbolList> {
        kept(&self.symbol_lists, graph, || {
            #[cfg(any(test, feature = "testing"))]
            self.symbol_lists_built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let components: Vec<&Component> = graph.components().collect();
            SymbolList {
                ids: components.iter().map(|c| c.id.clone()).collect(),
                symbols: arch_search::Symbols {
                    names: components.iter().map(|c| c.name.clone()).collect(),
                    qualified: components.iter().map(|c| format!("{} {}", search::symbol(c), c.file().map(|f| f.as_str()).unwrap_or_default())).collect(),
                },
            }
        })
    }

    /// Asks clients to show something (the agent driving the user's view).
    pub fn request_view(&self, request: ViewRequest, by: Requester) -> Result<(), ServiceError> {
        // What it points at has to be there: the root in the view asked for,
        // the card to select anywhere in the graph.
        let source = request.source.clone().unwrap_or_default();
        if let Some(root) = &request.root {
            let tree = TreeRequest { view: request.view.unwrap_or_default(), source: source.clone(), root: Some(root.clone()), depth: Some(0), ..TreeRequest::default() };
            self.timed("navigate", asked(&tree, None), |t| self.with_tree_input(t, &tree, |_, _| Ok(())))?;
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
/// The components in `file` and the relations leaving them, with how plan
/// and code line up.
fn file_view_of(graph: &Graph, file: &str, stale: bool) -> arch_graph::FileView {
    let components: Vec<(Component, Alignment)> = graph.in_file(file).map(|c| (c.clone(), graph.alignment(&c.id).unwrap_or(Alignment::Observed))).collect();
    let relations = components.iter().flat_map(|(c, _)| graph.outgoing(&c.id, None)).map(|r| (r.clone(), graph.relation_alignment(r))).collect();
    arch_graph::FileView { file: file.to_owned(), components, relations, stale }
}

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
