//! The kinds of buffer: what a panel can show.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferKind {
    Chat,
    Code,
    Graph,
    Tree,
    Spec,
    Queue,
    Inbox,
    Changes,
    Terminal,
    Tags,
}

impl BufferKind {
    pub const ALL: [Self; 10] = [Self::Chat, Self::Code, Self::Graph, Self::Tree, Self::Spec, Self::Queue, Self::Inbox, Self::Changes, Self::Terminal, Self::Tags];

    pub fn title(&self) -> &'static str {
        match self {
            Self::Chat => "CHAT",
            Self::Code => "CODE",
            Self::Graph => "GRAPH",
            Self::Tree => "TREE",
            Self::Spec => "SPEC",
            Self::Queue => "QUEUE",
            Self::Inbox => "INBOX",
            Self::Changes => "CHANGES",
            Self::Terminal => "TERMINAL",
            Self::Tags => "TAGS",
        }
    }

    /// There is one of these (the queue, the inbox, the changes: the
    /// desk's comparison, the tags); of the others, each panel can have its own.
    pub fn one_only(&self) -> bool {
        matches!(self, Self::Queue | Self::Inbox | Self::Changes | Self::Tags)
    }

    /// Lowercase name used in action ids ("buffer.chat").
    pub fn name(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Code => "code",
            Self::Graph => "graph",
            Self::Tree => "tree",
            Self::Spec => "spec",
            Self::Queue => "queue",
            Self::Inbox => "inbox",
            Self::Changes => "changes",
            Self::Terminal => "terminal",
            Self::Tags => "tags",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Self::Chat => egui_phosphor::regular::CHAT_TEARDROP_TEXT,
            Self::Code => egui_phosphor::regular::CODE,
            Self::Graph => egui_phosphor::regular::GIT_FORK,
            Self::Tree => egui_phosphor::regular::TREE_STRUCTURE,
            Self::Spec => egui_phosphor::regular::ARTICLE,
            Self::Queue => egui_phosphor::regular::LIST_CHECKS,
            Self::Inbox => egui_phosphor::regular::TRAY,
            Self::Changes => egui_phosphor::regular::GIT_DIFF,
            Self::Terminal => egui_phosphor::regular::TERMINAL_WINDOW,
            Self::Tags => egui_phosphor::regular::TAG,
        }
    }
}
