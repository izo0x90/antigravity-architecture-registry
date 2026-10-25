//! The project's changes (git): what a comparison changes, one file's
//! changes, which components they touch, the timeline. Reading only; the
//! comparison is the caller's (`from`, `to`: HEAD, index, files,
//! rev:<name>, snapshot:<id>; the uncommitted changes when left out).

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use harness_contracts::changes::{ChangeList, Comparison, ComponentChangeList, ComponentChanges, FileChanges, Group, Point, TEXTS_MOST};
use nah_changes::{Changes, ChangesError};
use serde::Deserialize;

use crate::cockpit::{Needs, can};
use crate::modules::ProjectState;

pub(crate) fn routes() -> Router<ProjectState> {
    Router::new().route("/api/changes", get(list)).route("/api/changes/file", get(file)).route("/api/changes/components", get(components)).route("/api/changes/timeline", get(timeline))
        // (Its body is the texts, JSON-escaped: room for them.)
        .route("/api/changes/texts", post(texts).layer(axum::extract::DefaultBodyLimit::max(4 * TEXTS_MOST)))
        .route("/api/changes/stage", post(stage))
        .route("/api/changes/unstage", post(unstage))
}

/// A changes error as an HTTP response: `{ "error": "..." }`.
pub(crate) struct ChangesFailed(pub(crate) ChangesError);

impl IntoResponse for ChangesFailed {
    fn into_response(self) -> Response {
        let status = match self.0 {
            ChangesError::NotARepo(_) => StatusCode::NOT_FOUND,
            ChangesError::BadPoint(_) => StatusCode::BAD_REQUEST,
            ChangesError::TooBig(_) => StatusCode::PAYLOAD_TOO_LARGE,
            ChangesError::Git(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(serde_json::json!({ "error": self.0.to_string() }))).into_response()
    }
}

type ChangesResult<T> = Result<T, ChangesFailed>;

/// The project's changes, if it's a git repository.
pub(crate) fn changes_of(state: &ProjectState) -> ChangesResult<Arc<Changes>> {
    state.changes.clone().ok_or_else(|| ChangesFailed(ChangesError::NotARepo(state.arch.root().display().to_string())))
}

/// Runs `f` on the changes off the async threads.
pub(crate) async fn blocking<T: Send + 'static>(changes: Arc<Changes>, f: impl FnOnce(&Changes) -> Result<T, ChangesError> + Send + 'static) -> ChangesResult<T> {
    tokio::task::spawn_blocking(move || f(&changes)).await.map_err(|e| ChangesFailed(ChangesError::Git(e.to_string())))?.map_err(ChangesFailed)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Compared {
    #[serde(default)]
    from: Option<Point>,
    #[serde(default)]
    to: Option<Point>,
}

impl Compared {
    pub(crate) fn comparison(&self) -> Comparison {
        let default = Comparison::default();
        Comparison { from: self.from.clone().unwrap_or(default.from), to: self.to.clone().unwrap_or(default.to) }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    #[serde(flatten)]
    compared: Compared,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

/// The files a comparison changes (a page), with their counts.
async fn list(_: Needs<can::ViewRead>, State(state): State<ProjectState>, Query(q): Query<ListQuery>) -> ChangesResult<Json<ChangeList>> {
    let comparison = q.compared.comparison();
    let (offset, limit) = (q.offset, q.limit.unwrap_or(nah_changes::PAGE));
    Ok(Json(blocking(changes_of(&state)?, move |c| c.list(&comparison, offset, limit)).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    #[serde(flatten)]
    compared: Compared,
    path: String,
    group: Group,
    #[serde(default)]
    context: Option<u32>,
}

/// One file's changes.
async fn file(_: Needs<can::ViewRead>, State(state): State<ProjectState>, Query(q): Query<FileQuery>) -> ChangesResult<Json<FileChanges>> {
    let comparison = q.compared.comparison();
    let context = q.context.unwrap_or(nah_changes::CONTEXT);
    Ok(Json(blocking(changes_of(&state)?, move |c| c.file(&comparison, &q.path, q.group, context)).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextsAsk {
    pairs: Vec<TextPair>,
}

/// Two texts, and the unchanged lines to show around each change.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TextPair {
    old: String,
    new: String,
    #[serde(default)]
    context: Option<u32>,
}

/// Pairs of texts (a call's changes, as its harness gave them), each
/// compared as a file's sides are, in the order asked. No repository
/// needed: nothing is read.
async fn texts(_: Needs<can::ViewRead>, Json(ask): Json<TextsAsk>) -> ChangesResult<Json<Vec<Vec<harness_contracts::changes::Hunk>>>> {
    let size: usize = ask.pairs.iter().map(|p| p.old.len() + p.new.len()).sum();
    if size > TEXTS_MOST {
        return Err(ChangesFailed(ChangesError::TooBig(format!("{size} bytes of text is too much to compare at once (the most is {TEXTS_MOST})"))));
    }
    let compared = tokio::task::spawn_blocking(move || ask.pairs.iter().map(|p| nah_changes::texts(&p.old, &p.new, p.context.unwrap_or(nah_changes::CONTEXT))).collect::<Result<Vec<_>, _>>()).await.map_err(|e| ChangesFailed(ChangesError::Git(e.to_string())))?;
    Ok(Json(compared.map_err(ChangesFailed)?))
}

/// What `comparison` changed in the components the index knows: each
/// component with the runs inside its lines (every level: a function and
/// what holds it), and the changed files it can't say for (out of date in
/// the index, or changed since the comparison's newer side).
pub(crate) fn component_changes(arch: &arch_service::ArchitectureService, changes: &Changes, comparison: &Comparison) -> Result<ComponentChangeList, ChangesError> {
    let on = changes.on_files(comparison)?;
    let mut out = ComponentChangeList { comparison: comparison.clone(), components: Vec::new(), out_of_date: on.moved };
    for file in &on.files {
        // Not indexed (no code the index reads): no components.
        let Ok(view) = arch.file_view(&file.path, &arch_graph::Source::default()) else { continue };
        if view.stale {
            out.out_of_date.push(file.path.clone());
            continue;
        }
        let spans: Vec<(&arch_graph::Component, u32, u32)> =
            view.components.iter().filter_map(|(c, _)| c.anchor.as_ref().filter(|a| a.file.as_str() == file.path).map(|a| (c, a.span.start_line, a.span.end_line.max(a.span.start_line)))).collect();
        // Each run's own component: the smallest that holds it.
        let owner: Vec<Option<&str>> = file.runs.iter().map(|r| spans.iter().filter(|(_, first, last)| r.within(*first, *last)).min_by_key(|(_, first, last)| last - first).map(|(c, ..)| c.id.as_str())).collect();
        for (c, first, last) in &spans {
            let (first, last) = (*first, *last);
            let inside: Vec<_> = file.runs.iter().filter(|r| r.within(first, last)).collect();
            if inside.is_empty() {
                continue;
            }
            out.components.push(ComponentChanges {
                component: c.id.as_str().to_owned(),
                file: file.path.clone(),
                added: inside.iter().map(|r| r.lines_in(first, last)).sum(),
                removed: inside.iter().filter(|r| (first..=last).contains(&r.at())).map(|r| r.removed).sum(),
                runs: inside.iter().map(|r| (r.start, r.lines)).collect(),
                direct: owner.contains(&Some(c.id.as_str())),
            });
        }
    }
    out.out_of_date.sort();
    out.out_of_date.dedup();
    Ok(out)
}

/// When the files changed and who was working: the windows between
/// snapshots (taken at every turn's start and end), each turn's changes.
async fn timeline(_: Needs<can::ViewRead>, State(state): State<ProjectState>) -> Result<Json<harness_contracts::changes::Timeline>, Response> {
    let timeline = state.timeline.clone().ok_or_else(|| ChangesFailed(ChangesError::NotARepo(state.arch.root().display().to_string())).into_response())?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let failed = |e: String| (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e }))).into_response();
    let read = tokio::task::spawn_blocking(move || timeline.read(now)).await.map_err(|e| failed(e.to_string()))?;
    Ok(Json(read.map_err(|e| match e {
        nah_timeline::TimelineError::Changes(e) => ChangesFailed(e).into_response(),
        e => failed(e.to_string()),
    })?))
}

/// One change in a file, as the Changes view showed it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HunkBody {
    old_start: u32,
    new_start: u32,
    context: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageBody {
    path: String,
    /// One change in it; the whole file when left out.
    #[serde(default)]
    hunk: Option<HunkBody>,
}

impl StageBody {
    fn hunk(&self) -> Option<nah_changes::HunkAt> {
        self.hunk.as_ref().map(|h| nah_changes::HunkAt { old_start: h.old_start, new_start: h.new_start, context: h.context })
    }
}

/// Stages a file, or one change in it: the user's act, from the cockpit.
async fn stage(_: Needs<can::ChangesStage>, State(state): State<ProjectState>, Json(body): Json<StageBody>) -> ChangesResult<StatusCode> {
    blocking(changes_of(&state)?, move |c| c.stage(&body.path, body.hunk())).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Unstages a file, or one change in it.
async fn unstage(_: Needs<can::ChangesStage>, State(state): State<ProjectState>, Json(body): Json<StageBody>) -> ChangesResult<StatusCode> {
    blocking(changes_of(&state)?, move |c| c.unstage(&body.path, body.hunk())).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The files a comparison (`from..to`) changed, for filters ("only changed
/// files").
pub(crate) async fn changed_files(changes: Option<Arc<Changes>>, range: &str) -> Result<std::collections::BTreeSet<String>, ChangesError> {
    let comparison: Comparison = range.parse().map_err(ChangesError::BadPoint)?;
    let changes = changes.ok_or_else(|| ChangesError::NotARepo("the project".into()))?;
    let paths = tokio::task::spawn_blocking(move || changes.changed_paths(&comparison)).await.map_err(|e| ChangesError::Git(e.to_string()))??;
    Ok(paths.into_iter().collect())
}

/// The components a comparison (`from..to`) changed themselves (not only
/// in their parts), for "only what changed".
pub(crate) async fn changed_components(arch: Arc<arch_service::ArchitectureService>, changes: Option<Arc<Changes>>, range: &str) -> Result<Vec<String>, ChangesError> {
    let comparison: Comparison = range.parse().map_err(ChangesError::BadPoint)?;
    let changes = changes.ok_or_else(|| ChangesError::NotARepo("the project".into()))?;
    let list = tokio::task::spawn_blocking(move || component_changes(&arch, &changes, &comparison)).await.map_err(|e| ChangesError::Git(e.to_string()))??;
    Ok(list.components.into_iter().filter(|c| c.direct).map(|c| c.component).collect())
}

/// Which components a comparison changed.
async fn components(_: Needs<can::ViewRead>, State(state): State<ProjectState>, Query(compared): Query<Compared>) -> ChangesResult<Json<ComponentChangeList>> {
    let comparison = compared.comparison();
    let arch = state.arch.clone();
    Ok(Json(blocking(changes_of(&state)?, move |c| component_changes(&arch, c, &comparison)).await?))
}
