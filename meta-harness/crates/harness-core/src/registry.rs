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
    /// The config each instance was made with.
    configs: RwLock<BTreeMap<InstanceId, serde_json::Value>>,
    hub: Arc<EventHub>,
}

impl InstanceRegistry {
    pub fn new(drivers: Vec<Arc<dyn ProviderDriver>>, hub: Arc<EventHub>) -> Self {
        let drivers = drivers.into_iter().map(|d| (d.metadata().kind, d)).collect();
        Self { drivers, instances: RwLock::default(), configs: RwLock::default(), hub }
    }

    /// One instance per harness (its profile names the instance and the
    /// driver), configured by its driver. A harness whose driver isn't
    /// built in is left out, and logged.
    pub fn default_configs(&self, harnesses: &[harness_contracts::HarnessEntry]) -> Vec<InstanceConfig> {
        let mut configs: Vec<InstanceConfig> = harnesses
            .iter()
            .filter_map(|entry| {
                let profile = &entry.profile;
                let Some(driver) = self.drivers.get(&profile.driver) else {
                    tracing::warn!(target: "harness_core", "{} needs driver {}, which isn't built in", profile.instance, profile.driver);
                    return None;
                };
                Some(InstanceConfig { id: profile.instance.clone(), driver: profile.driver.clone(), config: driver.default_config(entry) })
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
            let instance = driver.create(config.id.clone(), config.config.clone()).await?;
            self.configs.write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(config.id.clone(), config.config);
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

    /// Makes the instance again when its config differs from the one it was
    /// made with (its harness was installed or fixed since); true if it was.
    pub async fn reconfigure(&self, config: InstanceConfig) -> Result<bool> {
        let same = self.configs.read().unwrap_or_else(std::sync::PoisonError::into_inner).get(&config.id) == Some(&config.config);
        if same {
            return Ok(false);
        }
        self.configure(vec![config]).await?;
        Ok(true)
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
