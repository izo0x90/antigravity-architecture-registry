//! The harnesses the server can run: each one's profile (which instance
//! it is, the driver that runs it and that driver's settings, from its
//! pack) and how it's started (a program, its arguments and environment,
//! from its pack or an instance's own override), or why it can't be.

use serde::{Deserialize, Serialize};

use crate::ids::{DriverKind, InstanceId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Variables of the server's environment the harness doesn't get (one
    /// that would take precedence over its own login, say).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_remove: Vec<String>,
}

/// One harness the server can run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessEntry {
    pub profile: HarnessProfile,
    pub launch: HarnessLaunch,
    /// The folder the project keeps its home in (its login, settings,
    /// sessions); none: its own folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<std::path::PathBuf>,
    /// What of the project its file access never reaches (the server's own).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub private: Vec<std::path::PathBuf>,
}

/// Which harness it is, from its pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessProfile {
    /// The instance it is (one per harness pack).
    pub instance: InstanceId,
    /// The driver that runs it.
    pub driver: DriverKind,
    pub display_name: String,
    /// The driver's settings for it (their shape is the driver's).
    #[serde(default)]
    pub settings: serde_json::Value,
}

/// What a driver's instance is given: how to start its harness, or why
/// there's no way to, and the command that fixes that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum HarnessLaunch {
    Ready { launch: Launch },
    Unavailable { reason: String, fix: Option<String> },
}

impl HarnessLaunch {
    /// The reason and fix as one line, for a status.
    pub fn describe_unavailable(reason: &str, fix: Option<&str>) -> String {
        match fix {
            Some(fix) => format!("{reason} (fix: {fix})"),
            None => reason.to_owned(),
        }
    }
}
