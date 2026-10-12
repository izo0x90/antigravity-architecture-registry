//! How a harness is started: a program, its arguments and environment
//! (from its pack, or an instance's own override), or why it can't be.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
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
