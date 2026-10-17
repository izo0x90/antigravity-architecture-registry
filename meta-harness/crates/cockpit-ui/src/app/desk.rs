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
    /// Every buffer, by its id: shown in a panel, hidden, or the quick
    /// look's card. The shell drives them through `Buffer` alone.
    buffers: BTreeMap<BufferId, Box<dyn Buffer>>,
    /// The panels: the buffer each shows, and which has focus.
    pub panels: Panels,
    /// For each kind with a buffer per panel: the one used last (what
    /// keys, links and the agent act on), and the one being drawn (its
    /// panel's, while it's drawn).
    pub last_used: BTreeMap<BufferKind, u32>,
    active: BTreeMap<BufferKind, u32>,
    /// New buffers, to start where the buffer of their kind used before was.
    pub fresh: Vec<(BufferId, Place)>,
    /// What the buffers show in common (each reads what it needs).
    pub shared: crate::shared::Shared,
    /// Loads and saves the cockpit's state on the server.
    pub keeper: crate::saved::Keeper,
    /// Calls from outside: scripts in the page, and agents while allowed.
    pub remote: crate::remote::Remote,
    /// Where jumps left from, across buffers and slots.
    pub jumps: JumpList,
    /// Marks by letter.
    pub marks: BTreeMap<char, Jump>,
    /// The last search in each kind of buffer (one that found something),
    /// and the last that found nothing (so `n` can say so).
    pub searches: BTreeMap<BufferKind, String>,
    pub missed: BTreeMap<BufferKind, String>,
    /// Go-tos: component details, and one under way or being picked.
    pub details: Details,
    pub pending_goto: Option<Pending>,
    pub choosing: Option<Choosing>,
    pub at_inbox: Arc<Mutex<Vec<NetResult<arch_graph::Component>>>>,
    /// Code indexing's status, as asked for.
    pub index_inbox: Arc<Mutex<Vec<NetResult<arch_graph::IndexStatus>>>>,
    pub types_inbox: Arc<Mutex<Option<NetResult<arch_graph::TypeSystem>>>>,
}

impl Desk {
    /// Server `name`'s desk, its calls going through home at `host_origin`.
    pub fn new(host_origin: &str, name: &str) -> Self {
        let buffers = BufferKind::ALL.into_iter().map(|kind| (BufferId::new(kind, 0), crate::buffer::new(kind))).collect();
        Self {
            server: name.to_owned(),
            opened: false,
            net: NetClient::for_server(host_origin, name),
            buffers,
            panels: Panels::default(),
            last_used: BTreeMap::new(),
            active: BTreeMap::new(),
            fresh: Vec::new(),
            shared: crate::shared::Shared { server: name.to_owned(), ..Default::default() },
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
            index_inbox: Arc::default(),
            types_inbox: Arc::default(),
        }
    }

    /// The buffer of `kind` in use: the one used last.
    pub fn used_buffer(&self, kind: BufferKind) -> BufferId {
        BufferId::new(kind, if kind.one_only() { 0 } else { self.last_used.get(&kind).copied().unwrap_or(0) })
    }

    pub fn buffer(&self, id: BufferId) -> Option<&dyn Buffer> {
        self.buffers.get(&id).map(|b| b.as_ref())
    }

    pub fn buffer_mut(&mut self, id: BufferId) -> Option<&mut (dyn Buffer + 'static)> {
        self.buffers.get_mut(&id).map(|b| b.as_mut())
    }

    /// Makes `id`'s buffer if there's none yet; true when it's new.
    pub fn ensure(&mut self, id: BufferId) -> bool {
        let id = if id.kind.one_only() { BufferId::new(id.kind, 0) } else { id };
        if self.buffers.contains_key(&id) {
            return false;
        }
        self.buffers.insert(id, crate::buffer::new(id.kind));
        true
    }

    /// The buffer `from` is `to`'s from now on (the quick look's card,
    /// sent to a panel).
    pub fn rekey(&mut self, from: BufferId, to: BufferId) {
        if let Some(buffer) = self.buffers.remove(&from) {
            self.buffers.insert(to, buffer);
        }
    }

    /// `id`'s buffer, new again.
    pub fn renew(&mut self, id: BufferId) {
        self.buffers.insert(id, crate::buffer::new(id.kind));
    }

    /// The buffer of `kind` in use: the one being drawn, else the one
    /// used last.
    fn in_use(&self, kind: BufferKind) -> BufferId {
        match self.active.get(&kind) {
            Some(id) if !kind.one_only() => BufferId::new(kind, *id),
            _ => self.used_buffer(kind),
        }
    }

    pub fn view(&self, kind: BufferKind) -> &dyn Buffer {
        self.buffers.get(&self.in_use(kind)).expect("the buffer in use is kept").as_ref()
    }

    pub fn view_mut(&mut self, kind: BufferKind) -> &mut dyn Buffer {
        let id = self.in_use(kind);
        self.buffers.get_mut(&id).expect("the buffer in use is kept").as_mut()
    }

    /// The buffer of `kind` in use, and the desk's calls (to draw it).
    pub fn view_with_net(&mut self, kind: BufferKind) -> (&mut dyn Buffer, &NetClient) {
        let id = self.in_use(kind);
        (self.buffers.get_mut(&id).expect("the buffer in use is kept").as_mut(), &self.net)
    }

    /// `id` is the buffer of its kind in use while it's drawn.
    pub fn drawing(&mut self, id: BufferId) {
        self.active.insert(id.kind, id.id);
    }

    /// Drawn: the buffers used last are in use again.
    pub fn drawn(&mut self) {
        self.active.clear();
    }

    /// Every buffer, shown, hidden or the quick look's.
    pub fn each_buffer(&mut self, mut f: impl FnMut(&mut dyn Buffer)) {
        for buffer in self.buffers.values_mut() {
            f(buffer.as_mut());
        }
    }

    /// Every buffer, by its id.
    pub fn buffers(&self) -> impl Iterator<Item = (BufferId, &dyn Buffer)> {
        self.buffers.iter().map(|(id, b)| (*id, b.as_ref()))
    }

    /// Every buffer reads what they share.
    pub fn share(&mut self) {
        let Self { shared, buffers, .. } = self;
        for buffer in buffers.values_mut() {
            buffer.share(shared);
        }
    }

    /// Keeps only the buffers `held` says are (a panel shows or hides
    /// them); the one of each kind in use, and the first, are always kept.
    pub fn keep_only(&mut self, held: impl Fn(BufferId) -> bool) {
        let used: Vec<BufferId> = BufferKind::ALL.into_iter().map(|k| self.used_buffer(k)).collect();
        self.buffers.retain(|id, _| id.id == 0 || used.contains(id) || held(*id));
    }
}

