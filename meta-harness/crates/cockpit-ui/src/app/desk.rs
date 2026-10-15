//! A desk: one server's project as the cockpit shows it: its views,
//! their layout, the marks and jumps, and the calls to its server. The
//! shell (keys, modes, the palette, notes, the quick look) works on the
//! desk shown.

use super::*;

pub struct Desk {
    /// Its server's name (`here`: home's own).
    pub server: String,
    /// It was shown once (and loaded what it shows then).
    pub opened: bool,
    pub net: NetClient,
    pub chat: ChatStreamState,
    pub code: CodeViewerState,
    pub graph: GraphState,
    pub spec: SpecState,
    pub tree: ArchitectureTreeState,
    /// The current plan's comments and changes since review.
    pub review: ReviewState,
    pub queue: crate::queue::QueueState,
    /// What needs the user, one at a time.
    pub inbox: crate::inbox::InboxView,
    pub review_seen: u64,
    /// The columns: the view each shows, and which has focus.
    pub columns: Columns,
    /// The views not in use now (each column's own; see `panes`).
    pub parked: Parked,
    /// For each kind with a view per column: the view in use now, and the
    /// one used last (what keys, links and the agent act on).
    pub active: BTreeMap<BufferKind, u32>,
    pub last_used: BTreeMap<BufferKind, u32>,
    /// New views, to start where the view of their kind used before was.
    pub fresh: Vec<(Pane, Place)>,
    /// The component types, for each spec view.
    pub types: Option<arch_graph::TypeSystem>,
    /// Loads and saves the cockpit's state on the server.
    pub keeper: crate::saved::Keeper,
    /// Calls from outside: scripts in the page, and agents while allowed.
    pub remote: crate::remote::Remote,
    /// Where jumps left from, across views and slots.
    pub jumps: JumpList,
    /// Marks by letter.
    pub marks: BTreeMap<char, Jump>,
    /// The last search in each view (one that found something), and the
    /// last that found nothing (so `n` can say so).
    pub searches: BTreeMap<BufferKind, String>,
    pub missed: BTreeMap<BufferKind, String>,
    /// Go-tos: component details, and one under way or being picked.
    pub details: Details,
    pub pending_goto: Option<Pending>,
    pub choosing: Option<Choosing>,
    pub at_inbox: Arc<Mutex<Vec<NetResult<arch_graph::Component>>>>,
    /// Code indexing: running or not, progress, last run, stale files.
    pub index: arch_graph::IndexStatus,
    pub index_inbox: Arc<Mutex<Vec<NetResult<arch_graph::IndexStatus>>>>,
    pub types_inbox: Arc<Mutex<Option<NetResult<arch_graph::TypeSystem>>>>,
    /// The spec's anchor we last refetched the view for (so a mismatch the
    /// server itself holds doesn't refetch every frame).
    pub anchor_checked: Option<Option<arch_graph::CodeAnchor>>,
}

impl Desk {
    /// Server `name`'s desk, its calls going through home at `host_origin`.
    pub fn new(host_origin: &str, name: &str) -> Self {
        Self {
            server: name.to_owned(),
            opened: false,
            net: NetClient::for_server(host_origin, name),
            chat: ChatStreamState::default(),
            code: CodeViewerState::default(),
            graph: GraphState::default(),
            spec: SpecState::default(),
            tree: ArchitectureTreeState::default(),
            review: ReviewState::default(),
            queue: crate::queue::QueueState::default(),
            inbox: Default::default(),
            review_seen: 0,
            columns: Columns::default(),
            parked: Parked::default(),
            active: BTreeMap::new(),
            last_used: BTreeMap::new(),
            fresh: Vec::new(),
            types: None,
            keeper: Default::default(),
            remote: crate::remote::Remote::new(host_origin, name),
            jumps: JumpList::default(),
            marks: BTreeMap::new(),
            searches: BTreeMap::new(),
            missed: BTreeMap::new(),
            details: Details::default(),
            pending_goto: None,
            choosing: None,
            at_inbox: Arc::default(),
            index: arch_graph::IndexStatus::default(),
            index_inbox: Arc::default(),
            types_inbox: Arc::default(),
            anchor_checked: None,
        }
    }

    pub fn provider(&self, kind: BufferKind) -> &dyn ActionProvider {
        match kind {
            BufferKind::Chat => &self.chat,
            BufferKind::Code => &self.code,
            BufferKind::Graph => &self.graph,
            BufferKind::Tree => &self.tree,
            BufferKind::Spec => &self.spec,
            BufferKind::Queue => &self.queue,
            BufferKind::Inbox => &self.inbox,
        }
    }

    pub fn provider_mut(&mut self, kind: BufferKind) -> &mut dyn ActionProvider {
        match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
            BufferKind::Queue => &mut self.queue,
            BufferKind::Inbox => &mut self.inbox,
        }
    }

    pub fn view(&self, kind: BufferKind) -> &dyn Navigable {
        match kind {
            BufferKind::Chat => &self.chat,
            BufferKind::Code => &self.code,
            BufferKind::Graph => &self.graph,
            BufferKind::Tree => &self.tree,
            BufferKind::Spec => &self.spec,
            BufferKind::Queue => &self.queue,
            BufferKind::Inbox => &self.inbox,
        }
    }

    pub fn view_mut(&mut self, kind: BufferKind) -> &mut dyn Navigable {
        match kind {
            BufferKind::Chat => &mut self.chat,
            BufferKind::Code => &mut self.code,
            BufferKind::Graph => &mut self.graph,
            BufferKind::Tree => &mut self.tree,
            BufferKind::Spec => &mut self.spec,
            BufferKind::Queue => &mut self.queue,
            BufferKind::Inbox => &mut self.inbox,
        }
    }

    /// The view of `kind` in use: the one used last.
    pub fn used_pane(&self, kind: BufferKind) -> Pane {
        Pane::new(kind, self.last_used.get(&kind).copied().unwrap_or(0))
    }

    /// Every graph (the one in use first).
    pub fn graphs(&mut self) -> impl Iterator<Item = &mut GraphState> {
        std::iter::once(&mut self.graph).chain(self.parked.graphs.values_mut())
    }

    pub fn codes(&mut self) -> impl Iterator<Item = &mut CodeViewerState> {
        std::iter::once(&mut self.code).chain(self.parked.codes.values_mut())
    }

    pub fn specs(&mut self) -> impl Iterator<Item = &mut SpecState> {
        std::iter::once(&mut self.spec).chain(self.parked.specs.values_mut())
    }

    pub fn trees(&mut self) -> impl Iterator<Item = &mut ArchitectureTreeState> {
        std::iter::once(&mut self.tree).chain(self.parked.trees.values_mut())
    }

    /// Whether `pane`'s spec view is pinned.
    pub fn pinned(&self, pane: Pane) -> bool {
        if self.active.get(&BufferKind::Spec).copied().unwrap_or(0) == pane.id { self.spec.pinned } else { self.parked.specs.get(&pane.id).is_some_and(|s| s.pinned) }
    }
}
