//! The harness abstraction. Three layers, each knowing nothing about the next one up:
//!
//! - [`ProviderDriver`]: one per harness kind. Turns configuration into instances.
//! - [`ProviderInstance`]: install/auth/catalog for one configured harness, plus its
//!   [`ProviderAdapter`].
//! - [`ProviderAdapter`] / [`SessionRuntime`]: live sessions only. Emits
//!   [`harness_contracts::AdapterEvent`]s; never knows how the binary was installed
//!   or authenticated.
//!
//! The server routes by `InstanceId`/`ThreadId` and gates features on
//! [`harness_contracts::Capabilities`]; it never matches on a driver kind.

mod error;
mod hub;
mod registry;
mod sessions;
pub mod skills;
pub mod slots;
pub mod tool_config;

pub use error::HarnessError;
pub use hub::EventHub;
pub use registry::{InstanceConfig, InstanceRegistry};
pub use sessions::{OpenThread, SessionManager};
pub use slots::{Delivery, KeyHolder, Open, Opened, Origin, Reported, Sent, SlotManager, SlotReport, SlotSettings, SlotState, SlotStore};

use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use harness_contracts::{
    CatalogUpdate, Capabilities, DriverKind, InstanceId, NativeId, ProviderSnapshot, RequestId,
    RequestResponse, SavedSessionPage, SessionConfig, SteerInput, ThreadId, TurnId, TurnInput,
};
use tokio::sync::watch;

pub type Result<T> = std::result::Result<T, HarnessError>;

/// Static facts about a driver, shown before any instance exists.
#[derive(Debug, Clone)]
pub struct DriverMetadata {
    pub kind: DriverKind,
    pub display_name: String,
    pub supports_multiple_instances: bool,
}

/// Factory for one harness kind. Registered in the server's built-in driver list.
#[async_trait]
pub trait ProviderDriver: Send + Sync {
    fn metadata(&self) -> DriverMetadata;

    /// An instance's config for a harness this driver runs: its profile
    /// (from its pack) and how it's started (its pack's launch, or the
    /// instance's own), or why it can't be.
    fn default_config(&self, harness: &harness_contracts::HarnessEntry) -> serde_json::Value;

    /// Decodes `config` (driver-defined shape) and builds an instance.
    async fn create(&self, id: InstanceId, config: serde_json::Value) -> Result<Arc<dyn ProviderInstance>>;
}

/// One configured harness: everything except live sessions, plus the adapter for them.
#[async_trait]
pub trait ProviderInstance: Send + Sync {
    fn id(&self) -> &InstanceId;

    /// Live snapshot (status, auth, models, commands, skills). Updated by the
    /// instance itself and by `CatalogUpdated` events from its sessions.
    fn snapshot(&self) -> watch::Receiver<ProviderSnapshot>;

    /// Re-probe install/auth/catalog.
    async fn refresh(&self) -> Result<()>;

    /// Present when the harness needs an interactive login.
    fn auth(&self) -> Option<Arc<dyn AuthController>>;

    fn adapter(&self) -> Arc<dyn ProviderAdapter>;

    /// Folds a catalog change reported by one of this instance's sessions into
    /// the snapshot.
    fn apply_catalog(&self, update: &CatalogUpdate);

    /// Present when the harness keeps its own saved sessions (the way back into
    /// earlier work; the harness layer itself persists nothing).
    fn sessions(&self) -> Option<Arc<dyn SessionCatalog>>;
}

/// The harness's own saved sessions.
#[async_trait]
pub trait SessionCatalog: Send + Sync {
    async fn list(&self, cwd: &str, cursor: Option<&str>) -> Result<SavedSessionPage>;

    async fn delete(&self, id: &NativeId) -> Result<()>;
}

/// Interactive login for an instance.
#[async_trait]
pub trait AuthController: Send + Sync {
    /// Starts a login with one of the snapshot's `AuthMethod`s.
    async fn begin(&self, method_id: &str) -> Result<AuthChallenge>;

    /// Completes a login, e.g. with a pasted callback URL or code.
    async fn complete(&self, response: &str) -> Result<()>;

    async fn logout(&self) -> Result<()>;
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallenge {
    /// URL for the user to open, if the method is browser-based.
    pub url: Option<String>,
    pub instructions: String,
    /// What `complete` takes, in words (the page's last URL, a code), when
    /// it takes anything.
    pub expects: Option<String>,
}

/// Opens sessions. Stateless apart from what the instance gave it.
#[async_trait]
pub trait ProviderAdapter: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    async fn open_session(&self, config: SessionConfig) -> Result<Box<dyn SessionRuntime>>;
}

/// One live session. Operations a harness lacks return `HarnessError::Unsupported`;
/// callers check `Capabilities` first.
#[async_trait]
pub trait SessionRuntime: Send + Sync {
    fn thread_id(&self) -> &ThreadId;
    /// The harness's own id for this session (to resume it later), if it has one.
    fn native_id(&self) -> Option<NativeId>;

    /// Entity-upsert events for this session. Each call returns a new subscriber.
    fn events(&self) -> BoxStream<'static, harness_contracts::AdapterEvent>;

    /// Starts a turn; while one runs, queues it when the harness queues
    /// messages (`TurnCapabilities::queued_messages`).
    async fn start_turn(&self, input: TurnInput) -> Result<TurnId>;

    /// Adds a message to the running turn (`TurnCapabilities::active_steering`);
    /// with none running, starts one.
    async fn inject(&self, input: TurnInput) -> Result<TurnId> {
        let _ = input;
        Err(HarnessError::Unsupported { operation: "adding to a running turn" })
    }

    async fn steer(&self, input: SteerInput) -> Result<()>;

    async fn interrupt(&self) -> Result<()>;

    async fn respond(&self, request: &RequestId, response: RequestResponse) -> Result<()>;

    async fn close(&self) -> Result<()>;
}
