use std::path::{Path, PathBuf};
use std::sync::Arc;
use async_trait::async_trait;
use harness_protocol::arch::{ComponentFilter, ComponentSpec, SystemArchitecture, UsageNode};
use tokio::sync::RwLock;

use crate::registry::error::RegistryError;
use crate::registry::traits::ArchitectureRepository;

/// File-backed JSON repository implementation with in-memory caching and atomic file writes.
#[derive(Clone)]
pub struct FsJsonRepository {
    file_path: PathBuf,
    cache: Arc<RwLock<SystemArchitecture>>,
}

impl FsJsonRepository {
    /// Creates a repository bound to `path`. If the file exists, it loads it;
    /// otherwise it initializes an empty `SystemArchitecture`.
    pub async fn new_or_init(path: impl AsRef<Path>) -> Result<Self, RegistryError> {
        let file_path = path.as_ref().to_path_buf();
        let arch = if file_path.exists() {
            let content = tokio::fs::read_to_string(&file_path).await?;
            serde_json::from_str::<SystemArchitecture>(&content)?
        } else {
            SystemArchitecture::default()
        };

        Ok(Self {
            file_path,
            cache: Arc::new(RwLock::new(arch)),
        })
    }

    /// Creates an in-memory repository with an arbitrary target file path (useful for testing).
    pub fn from_architecture(arch: SystemArchitecture, path: impl AsRef<Path>) -> Self {
        Self {
            file_path: path.as_ref().to_path_buf(),
            cache: Arc::new(RwLock::new(arch)),
        }
    }

    /// Atomically persists the current cache to disk using a temporary file and rename.
    async fn persist_locked(&self, arch: &SystemArchitecture) -> Result<(), RegistryError> {
        if let Some(parent) = self.file_path.parent()
            && !parent.as_os_str().is_empty()
            && !parent.exists()
        {
            tokio::fs::create_dir_all(parent).await?;
        }

        let formatted = serde_json::to_string_pretty(arch)?;
        let tmp_path = self.file_path.with_extension("tmp");

        tokio::fs::write(&tmp_path, formatted).await?;
        tokio::fs::rename(&tmp_path, &self.file_path).await?;
        Ok(())
    }
}

#[async_trait]
impl ArchitectureRepository for FsJsonRepository {
    async fn get_component(&self, id: &str) -> Result<Option<ComponentSpec>, RegistryError> {
        let lock = self.cache.read().await;
        Ok(lock.components.get(id).cloned())
    }

    async fn save_component(&self, comp: &ComponentSpec) -> Result<(), RegistryError> {
        let mut lock = self.cache.write().await;
        lock.components.insert(comp.id.clone(), comp.clone());
        self.persist_locked(&lock).await?;
        Ok(())
    }

    async fn delete_component(&self, id: &str) -> Result<(), RegistryError> {
        let mut lock = self.cache.write().await;
        if lock.components.remove(id).is_some() {
            self.persist_locked(&lock).await?;
        }
        Ok(())
    }

    async fn query_components(&self, filter: &ComponentFilter) -> Result<Vec<ComponentSpec>, RegistryError> {
        let lock = self.cache.read().await;
        let matched = lock
            .components
            .values()
            .filter(|c| filter.matches(c))
            .cloned()
            .collect();
        Ok(matched)
    }

    async fn get_usage_tree(&self, name: &str) -> Result<Option<UsageNode>, RegistryError> {
        let lock = self.cache.read().await;
        Ok(lock.usage_trees.get(name).cloned())
    }

    async fn save_usage_tree(&self, name: &str, tree: &UsageNode) -> Result<(), RegistryError> {
        let mut lock = self.cache.write().await;
        lock.usage_trees.insert(name.to_string(), tree.clone());
        self.persist_locked(&lock).await?;
        Ok(())
    }

    async fn delete_usage_tree(&self, name: &str) -> Result<(), RegistryError> {
        let mut lock = self.cache.write().await;
        if lock.usage_trees.remove(name).is_some() {
            self.persist_locked(&lock).await?;
        }
        Ok(())
    }

    async fn list_usage_trees(&self) -> Result<Vec<String>, RegistryError> {
        let lock = self.cache.read().await;
        Ok(lock.usage_trees.keys().cloned().collect())
    }

    async fn load_full(&self) -> Result<SystemArchitecture, RegistryError> {
        let lock = self.cache.read().await;
        Ok(lock.clone())
    }

    async fn save_full(&self, arch: &SystemArchitecture) -> Result<(), RegistryError> {
        let mut lock = self.cache.write().await;
        *lock = arch.clone();
        self.persist_locked(&lock).await?;
        Ok(())
    }
}
