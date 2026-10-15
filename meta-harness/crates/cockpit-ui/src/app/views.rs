//! Dispatch to the views, jumps between them, and linking what they show.

use super::*;

impl CockpitApp {
    /// Runs a view's action with what it may touch (disjoint fields).
    pub(super) fn run_on(&mut self, kind: BufferKind, action: &Action, value: Option<String>, ctx: &egui::Context) {
        let slot = self.slot_of(kind);
        let shell = self.shell_actions();
        let provider: &mut dyn ActionProvider = match kind {
            BufferKind::Chat => &mut self.desk.chat,
            BufferKind::Code => &mut self.desk.code,
            BufferKind::Graph => &mut self.desk.graph,
            BufferKind::Tree => &mut self.desk.tree,
            BufferKind::Spec => &mut self.desk.spec,
            BufferKind::Queue => &mut self.desk.queue,
        };
        provider.run(action, value, &mut RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, slot, note: &mut self.note, shell: &shell });
    }

    /// The place `kind` is at, in the slot showing it.
    pub(super) fn jump_of(&self, kind: BufferKind) -> Jump {
        Jump { slot: self.slot_of(kind), kind, place: self.desk.view(kind).place() }
    }

    /// Where the cursor is now: the focused slot, its view, its place.
    pub(super) fn jump_here(&self) -> Jump {
        self.jump_of(self.focused_kind())
    }

    /// Records `from` in the jump list if the cursor has left it.
    pub(super) fn after_jump(&mut self, from: Jump) {
        if self.jump_here() != from {
            self.desk.jumps.record(from);
        }
    }

    /// Goes to a recorded place: its view in its slot, focused.
    pub(super) fn restore_jump(&mut self, jump: Jump, ctx: &egui::Context) {
        let slot = if jump.slot < self.desk.columns.len() { jump.slot } else { self.ensure_visible(jump.kind) };
        let pane = self.desk.columns.show(slot, jump.kind);
        self.desk.columns.focus(slot);
        self.use_pane(pane);
        self.restore_place(jump.kind, &jump.place, ctx);
    }

    /// Puts view `kind` back at `place` (fetching what it needs).
    pub(super) fn restore_place(&mut self, kind: BufferKind, place: &Place, ctx: &egui::Context) {
        match place {
            Place::Code { file, line } if !self.desk.code.shows(file) => self.desk.code.load_file(file, Some(*line), &self.desk.net, ctx.clone()),
            Place::Spec { component: Some(id), .. } if !self.desk.spec.pinned && self.desk.graph.selection.as_deref() != Some(id.as_str()) => {
                // The spec shows the graph's selection: move that first.
                self.desk.graph.reveal(id.clone());
                self.desk.spec.restore(place);
            }
            place => self.desk.view_mut(kind).restore(place),
        }
    }

    /// What `When` conditions are judged against, for the focused view.
    pub fn situation(&self) -> Situation {
        let kind = self.focused_kind();
        Situation {
            target: self.desk.view(kind).target(),
            searched: self.desk.searches.get(&kind).is_some_and(|q| !q.is_empty()),
            can_back: self.desk.jumps.can_back(),
            can_forward: self.desk.jumps.can_forward(),
            marked: !self.desk.marks.is_empty(),
            can_enter: self.desk.view(kind).can_enter(),
            can_leave: self.desk.view(kind).can_leave(),
            reaches: self.reaches(),
        }
    }

    /// The relational go-tos that lead somewhere from the focused target, if
    /// its relations are here.
    pub(super) fn reaches(&self) -> Option<Vec<GoTo>> {
        let target = self.desk.view(self.focused_kind()).target()?;
        let detail = self.desk.details.get(target.component()?, &self.desk.graph.nav.source)?;
        let ops = [GoTo::Uses, GoTo::Callers, GoTo::Callees, GoTo::Parent, GoTo::Members, GoTo::Implements, GoTo::Flows];
        Some(ops.into_iter().filter(|op| !candidates(*op, detail).is_empty()).collect())
    }

    /// The queue view runs on the graph's plan with the chat's harness, and
    /// sends the user to its workers' sessions.
    pub(super) fn link_queue(&mut self, plan: Option<&arch_graph::PlanId>, ctx: &egui::Context) {
        self.desk.queue.author = self.config.user.name.clone();
        self.desk.queue.instance = self.desk.chat.active_instance();
        self.desk.queue.tick(&self.desk.net, ctx, plan);
        if let Some(thread) = self.desk.queue.requests.open_thread.take() {
            let from = self.jump_of(BufferKind::Queue);
            self.focus_kind(BufferKind::Chat);
            if self.desk.chat.show_thread(&harness_contracts::ThreadId::new(&thread)) {
                self.after_jump(from);
            } else {
                self.note = Some(format!("No slot runs session {thread} now."));
            }
        }
    }

    /// The plan's review state feeds the graph's badges and the spec's threads.
    pub(super) fn link_review(&mut self, plan: Option<&arch_graph::PlanId>, ctx: &egui::Context) {
        if std::mem::take(&mut self.desk.spec.requests.review_changed) {
            self.desk.review.invalidate();
        }
        self.desk.review.tick(&self.desk.net, ctx, plan);
        if self.desk.review.generation != self.desk.review_seen {
            self.desk.review_seen = self.desk.review.generation;
            let marks = self.desk.review.marks();
            for graph in self.desk.graphs() {
                graph.marks.clone_from(&marks);
            }
            let (comments, diff) = (self.desk.review.comments.clone(), self.desk.review.diff.clone());
            for spec in self.desk.specs() {
                spec.comments.clone_from(&comments);
                spec.diff.clone_from(&diff);
            }
        }
        let author = self.config.user.name.clone();
        for spec in self.desk.specs() {
            if spec.author != author {
                spec.author.clone_from(&author);
            }
        }
    }

    /// Carries requests between views: graph ↔ code ↔ spec ↔ outline ↔ chat.
    /// Each one that sends the user somewhere is a jump.
    pub(super) fn link_views(&mut self, ctx: &egui::Context) {
        self.desk.graph.tick(&self.desk.net, ctx);
        let source = self.desk.graph.nav.source.clone();
        let plan = source.plan().cloned();
        // Specs not pinned show what the graph used last has selected.
        let selected = self.desk.graph.current();
        for spec in self.desk.specs() {
            spec.follow(selected.as_ref(), &source);
        }
        // One truth for where code is: if the spec fetched a newer anchor for
        // the selection than the view on screen has, the view is stale.
        if let (Some(detail), Some(shown)) = (&self.desk.spec.detail, self.desk.graph.selected())
            && detail.component.id == shown.id
            && detail.component.anchor != shown.anchor
            && self.desk.anchor_checked.replace(detail.component.anchor.clone()) != Some(detail.component.anchor.clone())
        {
            self.desk.graph.invalidate();
        }
        self.link_review(plan.as_ref(), ctx);
        self.link_queue(plan.as_ref(), ctx);
        self.desk.tree.set_source(&source);
        // Jumps made in the graph with the pointer or a finger.
        if let Some(place) = self.desk.graph.jumped.take() {
            self.desk.jumps.record(Jump { slot: self.slot_of(BufferKind::Graph), kind: BufferKind::Graph, place });
        }
        if let Some((id, value)) = self.desk.graph.requests.run.take() {
            self.run_by_id(&id, value, ctx);
        }
        if let Some((file, start, end)) = self.desk.graph.requests.open_code.take() {
            self.desk.jumps.record(self.jump_of(BufferKind::Graph));
            self.show_code(&file, start, end, ctx);
        }
        if std::mem::take(&mut self.desk.graph.requests.open_spec) {
            self.ensure_visible(BufferKind::Spec);
        }
        if let Some((file, start, end)) = self.desk.spec.requests.open_code.take() {
            self.desk.jumps.record(self.jump_of(BufferKind::Spec));
            self.show_code(&file, start, end, ctx);
        }
        if std::mem::take(&mut self.desk.code.reread) {
            self.run_index(IndexOp::File, ctx);
        }
        if let Some(file) = self.desk.spec.requests.reread.take() {
            let notes = self.desk.index_inbox.clone();
            self.desk.net.arch_refresh(&[file], ctx.clone(), move |r| {
                if let Err(e) = r
                    && let Ok(mut i) = notes.lock()
                {
                    i.push(Err(e));
                }
            });
        }
        if std::mem::take(&mut self.desk.graph.requests.reindex) {
            self.run_index(IndexOp::Changed, ctx);
        }
        if std::mem::take(&mut self.desk.spec.requests.insert) {
            self.focus_kind(BufferKind::Spec);
            self.input_mode = InputMode::Insert { slot: self.desk.columns.focused() };
        }
        // A name ctrl-clicked in the code: its definition.
        if let Some(id) = self.desk.code.open.take() {
            let from = self.jump_of(BufferKind::Code);
            self.goto_component(GoTo::Code, id, from, BufferKind::Code, ctx);
        }
        for (kind, id) in [(BufferKind::Spec, self.desk.spec.requests.select.take()), (BufferKind::Code, self.desk.code.select.take())] {
            if let Some(id) = id {
                // In the quick look, a link followed shows its card there too.
                if kind == BufferKind::Spec && self.quick_look.is_some() {
                    self.desk.spec.show(id.clone());
                }
                self.desk.jumps.record(self.jump_of(kind));
                self.ensure_visible(BufferKind::Graph);
                self.desk.graph.reveal(id);
            }
        }
        // The outline's Enter: the graph follows (focus stays in the outline).
        if let Some(id) = self.desk.tree.select.take() {
            self.ensure_visible(BufferKind::Graph);
            self.desk.graph.reveal(id);
        }
        if let Some((path, line)) = self.desk.chat.ui.requested_file_jump.take() {
            self.desk.jumps.record(self.jump_of(BufferKind::Chat));
            self.ensure_visible(BufferKind::Code);
            self.desk.code.load_file(&path, line, &self.desk.net, ctx.clone());
        }
    }
}
