//! Adapter events: whole-entity upserts (not deltas). Each carries the latest state
//! of one entity, so clients and the server just replace by id; replay is trivial.

use serde::{Deserialize, Serialize};

use crate::ids::{DiscussionId, SlotId, TagId, TerminalId, ThreadId, TurnId};
use crate::snapshot::ProviderSnapshot;
use crate::items::TurnItem;
use crate::notifications::Notification;
use crate::plan::PlanArtifact;
use crate::requests::RuntimeRequest;
use crate::session::{Subagent, Thread, Turn, Usage};
use crate::slots::Slot;
use crate::snapshot::{ModelOption, SignIn, SlashCommand};

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
    /// The harness reported who it's signed in as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<SignIn>,
}

/// An event, and when it happened on its server (milliseconds since the
/// epoch, its clock): what the server streams, each event so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamped {
    #[serde(rename = "atMs")]
    pub at_ms: u64,
    #[serde(flatten)]
    pub event: ClientEvent,
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
    /// A slot was made or changed (its sessions, its harness, its status).
    SlotUpdated { slot: Slot },
    SlotClosed { slot_id: SlotId },
    /// A notification appeared or changed state.
    NotificationUpdated { notification: Notification },
    /// A terminal was opened or changed (its title, its status, its size);
    /// its screen goes on its own socket.
    TerminalUpdated { terminal: crate::terminals::Terminal },
    TerminalClosed { terminal_id: TerminalId },
    /// Something a terminal's program or shell did (a bell, its end, a long
    /// command finishing): news, not kept.
    TerminalNews { terminal: crate::terminals::Terminal, news: crate::terminals::TerminalNews },
    /// A tag was made or changed (its name, colour, note, what it's on).
    TagUpdated { tag: crate::tags::Tag },
    TagRemoved { tag_id: TagId },
    /// A discussion was started or changed (a message, an answer, settled,
    /// reopened): all of it.
    DiscussionUpdated { discussion: crate::discussions::Discussion },
    DiscussionRemoved { discussion_id: DiscussionId },
    /// The stream fell behind: drop what you hold; the whole state follows.
    Resync,
    /// The state as it was (sent first, and after `Resync`) is all here:
    /// what follows happens now.
    Replayed,
}
