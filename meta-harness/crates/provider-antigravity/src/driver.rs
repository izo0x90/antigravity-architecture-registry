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
    /// Path to `agy_acp_server.par` (or a wrapper that runs it).
    pub executable: Option<PathBuf>,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// `AGY_EXECUTABLE`, else the ACP server under the user's local install dirs.
/// (The `agy` CLI is not an ACP server and is deliberately not considered.)
pub(crate) fn find_acp_server() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AGY_EXECUTABLE") {
        return Some(PathBuf::from(path));
    }
    let home = PathBuf::from(std::env::var("HOME").ok()?);
    [".local/share/antigravity-acp/agy_acp_server.par", ".local/bin/agy_acp_server.par"]
        .into_iter()
        .map(|rel| home.join(rel))
        .find(|p| p.is_file())
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

    fn default_config(&self) -> serde_json::Value {
        serde_json::to_value(AntigravityConfig::default()).unwrap_or_default()
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
