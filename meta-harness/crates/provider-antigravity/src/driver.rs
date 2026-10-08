//! `ProviderDriver` for Antigravity: decodes instance config and creates instances.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use harness_contracts::{DriverKind, InstanceId};
use harness_core::{DriverMetadata, HarnessError, ProviderDriver, ProviderInstance, Result};
use serde::{Deserialize, Serialize};

use crate::instance::{AntigravityInstance, DRIVER_KIND};

/// Instance configuration. Every field is optional; defaults find the ACP server
/// installed by the install scripts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AntigravityConfig {
    pub display_name: Option<String>,
    /// Path to `agy_acp_server.par` (or a wrapper that runs it); by
    /// default the program its harness pack installed.
    pub executable: Option<PathBuf>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
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

    fn default_config(&self, program: Option<&std::path::Path>) -> serde_json::Value {
        serde_json::to_value(AntigravityConfig { executable: program.map(std::path::Path::to_path_buf), ..AntigravityConfig::default() }).unwrap_or_default()
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
