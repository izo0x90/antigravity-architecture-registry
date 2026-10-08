//! REST for the three views (the cockpit's graph): a tree for what's asked,
//! what opening one card adds, where a card sits, everything about one card,
//! and the views' settings (the cockpit draws with the same sizes).

use arch_graph::tree::{Tree, TreeConfig, TreeRequest};
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
}

async fn tree(State(state): State<AppState>, Json(request): Json<TreeRequest>) -> ArchResult<Tree> {
    Ok(Json(blocking(&state.arch, move |a| a.tree(&request)).await?))
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

async fn expand(State(state): State<AppState>, Json(body): Json<ExpandBody>) -> ArchResult<Tree> {
    Ok(Json(blocking(&state.arch, move |a| a.expand(&body.request, &body.card, body.levels)).await?))
}

#[derive(Deserialize)]
struct ContextBody {
    request: TreeRequest,
    card: String,
}

async fn context(State(state): State<AppState>, Json(body): Json<ContextBody>) -> ArchResult<Tree> {
    Ok(Json(blocking(&state.arch, move |a| a.context(&body.request, &body.card)).await?))
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

async fn config(State(state): State<AppState>) -> Json<TreeConfig> {
    Json(state.arch.trees().clone())
}
