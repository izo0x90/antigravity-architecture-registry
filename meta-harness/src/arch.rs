//! REST API for the architecture service (the cockpit's side; agents use the
//! MCP tools). Approvals live here, never in the agent tools.

use std::sync::Arc;

use arch_graph::{ComponentId, PlanId, Requester, Stage, ViewRequest, ViewSpec};
use arch_service::{Actor, ArchitectureService, PlanOp, ServiceError};
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

type ArchResult<T> = Result<Json<T>, ArchError>;

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
        .route("/api/arch/views", get(views))
        .route("/api/arch/view", get(named_view).post(custom_view))
        .route("/api/arch/search", get(search))
        .route("/api/arch/components/{id}", get(component))
        .route("/api/arch/at", get(component_at))
        .route("/api/arch/file", get(file_view))
        .route("/api/arch/ingest", post(ingest))
        .route("/api/arch/ambiguous", get(ambiguous))
        .route("/api/arch/navigate", post(navigate))
        .route("/api/arch/plans", get(list_plans).post(create_plan))
        .route("/api/arch/plans/{plan}", get(get_plan).delete(delete_plan))
        .route("/api/arch/plans/{plan}/ops", post(apply_ops))
        .route("/api/arch/plans/{plan}/scope", get(scope))
        .route("/api/arch/plans/{plan}/validate", post(validate))
        .route("/api/arch/plans/{plan}/components/{id}/contract", get(contract))
        .route("/api/arch/plans/{plan}/components/{id}/verify", post(verify))
        .route("/api/arch/plans/{plan}/components/{id}/approve", post(approve))
}

fn plan_id(raw: &str) -> Result<PlanId, ArchError> {
    PlanId::new(raw).map_err(|e| ArchError(ServiceError::InvalidOp(e.to_string())))
}

fn plan_param(raw: &Option<String>) -> Result<Option<PlanId>, ArchError> {
    raw.as_deref().filter(|p| !p.is_empty()).map(plan_id).transpose()
}

async fn types(State(state): State<AppState>) -> Json<arch_graph::TypeSystem> {
    Json(state.arch.types().clone())
}

async fn views(State(state): State<AppState>) -> Json<arch_graph::ViewCatalog> {
    Json(state.arch.views().clone())
}

#[derive(Deserialize)]
struct ViewQuery {
    name: String,
    #[serde(default)]
    plan: Option<String>,
    /// Comma-separated component ids.
    #[serde(default)]
    roots: Option<String>,
    #[serde(default)]
    depth: Option<u32>,
}

async fn named_view(State(state): State<AppState>, Query(q): Query<ViewQuery>) -> ArchResult<arch_service::ViewResult> {
    let plan = plan_param(&q.plan)?;
    let roots: Vec<ComponentId> = q.roots.as_deref().unwrap_or("").split(',').filter(|s| !s.is_empty()).map(ComponentId::new).collect();
    Ok(Json(blocking(&state.arch, move |a| a.named_view(&q.name, plan.as_ref(), roots, q.depth)).await?))
}

#[derive(Deserialize)]
struct CustomView {
    spec: ViewSpec,
    #[serde(default)]
    plan: Option<String>,
}

async fn custom_view(State(state): State<AppState>, Json(body): Json<CustomView>) -> ArchResult<arch_service::ViewResult> {
    let plan = plan_param(&body.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.view(&body.spec, plan.as_ref())).await?))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn search(State(state): State<AppState>, Query(q): Query<SearchQuery>) -> ArchResult<Vec<arch_graph::Component>> {
    let plan = plan_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.search(&q.q, plan.as_ref(), q.limit.unwrap_or(25))).await?))
}

#[derive(Deserialize)]
struct PlanQuery {
    #[serde(default)]
    plan: Option<String>,
}

async fn component(State(state): State<AppState>, Path(id): Path<String>, Query(q): Query<PlanQuery>) -> ArchResult<arch_service::ComponentDetail> {
    let plan = plan_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.component(&ComponentId::new(id), plan.as_ref())).await?))
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
    let plan = plan_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.component_at(&q.file, q.line, plan.as_ref())).await?))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    #[serde(default)]
    plan: Option<String>,
}

async fn file_view(State(state): State<AppState>, Query(q): Query<FileQuery>) -> ArchResult<arch_graph::FileView> {
    let plan = plan_param(&q.plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.file_view(&q.path, plan.as_ref())).await?))
}

#[derive(Deserialize, Default)]
struct IngestBody {
    /// Path inside the repo; the whole repo when absent.
    #[serde(default)]
    scope: Option<String>,
}

async fn ingest(State(state): State<AppState>, body: Option<Json<IngestBody>>) -> ArchResult<arch_service::IngestReport> {
    let scope = body.and_then(|b| b.0.scope);
    Ok(Json(
        blocking(&state.arch, move |a| {
            let path = match &scope {
                Some(s) => a.root().join(arch_graph::RepoPath::new(s).map_err(|e| ServiceError::InvalidOp(e.to_string()))?.as_str()),
                None => a.root().to_path_buf(),
            };
            a.ingest(&path)
        })
        .await?,
    ))
}

async fn ambiguous(State(state): State<AppState>) -> ArchResult<Vec<arch_index::AmbiguousCall>> {
    Ok(Json(state.arch.ambiguous_calls()?))
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

async fn get_plan(State(state): State<AppState>, Path(plan): Path<String>) -> ArchResult<arch_graph::Plan> {
    let plan = plan_id(&plan)?;
    Ok(Json(blocking(&state.arch, move |a| a.plan(&plan)).await?))
}

async fn delete_plan(State(state): State<AppState>, Path(plan): Path<String>) -> Result<StatusCode, ArchError> {
    let plan = plan_id(&plan)?;
    blocking(&state.arch, move |a| a.delete_plan(&plan)).await?;
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
async fn approve(State(state): State<AppState>, Path((plan, id)): Path<(String, String)>, Json(body): Json<ApproveBody>) -> ArchResult<arch_graph::Approval> {
    let plan = plan_id(&plan)?;
    if body.by.trim().is_empty() {
        return Err(ArchError(ServiceError::InvalidOp("approvals need the approving user's name".into())));
    }
    let actor = Actor::User { name: body.by };
    Ok(Json(blocking(&state.arch, move |a| a.approve(&plan, &ComponentId::new(id), body.to, &actor)).await?))
}
