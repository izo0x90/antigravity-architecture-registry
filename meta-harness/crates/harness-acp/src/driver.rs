//! The driver for every ACP agent: an instance per harness pack that names
//! it, configured by that pack's profile (its settings say how the agent
//! differs) and started by its launch.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use harness_contracts::{DriverKind, HarnessEntry, HarnessLaunch, InstanceId};
use harness_core::{DriverMetadata, HarnessError, ProviderDriver, ProviderInstance, Result};

use crate::instance::{AcpInstance, InstanceParts};
use crate::launch::{AcpLauncher, ProcessLauncher};
use crate::profile::Profile;
use crate::settings::AcpSettings;

pub const DRIVER_KIND: &str = "acp";

pub struct AcpDriver {
    /// The project its agents work in.
    project: PathBuf,
}

impl AcpDriver {
    pub fn new(project: PathBuf) -> Self {
        Self { project }
    }
}

#[async_trait]
impl ProviderDriver for AcpDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata { kind: DriverKind::new(DRIVER_KIND), display_name: "ACP agent".into(), supports_multiple_instances: true }
    }

    /// Its config is the harness itself (profile and launch).
    fn default_config(&self, harness: &HarnessEntry) -> serde_json::Value {
        serde_json::to_value(harness).unwrap_or_default()
    }

    async fn create(&self, id: InstanceId, config: serde_json::Value) -> Result<Arc<dyn ProviderInstance>> {
        let entry: HarnessEntry = serde_json::from_value(config).map_err(|e| HarnessError::InvalidConfig { reason: e.to_string() })?;
        let launch = match &entry.launch {
            HarnessLaunch::Ready { launch } => Ok(launch.clone()),
            HarnessLaunch::Unavailable { reason, fix } => Err(HarnessLaunch::describe_unavailable(reason, fix.as_deref())),
        };
        // Wrong settings: it can't be started until they're fixed.
        let (settings, launch) = match AcpSettings::parse(&entry.profile.settings) {
            Ok(settings) => (settings, launch),
            Err(why) => (AcpSettings::default(), Err(why)),
        };
        let profile = Arc::new(Profile::new(settings, &self.project, launch.as_ref().ok()));
        let launcher = launch.map(|launch| (Arc::new(ProcessLauncher::from(&launch)) as Arc<dyn AcpLauncher>, Some(launch)));
        let instance = AcpInstance::create(InstanceParts { id, harness: entry.profile, profile, launcher });
        let first = instance.clone();
        tokio::spawn(async move {
            if let Err(err) = first.refresh().await {
                tracing::warn!(target: "harness_acp", "first check failed: {err}");
            }
        });
        Ok(instance)
    }
}
