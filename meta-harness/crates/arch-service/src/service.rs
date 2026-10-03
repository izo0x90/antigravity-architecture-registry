//! The architecture service: the code (observed, from the index) with plans
//! laid over it. Plans change only through operations; the observed graph
//! changes only through ingestion. Every change is announced to listeners.

use std::collections::BTreeSet;
use std::path::{Component as PathPart, Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arch_graph::view::{layout, query, render_text};
use arch_graph::{
    Alignment, Approval, ArchEvent, ChangeReason, Component, ComponentId, Finding, FindingCode, Graph, Plan, PlanId, Report, Requester,
    RepoPath, Severity, Stage, Target, TypeSystem, ViewCatalog, ViewRequest, ViewSpec, can_transition, ComponentDetail, ViewResult, schema::Dsl,
};
use arch_index::{AmbiguousCall, CodeRules, ExternalCall, FileExtract, PackSet, content_hash, extract_file, resolve, walk, UnverifiedFlow};
use serde::{Deserialize, Serialize};

use crate::progress::{Downgrade, cascade};
use crate::import::ImportReport;
use crate::contract::{ContractSlice, slice};
use crate::ops::{PlanOp, apply};
use crate::review::{ReviewProvider, ReviewerCatalog, ReviewsReport, Subject};
use crate::store::{CodeIndexStore, PlanRepository};
use crate::tools::{ProcessRunner, ToolCatalog, ToolRunner, ToolsReport};
use crate::{ServiceError, chain, checks, review, tools, verify};

mod comments;
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
    pub cached: usize,
    pub removed: usize,
    pub components: usize,
    pub relations: usize,
    pub ambiguous: usize,
    pub external: usize,
    pub millis: u64,
    /// Files whose content changed (parsed anew or removed).
    #[serde(default)]
    pub changed: Vec<RepoPath>,
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
}

/// What a refresh read and which components it re-verified.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Refresh {
    pub ingest: IngestReport,
    pub reverified: Vec<Reverified>,
    /// Planned components sent back because code they build on changed shape.
    pub downgraded: Vec<(PlanId, Downgrade)>,
}

#[derive(Default)]
struct Observed {
    graph: Graph,
    ambiguous: Vec<AmbiguousCall>,
    external: Vec<ExternalCall>,
    /// Data handed from one call to another that types couldn't confirm.
    unverified: Vec<UnverifiedFlow>,
}

type Listener = Arc<dyn Fn(&ArchEvent) + Send + Sync>;

pub struct ArchitectureService {
    root: PathBuf,
    packs: PackSet,
    types: TypeSystem,
    dsl: Dsl,
    views: ViewCatalog,
    rules: CodeRules,
    plans: Box<dyn PlanRepository>,
    index: Box<dyn CodeIndexStore>,
    observed: RwLock<Observed>,
    listeners: RwLock<Vec<Listener>>,
    tools: ToolCatalog,
    runner: Arc<dyn ToolRunner>,
    reviewers: ReviewerCatalog,
    review_provider: Option<Arc<dyn ReviewProvider>>,
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
    /// Opens the service over `root`, loading the cached index (no parsing).
    /// Code rules start as the defaults; see `with_rules`.
    pub fn open(root: &Path, packs: PackSet, types: TypeSystem, views: ViewCatalog, plans: Box<dyn PlanRepository>, index: Box<dyn CodeIndexStore>) -> Result<Self, ServiceError> {
        let rules = CodeRules::defaults(&packs, &types).map_err(|e| ServiceError::Index(e.to_string()))?;
        let service = Self {
            root: root.to_path_buf(),
            packs,
            types,
            dsl: Dsl::defaults().map_err(|e| ServiceError::Corrupt(format!("built-in schema DSL: {e}")))?,
            views,
            rules,
            plans,
            index,
            observed: RwLock::new(Observed::default()),
            listeners: RwLock::new(Vec::new()),
            tools: ToolCatalog::default(),
            runner: Arc::new(ProcessRunner),
            reviewers: ReviewerCatalog::defaults()?,
            review_provider: None,
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

    pub fn views(&self) -> &ViewCatalog {
        &self.views
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

    fn rebuild_observed(&self) -> Result<(), ServiceError> {
        let extracts: Vec<FileExtract> = self.index.all()?;
        let resolved = resolve(&extracts, &self.packs, &self.types);
        let mut observed = self.observed.write().map_err(ServiceError::poisoned)?;
        *observed = Observed { graph: resolved.graph, ambiguous: resolved.ambiguous, external: resolved.external, unverified: resolved.unverified_flows };
        Ok(())
    }

    /// Re-reads `scope` (a path inside the repo): parses changed files, drops
    /// deleted ones, then re-resolves the whole graph.
    pub fn ingest(&self, scope: &Path) -> Result<IngestReport, ServiceError> {
        self.ingest_scopes(&[scope.to_path_buf()])
    }

    fn ingest_scopes(&self, scopes: &[PathBuf]) -> Result<IngestReport, ServiceError> {
        let started = Instant::now();
        let mut report = IngestReport::default();
        for scope in scopes {
            self.index_scope(scope, &mut report)?;
        }
        self.rebuild_observed()?;
        let observed = self.observed.read().map_err(ServiceError::poisoned)?;
        (report.components, report.relations) = observed.graph.len();
        report.ambiguous = observed.ambiguous.len();
        report.external = observed.external.len();
        report.millis = started.elapsed().as_millis() as u64;
        drop(observed);
        self.emit(ArchEvent::Changed { reason: ChangeReason::Ingested, plans: Vec::new(), components: Vec::new() });
        Ok(report)
    }

    /// Brings the index up to date for one scope (a scope that no longer
    /// exists just drops what was indexed under it).
    fn index_scope(&self, scope: &Path, report: &mut IngestReport) -> Result<(), ServiceError> {
        let files = match scope.exists() {
            true => walk(&self.root, scope, &self.packs).map_err(|e| ServiceError::Index(e.to_string()))?,
            false => Vec::new(),
        };
        report.files += files.len();
        let mut seen = BTreeSet::new();
        for file in &files {
            let Some(pack) = self.packs.by_name(&file.language) else { continue };
            let content = std::fs::read_to_string(&file.absolute).map_err(|e| ServiceError::Io(format!("{}: {e}", file.path)))?;
            let hash = content_hash(&content);
            seen.insert(file.path.clone());
            if let Some(cached) = self.index.get(&file.path, &hash)?
                && cached.module == file.module
            {
                report.cached += 1;
                continue;
            }
            self.index.put(&extract_file(pack, file.path.clone(), file.module.clone(), &content))?;
            report.parsed += 1;
            report.changed.push(file.path.clone());
        }
        // Files under the scope that are gone (or now ignored) leave the index.
        let prefix = scope.strip_prefix(&self.root).unwrap_or(scope).to_string_lossy().trim_start_matches("./").trim_start_matches('.').to_owned();
        for extract in self.index.all()? {
            let under = prefix.is_empty() || extract.file.as_str() == prefix || extract.file.as_str().starts_with(&format!("{}/", prefix.trim_end_matches('/')));
            if under && !seen.contains(&extract.file) {
                self.index.remove(&extract.file)?;
                report.removed += 1;
                report.changed.push(extract.file.clone());
            }
        }
        Ok(())
    }

    /// Reacts to changed paths (from a file watcher, say): re-reads the ones
    /// that are source files the walk wouldn't ignore, then re-verifies only
    /// the planned components (already in the code) that the changed files
    /// affect: their own file, or the file of something they're planned to
    /// call. Re-verification checks the code (location, calls, signature,
    /// call chain); tools and reviewers run on explicit verification only.
    pub fn refresh(&self, paths: &[PathBuf]) -> Result<Refresh, ServiceError> {
        let scopes: Vec<PathBuf> = paths.iter().filter(|p| self.watches(p)).cloned().collect();
        if scopes.is_empty() {
            return Ok(Refresh::default());
        }
        let plans = self.plans()?;
        let before: Vec<Graph> = {
            let observed = self.observed()?;
            plans.iter().map(|p| Graph::merge(observed.clone(), std::slice::from_ref(p))).collect()
        };
        let ingest = self.ingest_scopes(&scopes)?;
        let changed: BTreeSet<&str> = ingest.changed.iter().map(RepoPath::as_str).collect();
        let observed = self.observed()?;
        let mut reverified = Vec::new();
        let mut downgraded = Vec::new();
        for (plan, before) in plans.iter().zip(&before) {
            let after = Graph::merge(observed.clone(), std::slice::from_ref(plan));
            // Existing code the plan builds on changed its signature: what
            // was designed against the old one needs another look.
            let mut plan = plan.clone();
            let reshaped: Vec<ComponentId> = observed
                .components()
                .filter(|c| plan.component(&c.id).is_none())
                .filter(|c| before.component(&c.id).is_some_and(|b| b.signature.is_some() && b.signature != c.signature))
                .map(|c| c.id.clone())
                .collect();
            let mut sent_back = Vec::new();
            for id in &reshaped {
                sent_back.extend(cascade(&mut plan, &after, &self.types, id));
            }
            if !sent_back.is_empty() {
                self.plans.save(&plan)?;
                self.emit(ArchEvent::Changed { reason: ChangeReason::StageChanged, plans: vec![plan.id.clone()], components: sent_back.iter().map(|d| d.id.clone()).collect() });
                downgraded.extend(sent_back.into_iter().map(|d| (plan.id.clone(), d)));
            }
            let plan = &plan;
            for id in affected(plan, before, &after, &changed) {
                let Some(planned) = plan.component(&id) else { continue };
                let report = self.check(plan, planned, &observed, planned.stage, false)?;
                self.emit_validated(&plan.id, Some(&id), &report);
                reverified.push(Reverified { plan: plan.id.clone(), component: id, stage: planned.stage, report });
            }
        }
        Ok(Refresh { ingest, reverified, downgraded })
    }

    /// Whether a changed path is something ingestion reads: a source file a
    /// pack handles (or a vanished one), inside the repo, not hidden and not
    /// git-ignored.
    fn watches(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else { return false };
        if relative.components().any(|c| matches!(c, PathPart::Normal(n) if n.to_string_lossy().starts_with('.'))) {
            return false;
        }
        if path.is_dir() || self.packs.for_path(path).is_none() {
            return false;
        }
        // Every .gitignore from the root down to the file's directory.
        let mut dir = self.root.clone();
        let mut dirs = vec![dir.clone()];
        for part in relative.parent().map(|p| p.components().collect::<Vec<_>>()).unwrap_or_default() {
            dir.push(part);
            dirs.push(dir.clone());
        }
        !dirs.iter().any(|d| {
            let file = d.join(".gitignore");
            file.is_file() && ignore::gitignore::Gitignore::new(&file).0.matched_path_or_any_parents(path, false).is_ignore()
        })
    }

    pub fn observed(&self) -> Result<Graph, ServiceError> {
        Ok(self.observed.read().map_err(ServiceError::poisoned)?.graph.clone())
    }

    pub fn ambiguous_calls(&self) -> Result<Vec<AmbiguousCall>, ServiceError> {
        Ok(self.observed.read().map_err(ServiceError::poisoned)?.ambiguous.clone())
    }

    /// The code with one plan (or all plans) laid over it.
    pub fn graph(&self, plan: Option<&PlanId>) -> Result<Graph, ServiceError> {
        let plans: Vec<Plan> = match plan {
            Some(id) => vec![self.plan(id)?],
            None => self.plans()?,
        };
        Ok(Graph::merge(self.observed()?, &plans))
    }

    pub fn plans(&self) -> Result<Vec<Plan>, ServiceError> {
        self.plans.list()?.iter().filter_map(|id| self.plans.get(id).transpose()).collect()
    }

    pub fn plan(&self, id: &PlanId) -> Result<Plan, ServiceError> {
        self.plans.get(id)?.ok_or_else(|| ServiceError::NotFound(format!("plan {id}")))
    }

    pub fn create_plan(&self, id: &str, title: &str, description: &str) -> Result<Plan, ServiceError> {
        let plan_id = PlanId::new(id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        if self.plans.get(&plan_id)?.is_some() {
            return Err(ServiceError::InvalidOp(format!("plan {id} already exists")));
        }
        let mut plan = Plan::new(id, title);
        plan.description = description.to_owned();
        self.plans.save(&plan)?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id], components: Vec::new() });
        Ok(plan)
    }

    /// Creates plan `id` from the old registry document (`json`), matched
    /// against the code. `language` places components that have neither a
    /// location nor code to match.
    pub fn import_registry(&self, id: &str, title: &str, json: &str, language: Option<&str>) -> Result<(Plan, ImportReport), ServiceError> {
        let plan_id = PlanId::new(id).map_err(|e| ServiceError::InvalidOp(e.to_string()))?;
        if self.plans.get(&plan_id)?.is_some() {
            return Err(ServiceError::InvalidOp(format!("plan {id} already exists")));
        }
        let input = crate::import::ImportInput { plan: plan_id.clone(), title: title.to_owned(), json, language: language.map(str::to_owned) };
        let (plan, report) = crate::import::import(input, &self.observed()?, &self.root, &self.packs, &self.types)?;
        self.plans.save(&plan)?;
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
        self.plans.delete(id)?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![id.clone()], components: Vec::new() });
        Ok(())
    }

    /// Applies one operation to a plan.
    pub fn apply(&self, plan_id: &PlanId, op: PlanOp) -> Result<Plan, ServiceError> {
        let mut plan = self.plan(plan_id)?;
        let observed = self.observed()?;
        let graph = Graph::merge(observed.clone(), std::slice::from_ref(&plan));
        let changed = op.design_change().cloned();
        let removes = matches!(op, PlanOp::Remove { .. });
        let mut touched = apply(&mut plan, op, &graph, &self.root, &self.packs, &self.types, &self.dsl)?;
        // A design change sends it (if approved) and what builds on it back.
        if let Some(changed) = changed {
            // A removed component's dependents are only visible before removal.
            let basis = if removes { graph } else { Graph::merge(observed, std::slice::from_ref(&plan)) };
            touched.extend(cascade(&mut plan, &basis, &self.types, &changed).into_iter().map(|d| d.id));
        }
        self.plans.save(&plan)?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::PlanEdited, plans: vec![plan_id.clone()], components: touched });
        Ok(plan)
    }

    /// The components a plan covers: its own plus referenced neighborhoods.
    pub fn scope(&self, plan_id: &PlanId) -> Result<Vec<ComponentId>, ServiceError> {
        let plan = self.plan(plan_id)?;
        let graph = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
        let mut ids: BTreeSet<ComponentId> = plan.components.iter().map(|c| c.id.clone()).collect();
        for reference in &plan.references {
            ids.insert(reference.id.clone());
            if reference.depth > 0 && !reference.kinds.is_empty() {
                let spec = ViewSpec {
                    name: String::new(),
                    description: String::new(),
                    kinds: reference.kinds.clone(),
                    direction: arch_graph::view::Direction::Out,
                    depth: reference.depth,
                    layout: arch_graph::view::LayoutKind::Layered,
                    overlays: Vec::new(),
                    include_ancestors: false,
                    roots: vec![reference.id.clone()],
                    filters: Default::default(),
                };
                ids.extend(query(&graph, &spec).nodes.into_iter().map(|n| n.id));
            }
        }
        Ok(ids.into_iter().collect())
    }

    /// Plan checks (before any code is written).
    pub fn validate(&self, plan_id: &PlanId) -> Result<Report, ServiceError> {
        let plan = self.plan(plan_id)?;
        let graph = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
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
        let mut plan = self.plan(plan_id)?;
        let from = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.stage;
        if !can_transition(from, to) {
            return Err(ServiceError::InvalidOp(format!("can't go from {from:?} to {to:?}")));
        }
        if let Some(blocked) = approval_blocker(&plan, id) {
            return Err(ServiceError::InvalidOp(blocked));
        }
        // Check as if approved (location rules depend on the stage).
        if let Some(c) = plan.component_mut(id) {
            c.stage = to;
        }
        let graph = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
        let report = checks::check_plan(&plan, &graph, &self.check_cx());
        let blocking: Vec<String> = report.errors_for(id).map(|f| f.message.clone()).collect();
        if !blocking.is_empty() {
            return Err(ServiceError::InvalidOp(format!("{id} can't be approved yet: {}", blocking.join("; "))));
        }
        let approval = Approval { component: id.clone(), from, to, by: name.clone(), at: now_rfc3339() };
        plan.approvals.push(approval.clone());
        self.plans.save(&plan)?;
        self.emit(ArchEvent::Changed { reason: ChangeReason::Approved, plans: vec![plan_id.clone()], components: vec![id.clone()] });
        Ok(approval)
    }

    pub fn contract(&self, plan_id: &PlanId, id: &ComponentId) -> Result<ContractSlice, ServiceError> {
        let plan = self.plan(plan_id)?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?;
        let graph = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
        Ok(slice(&plan, planned, &graph))
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
        let mut plan = self.plan(plan_id)?;
        let planned = plan.component(id).ok_or_else(|| ServiceError::NotFound(format!("{id} in plan {plan_id}")))?.clone();
        let merged = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
        let mut files: BTreeSet<String> = chain::caller_files(&plan, id, &merged);
        files.extend(planned.file().map(|f| f.as_str().to_owned()));
        let scopes: Vec<PathBuf> = files.iter().map(|f| self.root.join(f)).filter(|p| p.exists()).collect();
        if !scopes.is_empty() {
            self.ingest_scopes(&scopes)?;
        }
        let observed = self.observed()?;
        let mut report = self.check(&plan, &planned, &observed, goal, goal == Stage::Implemented)?;
        let merged = Graph::merge(observed.clone(), std::slice::from_ref(&plan));
        for f in checks::stage_gate(&plan, &merged, &self.types, id, goal) {
            report.push(f);
        }
        let mut advanced_to = None;
        if !report.has_errors() {
            if planned.stage == goal {
                advanced_to = Some(goal);
            } else if can_transition(planned.stage, goal) {
                if let Some(c) = plan.component_mut(id) {
                    c.stage = goal;
                }
                self.plans.save(&plan)?;
                advanced_to = Some(goal);
                self.emit(ArchEvent::Changed { reason: ChangeReason::StageChanged, plans: vec![plan_id.clone()], components: vec![id.clone()] });
            } else {
                report.push(Finding::error(
                    FindingCode::IllegalTransition,
                    Target::Component { id: id.clone() },
                    format!("verified, but {:?} can't move to {goal:?} (approve the design first)", planned.stage),
                ));
            }
        }
        self.emit_validated(plan_id, Some(id), &report);
        Ok(Verification { report, advanced_to })
    }

    /// The code checks for one planned component against `observed`; `deep`
    /// adds its validation tools and reviewers (once the code checks pass).
    fn check(&self, plan: &Plan, planned: &Component, observed: &Graph, goal: Stage, deep: bool) -> Result<Report, ServiceError> {
        let mut report = verify::verify_component(plan, planned, observed, goal);
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
        let contract = plan.component(&component.id).map(|p| slice(plan, p, merged).to_markdown()).unwrap_or_default();
        let code = self.component(&component.id, Some(&plan.id))?.code;
        let subject = Subject { plan, component, observed: merged, contract, code };
        Ok(review::review(&self.reviewers, &self.types, self.review_provider.as_deref(), &subject))
    }

    /// Runs the validation tools for one planned component, or every planned
    /// component that's in the code.
    pub fn run_tools(&self, plan_id: &PlanId, only: Option<&ComponentId>) -> Result<ToolsReport, ServiceError> {
        let plan = self.plan(plan_id)?;
        let merged = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
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
        let merged = Graph::merge(self.observed()?, std::slice::from_ref(&plan));
        let component = merged.component(id).unwrap_or(planned);
        let mut out = self.review_with(&plan, component, &merged)?;
        out.report = out.report.sorted();
        self.emit_validated(plan_id, Some(id), &out.report);
        Ok(out)
    }

    pub fn view(&self, spec: &ViewSpec, plan: Option<&PlanId>) -> Result<ViewResult, ServiceError> {
        let graph = self.graph(plan)?;
        let view = query(&graph, spec);
        let layout = layout(&view);
        let text = render_text(&graph, &view);
        let components = view.nodes.iter().filter_map(|n| graph.component(&n.id).map(|c| (n.id.clone(), c.clone()))).collect();
        Ok(ViewResult { view, layout, text, components })
    }

    pub fn named_view(&self, name: &str, plan: Option<&PlanId>, roots: Vec<ComponentId>, depth: Option<u32>) -> Result<ViewResult, ServiceError> {
        let mut spec = self.views.get(name).cloned().ok_or_else(|| ServiceError::NotFound(format!("view '{name}'")))?;
        if !roots.is_empty() {
            spec.roots = roots;
        }
        if let Some(depth) = depth {
            spec.depth = depth;
        }
        self.view(&spec, plan)
    }

    pub fn component(&self, id: &ComponentId, plan: Option<&PlanId>) -> Result<ComponentDetail, ServiceError> {
        let graph = self.graph(plan)?;
        let component = graph.component(id).cloned().ok_or_else(|| ServiceError::NotFound(id.to_string()))?;
        let code = component.anchor.as_ref().and_then(|a| {
            let text = std::fs::read_to_string(self.root.join(a.file.as_str())).ok()?;
            let lines: Vec<&str> = text.lines().collect();
            let (start, end) = (a.span.start_line.saturating_sub(1) as usize, (a.span.end_line as usize).min(lines.len()));
            (start < end).then(|| lines[start..end].join("\n"))
        });
        Ok(ComponentDetail {
            alignment: graph.alignment(id).unwrap_or(Alignment::Observed),
            outgoing: graph.outgoing(id, None).cloned().collect(),
            incoming: graph.incoming(id, None).cloned().collect(),
            component,
            code,
        })
    }

    /// Components whose name, symbol or file matches `query` (case-insensitive).
    pub fn search(&self, query: &str, plan: Option<&PlanId>, limit: usize) -> Result<Vec<Component>, ServiceError> {
        let graph = self.graph(plan)?;
        let q = query.to_lowercase();
        let mut hits: Vec<(usize, Component)> = graph
            .components()
            .filter_map(|c| {
                let symbol = c.anchor.as_ref().map(|a| a.symbol.clone()).or(c.proposed.as_ref().map(|p| p.symbol.clone())).unwrap_or_default();
                let file = c.file().map(|f| f.as_str().to_owned()).unwrap_or_default();
                let rank = if c.name.to_lowercase() == q {
                    0
                } else if c.name.to_lowercase().contains(&q) {
                    1
                } else if symbol.to_lowercase().contains(&q) {
                    2
                } else if file.to_lowercase().contains(&q) {
                    3
                } else {
                    return None;
                };
                Some((rank, c.clone()))
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
        Ok(hits.into_iter().take(limit).map(|(_, c)| c).collect())
    }

    /// Every component in `file` (in code or planned there) with the relations
    /// leaving them.
    pub fn file_view(&self, file: &str, plan: Option<&PlanId>) -> Result<arch_graph::FileView, ServiceError> {
        let graph = self.graph(plan)?;
        let components: Vec<(Component, Alignment)> = graph
            .in_file(file)
            .map(|c| (c.clone(), graph.alignment(&c.id).unwrap_or(Alignment::Observed)))
            .collect();
        let relations = components
            .iter()
            .flat_map(|(c, _)| graph.outgoing(&c.id, None))
            .map(|r| (r.clone(), graph.relation_alignment(r)))
            .collect();
        Ok(arch_graph::FileView { file: file.to_owned(), components, relations })
    }

    /// The innermost component whose code contains `line` of `file`.
    pub fn component_at(&self, file: &str, line: u32, plan: Option<&PlanId>) -> Result<Component, ServiceError> {
        let graph = self.graph(plan)?;
        graph.enclosing(file, line).cloned().ok_or_else(|| ServiceError::NotFound(format!("no component at {file}:{line}")))
    }

    /// Asks clients to show something (the agent driving the user's view).
    pub fn request_view(&self, request: ViewRequest, by: Requester) -> Result<(), ServiceError> {
        if let Some(name) = &request.view
            && self.views.get(name).is_none()
        {
            return Err(ServiceError::NotFound(format!("view '{name}'")));
        }
        if let Some(focus) = &request.focus
            && self.graph(request.plan.as_ref())?.component(focus).is_none()
        {
            return Err(ServiceError::NotFound(focus.to_string()));
        }
        self.emit(ArchEvent::ViewRequested { request: Box::new(request), by });
        Ok(())
    }
}

/// Planned components already in the code (Skeleton or Implemented) that a
/// change to `changed` files affects: they live in one (before or after the
/// change), or something they're planned to call does.
pub fn affected(plan: &Plan, before: &Graph, after: &Graph, changed: &BTreeSet<&str>) -> Vec<ComponentId> {
    let touches = |id: &ComponentId| {
        let files = [before.component(id).and_then(|c| c.file()), after.component(id).and_then(|c| c.file()), plan.component(id).and_then(|c| c.file())];
        files.into_iter().flatten().any(|f| changed.contains(f.as_str()))
    };
    plan.components
        .iter()
        .filter(|c| c.stage.expects_code())
        .filter(|c| touches(&c.id) || plan.relations.iter().any(|r| r.kind == arch_graph::RelationKind::Calls && r.source == c.id && touches(&r.target)))
        .map(|c| c.id.clone())
        .collect()
}
