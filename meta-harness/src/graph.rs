//! REST for the three views (the cockpit's graph): a tree for what's asked,
//! what opening one card adds, where a card sits, everything about one card,
//! and the views' settings (the cockpit draws with the same sizes).

use arch_graph::tree::{TreeConfig, TreeRequest};
use arch_graph::{ComponentDetail, PlanId};
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::arch::{ArchResult, blocking, plan_id};
use crate::server::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/graph/tree", post(tree))
        .route("/api/graph/expand", post(expand))
        .route("/api/graph/context", post(context))
        .route("/api/graph/card", get(card))
        .route("/api/graph/config", get(config))
        .route("/api/perf", get(perf))
}

/// JSON the service already encoded (so encoding is timed with the rest).
type Encoded = Result<([(axum::http::HeaderName, &'static str); 1], Vec<u8>), crate::arch::ArchError>;

fn json(bytes: Vec<u8>) -> ([(axum::http::HeaderName, &'static str); 1], Vec<u8>) {
    ([(axum::http::header::CONTENT_TYPE, "application/json")], bytes)
}

async fn tree(State(state): State<AppState>, Json(request): Json<TreeRequest>) -> Encoded {
    Ok(json(blocking(&state.arch, move |a| a.tree_json(&request)).await?))
}

#[derive(Deserialize)]
struct ExpandBody {
    request: TreeRequest,
    card: String,
    #[serde(default = "one")]
    levels: u32,
}

fn one() -> u32 {
    1
}

async fn expand(State(state): State<AppState>, Json(body): Json<ExpandBody>) -> Encoded {
    Ok(json(blocking(&state.arch, move |a| a.expand_json(&body.request, &body.card, body.levels)).await?))
}

#[derive(Deserialize)]
struct ContextBody {
    request: TreeRequest,
    card: String,
}

async fn context(State(state): State<AppState>, Json(body): Json<ContextBody>) -> Encoded {
    Ok(json(blocking(&state.arch, move |a| a.context_json(&body.request, &body.card)).await?))
}

#[derive(Deserialize)]
struct CardQuery {
    key: String,
    #[serde(default)]
    plan: Option<String>,
}

async fn card(State(state): State<AppState>, Query(q): Query<CardQuery>) -> ArchResult<ComponentDetail> {
    let plan: Option<PlanId> = q.plan.as_deref().filter(|p| !p.is_empty()).map(plan_id).transpose()?;
    let source = arch_graph::Source::from_plan(plan);
    Ok(Json(blocking(&state.arch, move |a| a.card(&q.key, &source)).await?))
}

/// The last answers' timings stage by stage, the graph's and the store's
/// size, memory, and the last index run.
async fn perf(State(state): State<AppState>) -> ArchResult<arch_service::PerfReport> {
    Ok(Json(blocking(&state.arch, |a| a.perf()).await?))
}

async fn config(State(state): State<AppState>) -> Json<TreeConfig> {
    Json(state.arch.trees().clone())
}
