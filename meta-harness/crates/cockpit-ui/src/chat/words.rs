//! What each kind of thing the agent does is, in words, in one table: its
//! icon, what it did ("read", a file), what it's doing ("Reading"); and
//! what it's doing when it isn't at a call. The run's summary, the live
//! line and the call's line read it.

use egui_phosphor::regular as icon;
use harness_contracts::{ItemBody, ToolKind, TurnItem};

/// One kind, in words.
pub(super) struct Said {
    pub icon: &'static str,
    /// What it did, and to what (none: "searched twice").
    pub did: &'static str,
    pub thing: Option<&'static str>,
    /// What it's doing, and whether the file it's on follows.
    pub doing: &'static str,
    pub on_file: bool,
}

const fn said(icon: &'static str, did: &'static str, thing: Option<&'static str>, doing: &'static str, on_file: bool) -> Said {
    Said { icon, did, thing, doing, on_file }
}

/// What an item did, in words: a call's, reasoning's, and what a quiet
/// run folds besides (a sub-agent's message, a to-do list, a notice);
/// none for the rest.
pub(super) fn of(body: &ItemBody) -> Option<Said> {
    Some(match body {
        ItemBody::ToolCall { kind, .. } => match kind {
            ToolKind::Read => said(icon::BOOK_OPEN, "read", Some("file"), "Reading", true),
            ToolKind::Edit => said(icon::PENCIL_SIMPLE, "edited", Some("file"), "Editing", true),
            ToolKind::Delete => said(icon::TRASH, "deleted", Some("file"), "Deleting", true),
            ToolKind::Move => said(icon::ARROWS_LEFT_RIGHT, "moved", Some("file"), "Moving", true),
            ToolKind::Search => said(icon::MAGNIFYING_GLASS, "searched", None, "Searching", false),
            ToolKind::Execute => said(icon::PLAY, "ran", Some("command"), "Running a command", false),
            ToolKind::Think => said(icon::LIGHTBULB, "thought", None, "Thinking", false),
            ToolKind::Fetch => said(icon::GLOBE, "fetched", Some("page"), "Fetching", false),
            ToolKind::Other => said(icon::WRENCH, "used", Some("tool"), "Using a tool", false),
        },
        ItemBody::CommandExecution { .. } => said(icon::TERMINAL, "ran", Some("command"), "Running a command", false),
        ItemBody::FileChange { .. } => said(icon::PENCIL_SIMPLE, "changed", Some("file"), "Changing", true),
        ItemBody::Reasoning { .. } => said(icon::BRAIN, "thought", None, "Thinking", false),
        ItemBody::AssistantMessage { .. } => said(icon::CHAT_TEXT, "wrote", Some("message"), "Writing", false),
        ItemBody::TodoList { .. } => said(icon::LIST_CHECKS, "updated", Some("to-do list"), "Planning", false),
        ItemBody::Notice { .. } | ItemBody::Subagent { .. } | ItemBody::Unknown { .. } => said(icon::INFO, "noted", None, "Working", false),
        _ => return None,
    })
}

/// What it's doing at none of the calls: before its first item, writing
/// a message, anything else.
pub(super) const STARTING: &str = "Starting";
pub(super) const WRITING: &str = "Writing";
pub(super) const WORKING: &str = "Working";

/// What `item` is doing, in a word or two ("Reading render.rs"; another
/// tool, by its title); none for what isn't a call or reasoning.
pub(super) fn doing(item: &TurnItem) -> Option<String> {
    let said = of(&item.body)?;
    let path = match &item.body {
        ItemBody::ToolCall { locations, .. } => locations.first().map(|l| l.path.as_str()),
        ItemBody::FileChange { path, .. } => Some(path.as_str()),
        _ => None,
    };
    let file = path.map(|p| p.rsplit('/').next().unwrap_or(p)).filter(|_| said.on_file);
    Some(match (&item.body, file) {
        (ItemBody::ToolCall { kind: ToolKind::Other, .. }, _) => item.title.clone().unwrap_or_else(|| said.doing.to_owned()),
        (_, Some(file)) => format!("{} {file}", said.doing),
        _ => said.doing.to_owned(),
    })
}
