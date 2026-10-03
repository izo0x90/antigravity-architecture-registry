//! Adapter events: whole-entity upserts (not deltas). Each carries the latest state
//! of one entity, so clients and the server just replace by id; replay is trivial.

use serde::{Deserialize, Serialize};

use crate::ids::{ThreadId, TurnId};
use crate::snapshot::ProviderSnapshot;
use crate::items::TurnItem;
use crate::plan::PlanArtifact;
use crate::requests::RuntimeRequest;
use crate::session::{Subagent, Thread, Turn, Usage};
use crate::snapshot::{ModelOption, SlashCommand};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum AdapterEvent {
    ThreadUpdated { thread: Thread },
    TurnUpdated { turn: Turn },
    ItemUpdated { item: TurnItem },
    RequestUpdated { request: RuntimeRequest },
    PlanUpdated { plan: PlanArtifact },
    SubagentUpdated { subagent: Subagent },
    UsageUpdated { thread_id: ThreadId, turn_id: Option<TurnId>, usage: Usage },
    /// The live session changed the instance catalog (e.g. ACP
    /// `available_commands_update`); the instance folds it into its snapshot.
    CatalogUpdated { update: CatalogUpdate },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogUpdate {
    pub commands: Option<Vec<SlashCommand>>,
    pub models: Option<Vec<ModelOption>>,
}

/// What the server streams to clients: instance snapshots plus session events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub enum ClientEvent {
    InstanceUpdated { snapshot: ProviderSnapshot },
    Session { event: AdapterEvent },
    ThreadClosed { thread_id: ThreadId },
    /// The architecture graph changed, was checked, or someone asked the UI to
    /// show part of it.
    Architecture { event: arch_graph::ArchEvent },
}
