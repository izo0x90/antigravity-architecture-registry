use async_trait::async_trait;
use harness_protocol::arch::{ComponentFilter, ComponentSpec, SystemArchitecture, UsageNode};
use crate::registry::error::RegistryError;

/// Storage-agnostic abstraction for Architecture Registry persistence.
/// Mediates between the domain aggregates and physical storage (JSON files, SQLite, KV, DB).
#[async_trait]
pub trait ArchitectureRepository: Send + Sync {
    // -------------------------------------------------------------------------
    // Component Aggregate
    // -------------------------------------------------------------------------

    /// Fetches a component by its unique ID.
    async fn get_component(&self, id: &str) -> Result<Option<ComponentSpec>, RegistryError>;

    /// Saves or updates a component (upsert).
    async fn save_component(&self, comp: &ComponentSpec) -> Result<(), RegistryError>;

    /// Removes a component by its unique ID.
    async fn delete_component(&self, id: &str) -> Result<(), RegistryError>;

    /// Queries components matching the given filter.
    async fn query_components(&self, filter: &ComponentFilter) -> Result<Vec<ComponentSpec>, RegistryError>;

    // -------------------------------------------------------------------------
    // Usage Tree Aggregate
    // -------------------------------------------------------------------------

    /// Fetches a usage call tree by its root name.
    async fn get_usage_tree(&self, name: &str) -> Result<Option<UsageNode>, RegistryError>;

    /// Saves or updates a usage call tree.
    async fn save_usage_tree(&self, name: &str, tree: &UsageNode) -> Result<(), RegistryError>;

    /// Deletes a usage call tree by its root name.
    async fn delete_usage_tree(&self, name: &str) -> Result<(), RegistryError>;

    /// Lists all registered usage tree names.
    async fn list_usage_trees(&self) -> Result<Vec<String>, RegistryError>;

    // -------------------------------------------------------------------------
    // Full Snapshot
    // -------------------------------------------------------------------------

    /// Loads the entire SystemArchitecture snapshot.
    async fn load_full(&self) -> Result<SystemArchitecture, RegistryError>;

    /// Overwrites the full architecture atomically.
    async fn save_full(&self, arch: &SystemArchitecture) -> Result<(), RegistryError>;
}
