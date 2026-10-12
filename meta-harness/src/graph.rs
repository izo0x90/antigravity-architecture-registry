//! REST for the three views (the cockpit's graph): a tree for what's asked,
//! what opening one card adds, where a card sits, everything about one card,
//! and the views' settings (the cockpit draws with the same sizes).

use arch_graph::tree::{QUERY_EXTRA, TreeConfig, TreeRequest};
use arch_graph::ComponentDetail;
use arch_service::ServiceError;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use crate::arch::{ArchResult, blocking};
use crate::server::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/graph/tree", get(tree))
        .route("/api/graph/expand", get(expand))
        .route("/api/graph/context", get(context))
        .route("/api/graph/details/{card}", get(details))
        .route("/api/graph/config", get(config))
        .route("/api/perf", get(perf))
}

/// JSON the service already encoded (so encoding is timed with the rest).
type Encoded = Result<([(axum::http::HeaderName, &'static str); 1], Vec<u8>), crate::arch::ArchError>;

fn json(bytes: Vec<u8>) -> ([(axum::http::HeaderName, &'static str); 1], Vec<u8>) {
    ([(axum::http::header::CONTENT_TYPE, "application/json")], bytes)
}

/// A view's request from the query, and what the asker added to it.
struct Asked {
    request: TreeRequest,
    pairs: Vec<(String, String)>,
}

impl Asked {
    fn from(pairs: Vec<(String, String)>, extra: &[&str]) -> Result<Self, crate::arch::ArchError> {
        let request = TreeRequest::from_query_pairs(&pairs, extra).map_err(ServiceError::InvalidOp)?;
        Ok(Self { request, pairs })
    }

    fn one(&self, name: &str) -> Option<&str> {
        self.pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// The card asked about (expand, context).
    fn card(&self) -> Result<String, crate::arch::ArchError> {
        Ok(self.one("card").ok_or_else(|| ServiceError::InvalidOp("card: which card?".into()))?.to_owned())
    }
}

async fn tree(State(state): State<AppState>, Query(pairs): Query<Vec<(String, String)>>) -> Encoded {
    let asked = Asked::from(pairs, &[])?;
    Ok(json(blocking(&state.arch, move |a| a.tree_json(&asked.request)).await?))
}

/// What opening a card some levels further changes in the tree the asker
/// holds (the one its request was answered with): the new cards, every
/// card's place, the lines.
async fn expand(State(state): State<AppState>, Query(pairs): Query<Vec<(String, String)>>) -> Encoded {
    let asked = Asked::from(pairs, &QUERY_EXTRA)?;
    let card = asked.card()?;
    let levels = asked.one("levels").map_or(Ok(1), str::parse::<u32>).map_err(|e| ServiceError::InvalidOp(format!("levels: {e}")))?;
    Ok(json(blocking(&state.arch, move |a| a.expand_json(&asked.request, &card, levels)).await?))
}

async fn context(State(state): State<AppState>, Query(pairs): Query<Vec<(String, String)>>) -> Encoded {
    let asked = Asked::from(pairs, &["card"])?;
    let card = asked.card()?;
    Ok(json(blocking(&state.arch, move |a| a.context_json(&asked.request, &card)).await?))
}

// An argument it doesn't know is refused, never ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceQuery {
    /// `code` (the default) or `plan:<id>`.
    #[serde(default)]
    source: arch_graph::Source,
}

/// Everything about one card (its component's details).
async fn details(State(state): State<AppState>, Path(card): Path<String>, Query(q): Query<SourceQuery>) -> ArchResult<ComponentDetail> {
    Ok(Json(blocking(&state.arch, move |a| a.card(&card, &q.source)).await?))
}

/// The last answers' timings stage by stage, the graph's and the store's
/// size, memory, and the last index run.
async fn perf(State(state): State<AppState>) -> ArchResult<arch_service::PerfReport> {
    Ok(Json(blocking(&state.arch, |a| a.perf()).await?))
}

async fn config(State(state): State<AppState>) -> Json<TreeConfig> {
    Json(state.arch.trees().clone())
}
