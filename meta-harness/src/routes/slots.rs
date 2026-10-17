//! Slots, for the cockpit: list them, start and close sub-slots, choose a
//! slot's harness and settings, open, switch and close its sessions, and
//! send it a prompt (which resumes its session first if it isn't running).

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use harness_contracts::{InstanceId, ModelSelection, NativeId, RuntimePolicy, SessionKey, Slot, SlotId, Starter, ThreadId, TurnId};
use harness_core::{Delivery, Open, Origin, Sent, SlotSettings};
use serde::{Deserialize, Serialize};

use crate::cockpit::{Needs, can};
use crate::server::ApiResult;
use crate::modules::HarnessState;

pub(crate) fn routes() -> Router<HarnessState> {
    Router::new()
        .route("/api/slots", get(list).post(create))
        .route("/api/slots/{slot}", delete(close))
        .route("/api/slots/{slot}/harness", post(harness))
        .route("/api/slots/{slot}/settings", post(settings))
        .route("/api/slots/{slot}/sessions", post(open))
        .route("/api/slots/{slot}/sessions/{session}", delete(close_session))
        .route("/api/slots/{slot}/current", post(switch))
        .route("/api/slots/{slot}/prompt", post(prompt))
}

async fn list(_: Needs<can::ViewRead>, State(state): State<HarnessState>) -> Json<Vec<Slot>> {
    Json(state.slots.list())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NewSlot {
    #[serde(default = "sub")]
    role: String,
    #[serde(default)]
    name: Option<String>,
    instance_id: InstanceId,
    #[serde(default)]
    policy: RuntimePolicy,
    #[serde(default)]
    model: Option<ModelSelection>,
}

fn sub() -> String {
    "sub".into()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Created {
    slot_id: SlotId,
}

/// The user starts a sub-slot (the lead and the queue's workers aren't made here).
async fn create(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Json(body): Json<NewSlot>) -> ApiResult<Json<Created>> {
    if body.role != "sub" {
        return Err(harness_core::HarnessError::InvalidConfig { reason: format!("the cockpit starts sub-slots, not {:?}", body.role) }.into());
    }
    state.registry.get(&body.instance_id)?;
    let settings = SlotSettings { instance: Some(body.instance_id), policy: body.policy, model: body.model };
    let slot_id = state.slots.create(&body.role, body.name, settings, Starter::User).await?;
    Ok(Json(Created { slot_id }))
}

async fn close(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>) -> ApiResult<StatusCode> {
    state.slots.close(&SlotId::new(slot)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Harness {
    instance_id: InstanceId,
}

async fn harness(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>, Json(body): Json<Harness>) -> ApiResult<StatusCode> {
    state.registry.get(&body.instance_id)?;
    state.slots.set_harness(&SlotId::new(slot), body.instance_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    policy: RuntimePolicy,
    #[serde(default)]
    model: Option<ModelSelection>,
}

/// How the slot's next sessions run.
async fn settings(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>, Json(body): Json<Settings>) -> ApiResult<StatusCode> {
    state.slots.set_settings(&SlotId::new(slot), body.policy, body.model).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OpenBody {
    /// A session the harness has saved, by its id.
    #[serde(default)]
    resume: Option<NativeId>,
    /// One of the slot's own sessions, again.
    #[serde(default)]
    session: Option<SessionKey>,
    /// The harness to open it on (the slot's is set first; only while
    /// none of its sessions runs).
    #[serde(default)]
    instance_id: Option<InstanceId>,
    /// How it runs (the slot's settings are set first).
    #[serde(default)]
    policy: Option<RuntimePolicy>,
    #[serde(default)]
    model: Option<ModelSelection>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenedBody {
    thread_id: ThreadId,
    session: SessionKey,
    note: Option<String>,
    delivered_reports: usize,
}

async fn open(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>, Json(body): Json<OpenBody>) -> ApiResult<Json<OpenedBody>> {
    let slot_id = SlotId::new(slot);
    if let Some(instance) = body.instance_id {
        state.registry.get(&instance)?;
        state.slots.set_harness(&slot_id, instance).await?;
    }
    if let Some(policy) = body.policy {
        state.slots.set_settings(&slot_id, policy, body.model).await?;
    }
    let open = match (body.session, body.resume) {
        (Some(_), Some(_)) => return Err(harness_core::HarnessError::InvalidConfig { reason: "open one of the slot's sessions, or resume a saved one: not both".into() }.into()),
        (Some(session), None) => Open::Again(session),
        (None, resume) => Open::New { resume },
    };
    let opened = state.slots.open_session(&slot_id, open).await?;
    Ok(Json(OpenedBody { thread_id: opened.thread, session: opened.session, note: opened.note, delivered_reports: opened.delivered_reports }))
}

async fn close_session(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path((slot, session)): Path<(String, String)>) -> ApiResult<StatusCode> {
    state.slots.close_session(&SlotId::new(slot), &SessionKey::new(session)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Switch {
    session: SessionKey,
}

async fn switch(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>, Json(body): Json<Switch>) -> ApiResult<StatusCode> {
    state.slots.switch(&SlotId::new(slot), &body.session).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Prompt {
    text: String,
    /// Into the turn it's running (when its harness takes that), not after it.
    #[serde(default)]
    add_to_running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Prompted {
    turn_id: TurnId,
}

/// The user's prompt to a slot: its current session is resumed (or a new
/// one opened) first if it isn't running. The tags it names (`#name`) hand
/// the agent what they're on.
async fn prompt(_: Needs<can::SessionsControl>, State(state): State<HarnessState>, Path(slot): Path<String>, Json(body): Json<Prompt>) -> ApiResult<Json<Prompted>> {
    let delivery = if body.add_to_running { Delivery::AddToRunning } else { Delivery::Queue };
    let tags = state.tags.clone();
    let text = tokio::task::spawn_blocking(move || tags.expand(&body.text)).await.map_err(|e| harness_core::HarnessError::Store { reason: e.to_string() })?;
    let turn_id = match state.slots.send(&SlotId::new(slot), text, delivery, Origin::User, None).await? {
        Sent::Started { turn } | Sent::Replied { turn, .. } | Sent::StillWorking { turn } => turn,
    };
    Ok(Json(Prompted { turn_id }))
}
