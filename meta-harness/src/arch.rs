//! REST API for the architecture service (the cockpit's side; agents use the
//! MCP tools). Approvals live here, never in the agent tools.

use crate::cockpit::CockpitUser;
use std::sync::Arc;

use arch_graph::{Comment, CommentId, ComponentId, PlanId, Requester, Source, Stage, ViewRequest};
use arch_service::{Actor, ArchitectureService, NewComment, PlanOp, ServiceError};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::server::AppState;

pub struct ArchError(ServiceError);

impl From<ServiceError> for ArchError {
    fn from(err: ServiceError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ArchError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            ServiceError::NotFound(_) => StatusCode::NOT_FOUND,
            ServiceError::InvalidOp(_) => StatusCode::BAD_REQUEST,
            ServiceError::Forbidden(_) => StatusCode::FORBIDDEN,
            ServiceError::Io(_) | ServiceError::Corrupt(_) | ServiceError::Index(_) | ServiceError::Poisoned => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

pub type ArchResult<T> = Result<Json<T>, ArchError>;

/// Runs a service call off the async runtime (parsing and file I/O block).
pub async fn blocking<T: Send + 'static>(
    arch: &Arc<ArchitectureService>,
    f: impl FnOnce(&ArchitectureService) -> Result<T, ServiceError> + Send + 'static,
) -> Result<T, ServiceError> {
    let arch = arch.clone();
    tokio::task::spawn_blocking(move || f(&arch)).await.map_err(|e| ServiceError::Io(format!("task failed: {e}")))?
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/arch/types", get(types))
        .route("/api/search", get(search))
        .route("/api/arch/components/{id}", get(component))
        .route("/api/arch/at", get(component_at))
        .route("/api/arch/file", get(file_view))
        .route("/api/arch/ingest", post(ingest))
        .route("/api/arch/index", get(index_status))
        .route("/api/arch/refresh", post(refresh))
        .route("/api/arch/tools", get(tools))
        .route("/api/arch/reviewers", get(reviewers))
        .route("/api/arch/ambiguous", get(ambiguous))
        .route("/api/arch/unproven", get(unproven))
        .route("/api/arch/navigate", post(navigate))
        .route("/api/arch/plans", get(list_plans).post(create_plan))
        .route("/api/arch/import", post(import_registry))
        .route("/api/arch/plans/{plan}", get(get_plan).delete(delete_plan))
        .route("/api/arch/plans/{plan}/ops", post(apply_ops))
        .route("/api/arch/plans/{plan}/scope", get(scope))
        .route("/api/arch/plans/{plan}/validate", post(validate))
        .route("/api/arch/plans/{plan}/next", get(next_actionable))
        .route("/api/arch/plans/{plan}/components/{id}/contract", get(contract))
        .route("/api/arch/plans/{plan}/components/{id}/verify", post(verify))
        .route("/api/arch/plans/{plan}/components/{id}/approve", post(approve))
        .route("/api/arch/plans/{plan}/tools", post(run_tools))
        .route("/api/arch/plans/{plan}/components/{id}/review", post(review))
        .route("/api/arch/plans/{plan}/comments", get(list_comments).post(add_comment))
        .route("/api/arch/plans/{plan}/comments/{comment}/replies", post(reply_comment))
        .route("/api/arch/plans/{plan}/comments/{comment}/accept", post(accept_comment))
        .route("/api/arch/plans/{plan}/comments/{comment}/reopen", post(reopen_comment))
        .route("/api/arch/plans/{plan}/reviewed", post(mark_reviewed))
        .route("/api/arch/plans/{plan}/diff", get(review_diff))
}

pub fn plan_id(raw: &str) -> Result<PlanId, ArchError> {
    PlanId::new(raw).map_err(|e| ArchError(ServiceError::InvalidOp(e.to_string())))
}

/// What a read shows: the plan named (`plan=<id>`), else the code alone.
fn source_param(raw: &Option<String>) -> Result<Source, ArchError> {
    Ok(Source::from_plan(raw.as_deref().filter(|p| !p.is_empty()).map(plan_id).transpose()?))
}

async fn types(State(state): State<AppState>) -> Json<arch_graph::TypeSystem> {
    Json(state.arch.types().clone())
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    /// symbols (the default), files or text.
    #[serde(default)]
    kind: arch_service::SearchKind,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

/// Symbols, files or lines matching `q`; each hit names the card it stands for.
async fn search(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> ArchResult<arch_service::Found> {
    let source = source_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.search(&q.q, q.kind, &source, q.limit)).await?))
}

#[derive(Deserialize)]
struct PlanQuery {
    #[serde(default)]
    plan: Option<String>,
}

async fn component(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<PlanQuery>) -> ArchResult<arch_service::ComponentDetail> {
    let source = source_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.component(&ComponentId::new(id), &source)).await?))
}

#[derive(Deserialize)]
struct AtQuery {
    file: String,
    line: u32,
    #[serde(default)]
    plan: Option<String>,
}

/// The component enclosing a code location (reveal-in-graph from the code view).
async fn component_at(State(state): State<AppState>, Query(q): Query<AtQuery>) -> ArchResult<arch_graph::Component> {
    let source = source_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.component_at(&q.file, q.line, &source)).await?))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    plan: Option<String>,
}

async fn file_view(State(state): State<AppState>, Query(q): Query<FileQuery>) -> ArchResult<arch_graph::FileView> {
    let source = source_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.file_view(&q.path, &source)).await?))
}

#[derive(Deserialize, Default)]
struct IngestBody {
    /// Path inside the repo; the whole repo when absent.
    #[serde(default)]
    scope: Option<String>,
    /// Drop the cache and read everything again.
    #[serde(default)]
    full: bool,
}

async fn ingest(State(state): State<AppState>, body: Option<Json<IngestBody>>) -> ArchResult<arch_service::IngestReport> {
    let (scope, full) = body.map(|b| (b.0.scope, b.0.full)).unwrap_or_default();
    Ok(Json(
        blocking(&state.arch, move |a| {
            if full {
                return a.reindex();
            }
            let path = match &scope {
                Some(s) => a.root().join(arch_graph::RepoPath::new(s).map_err(|e| ServiceError::InvalidOp(e.to_string()))?.as_str()),
                None => a.root().to_path_buf(),
            };
            a.ingest(&path)
        })
        .await?,
    ))
}

async fn index_status(State(state): State<AppState>) -> Json<arch_graph::IndexStatus> {
    Json(state.arch.index_status())
}

async fn ambiguous(State(state): State<AppState>) -> ArchResult<Vec<code_analyzer::AmbiguousCall>> {
    Ok(Json(state.arch.ambiguous_calls()?))
}

/// Calls the code makes that couldn't be tied to a component (never guessed).
async fn unproven(State(state): State<AppState>) -> ArchResult<Vec<code_analyzer::AmbiguousCall>> {
    Ok(Json(state.arch.unproven_calls()?))
}

async fn navigate(State(state): State<AppState>, Json(request): Json<ViewRequest>) -> Result<StatusCode, ArchError> {
    blocking(&state.arch, move |a| a.request_view(request, Requester::User)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_plans(State(state): State<AppState>) -> ArchResult<Vec<arch_graph::Plan>> {
    Ok(Json(blocking(&state.arch, |a| a.plans()).await?))
}

#[derive(Deserialize)]
struct NewPlan {
    id: String,
    title: String,
    #[serde(default)]
    description: String,
}

async fn create_plan(State(state): State<AppState>, Json(body): Json<NewPlan>) -> ArchResult<arch_graph::Plan> {
    Ok(Json(blocking(&state.arch, move |a| a.create_plan(&body.id, &body.title, &body.description)).await?))
}

fn old_registry_file() -> String {
    "system_architecture.json".to_owned()
}

#[derive(Deserialize)]
struct Import {
    plan: String,
    #[serde(default)]
    title: Option<String>,
    /// The old registry document, relative to the repo root.
    #[serde(default = "old_registry_file")]
    file: String,
    #[serde(default)]
    language: Option<String>,
}

#[derive(serde::Serialize)]
struct Imported {
    plan: arch_graph::Plan,
    report: arch_service::ImportReport,
}

/// Creates a plan from the old registry document.
async fn import_registry(State(state): State<AppState>, Json(body): Json<Import>) -> ArchResult<Imported> {
    let (plan, report) = blocking(&state.arch, move |a| {
        let title = match &body.title {
            Some(t) => t.clone(),
            None => format!("Imported from {}", body.file),
        };
        a.import_registry_file(&body.plan, &title, &body.file, body.language.as_deref())
    })
    .await?;
    Ok(Json(Imported { plan, report }))
}

async fn get_plan(State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<arch_graph::Plan> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.plan(&plan)).await?))
}

async fn delete_plan(_: CockpitUser, State(state): State<AppState>, Path(plan): Path<String>) -> Result<StatusCode, ArchError> {
    let plan = plan_id(&plan)?;
    let p = plan.clone();
    blocking(&state.arch, move |a| a.delete_plan(&p)).await?;
    state.queue.forget(&plan);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Ops {
    ops: Vec<PlanOp>,
}

/// Applies operations in order; stops at the first one that fails.
async fn apply_ops(State(state): State<AppState>, Path(plan): Path<String>, Json(body): Json<Ops>) -> ArchResult<arch_graph::Plan> {
    let plan = plan_id(&plan)?;
    Ok(Json(
        blocking(&state.arch, move |a| {
            let mut current = a.plan(&plan)?;
            for op in body.ops {
                current = a.apply(&plan, op)?;
            }
            Ok(current)
        })
        .await?,
    ))
}

async fn scope(State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<Vec<ComponentId>> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.scope(&plan)).await?))
}

async fn validate(State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<arch_graph::Report> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.validate(&plan)).await?))
}

#[derive(Deserialize)]
struct NextQuery {
    action: arch_service::Action,
}

/// Components ready for detailed design (`plan`) or for code (`build`), in dependency order.
async fn next_actionable(State(state): State<AppState>, Path(plan): Path<String>, Query(q): Query<NextQuery>) -> ArchResult<Vec<ComponentId>> {
    let plan = plan_id(&plan)?;
    Ok(Json(
        blocking(&state.arch, move |a| {
            let current = a.plan(&plan)?;
            let graph = a.graph(&Source::Plan(plan.clone()))?;
            Ok(arch_service::next_actionable(&current, &graph, a.types(), q.action))
        })
        .await?,
    ))
}

#[derive(serde::Serialize)]
struct ContractResponse {
    slice: arch_service::ContractSlice,
    markdown: String,
}

async fn contract(State(state): State<AppState>, Path((plan, id)): Path<(String, String)>) -> ArchResult<ContractResponse> {
    let plan = plan_id(&plan)?;
    let slice = blocking(&state.arch, move |a| a.contract(&plan, &ComponentId::new(id))).await?;
    Ok(Json(ContractResponse { markdown: slice.to_markdown(), slice }))
}

#[derive(Deserialize)]
struct VerifyBody {
    goal: Stage,
}

async fn verify(State(state): State<AppState>, Path((plan, id)): Path<(String, String)>, Json(body): Json<VerifyBody>) -> ArchResult<arch_service::Verification> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.verify(&plan, &ComponentId::new(id), body.goal)).await?))
}

#[derive(Deserialize)]
struct ApproveBody {
    to: Stage,
    /// The approving user, as shown in the cockpit.
    by: String,
}

/// The cockpit's approval button. Agents have no equivalent tool.
async fn approve(_: CockpitUser, State(state): State<AppState>, Path((plan, id)): Path<(String, String)>, Json(body): Json<ApproveBody>) -> ArchResult<arch_graph::Approval> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.approve(&plan, &ComponentId::new(id), body.to, &actor)).await?))
}

#[derive(Deserialize)]
struct RefreshBody {
    /// Repo-relative paths that changed.
    paths: Vec<String>,
}

/// Re-reads changed files and re-verifies the planned components they affect.
async fn refresh(State(state): State<AppState>, Json(body): Json<RefreshBody>) -> ArchResult<arch_service::Refresh> {
    let paths = body
        .paths
        .iter()
        .map(|p| arch_graph::RepoPath::new(p).map(|p| state.arch.root().join(p.as_str())).map_err(|e| ArchError(ServiceError::InvalidOp(e.to_string()))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(blocking(&state.arch, move |a| a.refresh(&paths)).await?))
}

async fn tools(State(state): State<AppState>) -> Json<arch_service::ToolCatalog> {
    Json(state.arch.tools().clone())
}

async fn reviewers(State(state): State<AppState>) -> Json<arch_service::ReviewerCatalog> {
    Json(state.arch.reviewers().clone())
}

#[derive(Deserialize, Default)]
struct ToolsBody {
    /// One component; every planned component in the code when absent.
    #[serde(default)]
    component: Option<String>,
}

async fn run_tools(State(state): State<AppState>, Path(plan): Path<String>, body: Option<Json<ToolsBody>>) -> ArchResult<arch_service::ToolsReport> {
    let plan = plan_id(&plan)?;
    let only = body.and_then(|b| b.0.component).map(ComponentId::new);
    Ok(Json(blocking(&state.arch, move |a| a.run_tools(&plan, only.as_ref())).await?))
}

async fn review(State(state): State<AppState>, Path((plan, id)): Path<(String, String)>) -> ArchResult<arch_service::ReviewsReport> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.review(&plan, &ComponentId::new(id))).await?))
}

/// The cockpit's user, as named in the request (comments and approvals are theirs).
fn user(by: &str) -> Result<Actor, ArchError> {
    let name = by.trim();
    if name.is_empty() {
        return Err(ArchError(ServiceError::InvalidOp("say who you are (`by`)".into())));
    }
    Ok(Actor::User { name: name.to_owned() })
}

#[derive(Deserialize)]
struct CommentsQuery {
    /// Include resolved comments too.
    #[serde(default)]
    all: bool,
}

async fn list_comments(State(state): State<AppState>, Path(plan): Path<String>, Query(q): Query<CommentsQuery>) -> ArchResult<Vec<Comment>> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.comments(&plan, q.all)).await?))
}

#[derive(Deserialize)]
struct NewCommentBody {
    #[serde(flatten)]
    comment: NewComment,
    by: String,
}

async fn add_comment(_: CockpitUser, State(state): State<AppState>, Path(plan): Path<String>, Json(body): Json<NewCommentBody>) -> ArchResult<Comment> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.add_comment(&plan, body.comment, &actor)).await?))
}

#[derive(Deserialize)]
struct TextBody {
    #[serde(default)]
    text: String,
    by: String,
}

async fn reply_comment(_: CockpitUser, State(state): State<AppState>, Path((plan, comment)): Path<(String, String)>, Json(body): Json<TextBody>) -> ArchResult<Comment> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.reply(&plan, &CommentId(comment), &body.text, &actor)).await?))
}

#[derive(Deserialize)]
struct ByBody {
    by: String,
}

async fn accept_comment(_: CockpitUser, State(state): State<AppState>, Path((plan, comment)): Path<(String, String)>, Json(body): Json<ByBody>) -> ArchResult<Comment> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.accept_comment(&plan, &CommentId(comment), &actor)).await?))
}

async fn reopen_comment(_: CockpitUser, State(state): State<AppState>, Path((plan, comment)): Path<(String, String)>, Json(body): Json<TextBody>) -> ArchResult<Comment> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.reopen_comment(&plan, &CommentId(comment), &body.text, &actor)).await?))
}

async fn mark_reviewed(_: CockpitUser, State(state): State<AppState>, Path(plan): Path<String>, Json(body): Json<ByBody>) -> ArchResult<arch_graph::ReviewSnapshot> {
    let plan = plan_id(&plan)?;
    let actor = user(&body.by)?;
    Ok(Json(blocking(&state.arch, move |a| a.mark_reviewed(&plan, &actor)).await?))
}

async fn review_diff(State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<arch_graph::PlanDiff> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.review_diff(&plan)).await?))
}
