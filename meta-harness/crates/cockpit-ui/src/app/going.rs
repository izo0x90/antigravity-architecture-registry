//! Go-tos: from what the cursor is on to where it leads.

use super::*;

impl CockpitApp {
    pub(super) fn component_name(&self, id: &ComponentId) -> String {
        self.desk.graph.component(id).map(|c| c.name.clone()).unwrap_or_else(|| {
            let symbol = id.as_str().split_once(':').map_or(id.as_str(), |(_, s)| s);
            symbol.rsplit([':', '.']).next().unwrap_or(symbol).to_owned()
        })
    }

    /// A go-to's count names the column it lands in (1 is the leftmost; one
    /// past the last adds a column). Not the column it starts from: false,
    /// and the user is told why.
    pub(super) fn land_in(&mut self, column: usize) -> bool {
        let count = self.desk.columns.len();
        let Some(slot) = column.checked_sub(1) else { return true };
        if slot == self.desk.columns.focused() {
            self.note = Some(format!("Column {column} is this one: give another column's number (1 to {count}, or {} for a new one).", count + 1));
            return false;
        }
        if slot > count || slot >= self.config.columns.max {
            self.note = Some(format!("There are {count} columns: give 1 to {count}, or {} for a new one.", count + 1));
            return false;
        }
        if slot == count {
            self.desk.columns.set_count(count + 1, self.config.columns.max);
        }
        self.landing = Some(slot);
        true
    }

    /// Takes what the cursor is on where a go-to key says.
    pub(super) fn goto(&mut self, op: GoTo, ctx: &egui::Context) {
        let origin = self.focused_kind();
        let from = self.jump_here();
        let Some(target) = self.desk.view(origin).target() else {
            self.note = Some("Nothing under the cursor.".into());
            return;
        };
        // A message's changes: its turn's, compared.
        if let (GoTo::Changes, Target::Message { turn: Some(turn), .. }) = (op, &target) {
            match self.desk.changes.turn(turn).map(|t| t.comparison.clone()) {
                Some(comparison) => {
                    self.desk.changes.compare(comparison);
                    self.desk.changes.show_timeline(false);
                    self.focus_kind(BufferKind::Changes);
                    self.after_jump(from);
                }
                None => self.note = Some("The timeline didn't see this turn's start.".into()),
            }
            return;
        }
        if op == GoTo::Code {
            // A call goes to what it calls; a file link opens as it is.
            if let Target::Line { refs, .. } = &target
                && !refs.is_empty()
            {
                let refs = refs.iter().map(|id| Candidate { id: id.clone(), line: None }).collect();
                self.choose(op, refs, from, origin, ctx);
                return;
            }
            // A link to a file, or a change: the file, at that line.
            if let (Target::Message { .. } | Target::Change { .. }, Some((file, line))) = (&target, target.code()) {
                self.focus_kind(BufferKind::Code);
                self.desk.code.load_file(&file, line, &self.desk.net, ctx.clone());
                self.after_jump(from);
                return;
            }
        }
        match (target.component().cloned(), &target) {
            (Some(id), _) => self.goto_component(op, id, from, origin, ctx),
            // A line the index has nothing on yet: ask the server what's there.
            (None, Target::Line { file, line, .. } | Target::Change { file, line }) => {
                let inbox = self.desk.at_inbox.clone();
                self.desk.net.arch_at(file, *line, &self.desk.graph.nav.source, ctx.clone(), move |r| {
                    if let Ok(mut i) = inbox.lock() {
                        i.push(r);
                    }
                });
                self.desk.pending_goto = Some(Pending { wait: Wait::At { op }, from, origin, column: self.landing });
            }
            (None, _) => self.note = Some("There's no component here.".into()),
        }
    }

    pub(super) fn goto_component(&mut self, op: GoTo, id: ComponentId, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        match op {
            GoTo::Code => self.open_component_code(id, None, from, ctx),
            GoTo::Spec => self.show_in(BufferKind::Spec, id, from),
            GoTo::Graph => self.show_in(BufferKind::Graph, id, from),
            GoTo::Tree => self.show_in(BufferKind::Tree, id, from),
            GoTo::Queue => self.show_in(BufferKind::Queue, id, from),
            GoTo::Changes => self.show_in(BufferKind::Changes, id, from),
            _ => {
                let source = self.desk.graph.nav.source.clone();
                match self.desk.details.get(&id, &source) {
                    Some(detail) => {
                        let found = candidates(op, detail);
                        if found.is_empty() {
                            self.note = Some(format!("{} has no {}.", self.component_name(&id), op.noun()));
                        } else {
                            self.choose(op, found, from, origin, ctx);
                        }
                    }
                    None => {
                        self.desk.details.want(&id, &source, &self.desk.net, ctx);
                        self.desk.pending_goto = Some(Pending { wait: Wait::Relations { op, component: id }, from, origin, column: self.landing });
                    }
                }
            }
        }
    }

    /// One place: go. Several: the palette asks which.
    pub(super) fn choose(&mut self, op: GoTo, found: Vec<Candidate>, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        match found.len() {
            0 => {}
            1 => self.arrive(op, found[0].clone(), from, origin, ctx),
            _ => {
                self.desk.choosing = Some(Choosing { op, from, origin, candidates: found, column: self.landing });
                if let Some(pick) = self.bound_actions().into_iter().find(|b| b.action.id == "goto.pick") {
                    self.open_palette(Some(&pick));
                }
            }
        }
    }

    pub(super) fn goto_pick(&mut self, value: Option<String>, ctx: &egui::Context) {
        let (Some(choosing), Some(value)) = (self.desk.choosing.take(), value) else { return };
        if let Some(c) = choosing.candidates.iter().find(|c| c.id.as_str() == value) {
            self.landing = choosing.column;
            self.arrive(choosing.op, c.clone(), choosing.from, choosing.origin, ctx);
            self.landing = None;
        }
    }

    /// Lands a go-to: in the code for `gd` (and anything from the code view),
    /// else in the same kind of view it started in, when that view can show
    /// components.
    pub(super) fn arrive(&mut self, op: GoTo, to: Candidate, from: Jump, origin: BufferKind, ctx: &egui::Context) {
        if op == GoTo::Code || origin == BufferKind::Code {
            self.open_component_code(to.id, to.line, from, ctx);
            return;
        }
        // In the column a count named: the same kind of view, there.
        if self.landing.is_some() {
            let kind = if matches!(origin, BufferKind::Graph | BufferKind::Tree | BufferKind::Spec) { origin } else { BufferKind::Graph };
            return self.show_in(kind, to.id, from);
        }
        match origin {
            BufferKind::Graph => self.desk.graph.reveal(to.id),
            BufferKind::Tree => self.desk.tree.reveal(&to.id, Scroll::Center),
            BufferKind::Spec => {
                if !self.desk.spec.pinned {
                    self.desk.graph.reveal(to.id.clone());
                }
                self.desk.spec.show(to.id);
            }
            BufferKind::Queue if self.desk.queue.select_component(&to.id) => {}
            BufferKind::Queue | BufferKind::Chat | BufferKind::Code | BufferKind::Inbox | BufferKind::Changes => return self.show_in(BufferKind::Graph, to.id, from),
        }
        self.after_jump(from);
    }

    /// Shows a component in a view and moves focus there.
    pub(super) fn show_in(&mut self, kind: BufferKind, id: ComponentId, from: Jump) {
        match kind {
            BufferKind::Code | BufferKind::Chat | BufferKind::Inbox => return,
            BufferKind::Changes if !self.desk.changes.reveal(&id) => {
                self.note = Some(format!("The changes ({}) don't touch {}.", self.desk.changes.comparison, self.component_name(&id)));
                return;
            }
            BufferKind::Queue if !self.desk.queue.select_component(&id) => {
                self.note = Some(match &self.desk.queue.plan {
                    None => "No plan, so no queue: pick a plan in the graph.".to_owned(),
                    Some(_) => format!("{} has no job in the queue.", self.component_name(&id)),
                });
                return;
            }
            _ => {}
        }
        // The view it lands in first (a column's own), then shown there.
        self.focus_kind(kind);
        match kind {
            BufferKind::Graph => self.desk.graph.reveal(id),
            BufferKind::Spec => {
                if !self.desk.spec.pinned {
                    self.desk.graph.reveal(id.clone());
                }
                self.desk.spec.show(id);
            }
            BufferKind::Tree => self.desk.tree.reveal(&id, Scroll::Center),
            BufferKind::Queue | BufferKind::Code | BufferKind::Chat | BufferKind::Inbox | BufferKind::Changes => {}
        }
        self.after_jump(from);
    }

    /// Where a component's code is, from whatever here knows it (the code
    /// view's file, the graph, fetched details): `Some(None)` when they know
    /// the component but it isn't in the code, `None` when nothing here
    /// knows it.
    pub(super) fn code_location(&self, id: &ComponentId) -> Option<Option<(String, u32, u32)>> {
        let of = |c: &arch_graph::Component| match (&c.anchor, &c.proposed) {
            (Some(a), _) if crate::status::is_folder(a) => None,
            (Some(a), _) => Some((a.file.to_string(), a.span.start_line, a.span.end_line)),
            (None, Some(p)) => Some((p.file.to_string(), 1, 1)),
            (None, None) => None,
        };
        let in_file = self.desk.code.defs.iter().find(|d| &d.id == id).map(|d| Some((self.desk.code.current_file.clone(), d.start as u32, d.end as u32)));
        let known = [in_file, self.desk.graph.component(id).map(of), self.desk.details.get(id, &self.desk.graph.nav.source).map(|d| of(&d.component))];
        let mut answer = None;
        for found in known.into_iter().flatten() {
            if found.is_some() {
                return Some(found);
            }
            answer = Some(None);
        }
        answer
    }

    /// Opens a component's code (at `line` in its file, else its span) and
    /// moves focus there; waits for its detail when it isn't known yet.
    pub(super) fn open_component_code(&mut self, id: ComponentId, line: Option<u32>, from: Jump, ctx: &egui::Context) {
        match self.code_location(&id) {
            Some(Some((file, start, end))) => {
                let (start, end) = line.map_or((start, end), |l| (l, l));
                self.show_code(&file, start, end, ctx);
                self.focus_kind(BufferKind::Code);
                self.after_jump(from);
                // Planned, not written: the file it's planned in.
                let planned = |c: &arch_graph::Component| c.anchor.is_none() && c.proposed.is_some();
                if self.desk.graph.component(&id).is_some_and(planned) || self.desk.details.get(&id, &self.desk.graph.nav.source).is_some_and(|d| planned(&d.component)) {
                    self.note = Some(format!("{} isn't written yet: this is the file it's planned in.", self.component_name(&id)));
                }
            }
            Some(None) => self.note = Some(format!("{} isn't in the code yet.", self.component_name(&id))),
            None => {
                let source = self.desk.graph.nav.source.clone();
                self.desk.details.want(&id, &source, &self.desk.net, ctx);
                self.desk.pending_goto = Some(Pending { wait: Wait::Code { component: id, line }, from, origin: self.focused_kind(), column: self.landing });
            }
        }
    }

    /// Continues a go-to once what it waits for is in.
    pub(super) fn goto_tick(&mut self, ctx: &egui::Context) {
        self.desk.details.tick();
        // A lookup a go-to waits on failed: say so (others were only prefetches).
        for (id, error) in std::mem::take(&mut self.desk.details.failed) {
            if matches!(&self.desk.pending_goto, Some(Pending { wait: Wait::Relations { component, .. } | Wait::Code { component, .. }, .. }) if *component == id) {
                self.note = Some(format!("Couldn't look up {}: {error}", self.component_name(&id)));
                self.desk.pending_goto = None;
            }
        }
        let at = std::mem::take(&mut *self.desk.at_inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        for reply in at {
            let Some(Pending { wait: Wait::At { op }, from, origin, column }) = self.desk.pending_goto.clone() else { continue };
            self.desk.pending_goto = None;
            self.landing = column;
            match reply {
                Ok(component) => self.goto_component(op, component.id, from, origin, ctx),
                Err(_) => self.note = Some("No component at this line.".into()),
            }
        }
        let source = self.desk.graph.nav.source.clone();
        let ready = match &self.desk.pending_goto {
            Some(Pending { wait: Wait::Relations { component, .. } | Wait::Code { component, .. }, .. }) => self.desk.details.get(component, &source).is_some(),
            _ => false,
        };
        if ready && let Some(Pending { wait, from, origin, column }) = self.desk.pending_goto.take() {
            self.landing = column;
            match wait {
                Wait::Relations { op, component } => self.goto_component(op, component, from, origin, ctx),
                Wait::Code { component, line } => self.open_component_code(component, line, from, ctx),
                Wait::At { .. } => {}
            }
        }
        self.landing = None;
    }
}
