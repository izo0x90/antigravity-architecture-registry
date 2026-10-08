//! Configured provider instances, built from the registered drivers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use harness_contracts::{ClientEvent, DriverKind, InstanceId, ProviderSnapshot};

use crate::{EventHub, HarnessError, ProviderDriver, ProviderInstance, Result};

/// One configured instance: which driver, with what driver-defined config.
#[derive(Debug, Clone)]
pub struct InstanceConfig {
    pub id: InstanceId,
    pub driver: DriverKind,
    pub config: serde_json::Value,
}

pub struct InstanceRegistry {
    drivers: HashMap<DriverKind, Arc<dyn ProviderDriver>>,
    instances: RwLock<BTreeMap<InstanceId, Arc<dyn ProviderInstance>>>,
    hub: Arc<EventHub>,
}

impl InstanceRegistry {
    pub fn new(drivers: Vec<Arc<dyn ProviderDriver>>, hub: Arc<EventHub>) -> Self {
        let drivers = drivers.into_iter().map(|d| (d.metadata().kind, d)).collect();
        Self { drivers, instances: RwLock::default(), hub }
    }

    /// One instance per registered driver, with the driver's default config and
    /// the driver kind as instance id.
    /// One instance per driver, each given the program its harness pack
    /// installed (`programs`, by driver kind).
    pub fn default_configs(&self, programs: &std::collections::BTreeMap<String, std::path::PathBuf>) -> Vec<InstanceConfig> {
        let mut configs: Vec<InstanceConfig> = self
            .drivers
            .values()
            .map(|d| {
                let kind = d.metadata().kind;
                let config = d.default_config(programs.get(kind.as_str()).map(std::path::PathBuf::as_path));
                InstanceConfig { id: InstanceId::new(kind.as_str()), driver: kind, config }
            })
            .collect();
        configs.sort_by(|a, b| a.id.cmp(&b.id));
        configs
    }

    /// Creates the given instances and starts publishing their snapshots.
    pub async fn configure(&self, configs: Vec<InstanceConfig>) -> Result<()> {
        for config in configs {
            let driver = self
                .drivers
                .get(&config.driver)
                .ok_or_else(|| HarnessError::InvalidConfig { reason: format!("unknown driver {}", config.driver) })?;
            let instance = driver.create(config.id.clone(), config.config).await?;
            let mut snapshots = instance.snapshot();
            let hub = self.hub.clone();
            hub.publish(ClientEvent::InstanceUpdated { snapshot: snapshots.borrow_and_update().clone() });
            tokio::spawn(async move {
                while snapshots.changed().await.is_ok() {
                    let snapshot = snapshots.borrow_and_update().clone();
                    hub.publish(ClientEvent::InstanceUpdated { snapshot });
                }
            });
            self.write().insert(config.id, instance);
        }
        Ok(())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<InstanceId, Arc<dyn ProviderInstance>>> {
        self.instances.write().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<InstanceId, Arc<dyn ProviderInstance>>> {
        self.instances.read().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn get(&self, id: &InstanceId) -> Result<Arc<dyn ProviderInstance>> {
        self.read().get(id).cloned().ok_or_else(|| HarnessError::NotFound { what: format!("instance {id}") })
    }

    pub fn snapshots(&self) -> Vec<ProviderSnapshot> {
        self.read().values().map(|i| i.snapshot().borrow().clone()).collect()
    }
}
