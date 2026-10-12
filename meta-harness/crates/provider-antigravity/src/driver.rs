//! `ProviderDriver` for Antigravity: decodes instance config and creates instances.

use std::sync::Arc;

use async_trait::async_trait;
use harness_contracts::{DriverKind, HarnessLaunch, InstanceId, Launch};
use harness_core::{DriverMetadata, HarnessError, ProviderDriver, ProviderInstance, Result};
use serde::{Deserialize, Serialize};

use crate::instance::{AntigravityInstance, DRIVER_KIND};

/// Instance configuration: how the ACP server is started (by default, its
/// harness pack's launch), or why it can't be.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AntigravityConfig {
    pub display_name: Option<String>,
    pub launch: Option<Launch>,
    /// Why there's no launch, and the command that fixes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

pub struct AntigravityDriver;

#[async_trait]
impl ProviderDriver for AntigravityDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata {
            kind: DriverKind::new(DRIVER_KIND),
            display_name: "Google Antigravity".into(),
            supports_multiple_instances: true,
        }
    }

    fn default_config(&self, harness: &HarnessLaunch) -> serde_json::Value {
        let config = match harness {
            HarnessLaunch::Ready { launch } => AntigravityConfig { launch: Some(launch.clone()), ..AntigravityConfig::default() },
            HarnessLaunch::Unavailable { reason, fix } => AntigravityConfig { unavailable: Some(HarnessLaunch::describe_unavailable(reason, fix.as_deref())), ..AntigravityConfig::default() },
        };
        serde_json::to_value(config).unwrap_or_default()
    }

    async fn create(&self, id: InstanceId, config: serde_json::Value) -> Result<Arc<dyn ProviderInstance>> {
        let config: AntigravityConfig =
            serde_json::from_value(config).map_err(|e| HarnessError::InvalidConfig { reason: e.to_string() })?;
        let instance = AntigravityInstance::create(id, config);
        let probe_target = instance.clone();
        tokio::spawn(async move {
            if let Err(err) = probe_target.refresh().await {
                tracing::warn!(target: "provider_antigravity", "initial refresh failed: {err}");
            }
        });
        Ok(instance)
    }
}
