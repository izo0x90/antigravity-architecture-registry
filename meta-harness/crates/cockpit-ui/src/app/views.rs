//! Dispatch to the views, jumps between them, and what they show in common.

use super::*;

impl CockpitApp {
    /// Runs a view's action with what it may touch (disjoint fields).
    pub(super) fn run_on(&mut self, kind: BufferKind, action: &Action, value: Option<String>, ctx: &egui::Context) {
        let panel = self.panel_of(kind);
        let provider: &mut dyn ActionProvider = match kind {
            BufferKind::Chat => &mut self.desk.chat,
            BufferKind::Code => &mut self.desk.code,
            BufferKind::Graph => &mut self.desk.graph,
            BufferKind::Tree => &mut self.desk.tree,
            BufferKind::Spec => &mut self.desk.spec,
            BufferKind::Queue => &mut self.desk.queue,
            BufferKind::Inbox => &mut self.desk.inbox,
            BufferKind::Changes => &mut self.desk.changes,
        };
        provider.run(action, value, &mut RunCx { net: &self.desk.net, ctx, input_mode: &mut self.input_mode, panel, note: &mut self.note, asked: &mut self.asked });
    }

    /// The place `kind` is at, in the panel showing it.
    pub(super) fn jump_of(&self, kind: BufferKind) -> Jump {
        Jump { panel: self.panel_of(kind), kind, place: self.desk.view(kind).place() }
    }

    /// Where the cursor is now: the focused panel, its view, its place.
    pub(super) fn jump_here(&self) -> Jump {
        self.jump_of(self.focused_kind())
    }

    /// Records `from` in the jump list if the cursor has left it.
    pub(super) fn after_jump(&mut self, from: Jump) {
        if self.jump_here() != from {
            self.desk.jumps.record(from);
        }
    }

    /// Goes to a recorded place: its view in its panel, focused.
    pub(super) fn restore_jump(&mut self, jump: Jump) {
        let panel = if jump.panel < self.desk.panels.len() { jump.panel } else { self.ensure_visible(jump.kind) };
        let buffer = self.desk.panels.show(panel, jump.kind);
        self.desk.panels.focus(panel);
        self.use_buffer(buffer);
        self.desk.view_mut(jump.kind).restore(&jump.place);
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
            reaches: self.reaches(),
        }
    }

    /// The relational go-tos that lead somewhere from the focused target, if
    /// its relations are here.
    pub(super) fn reaches(&self) -> Option<Vec<GoTo>> {
        let target = self.desk.view(self.focused_kind()).target()?;
        let detail = self.desk.details.get(target.component()?, self.desk.shared.source.get())?;
        let ops = [GoTo::Uses, GoTo::Callers, GoTo::Callees, GoTo::Parent, GoTo::Members, GoTo::Implements, GoTo::Flows];
        Some(ops.into_iter().filter(|op| !candidates(*op, detail).is_empty()).collect())
    }

    /// The queue view runs with the chat's harness.
    pub(super) fn link_queue(&mut self, ctx: &egui::Context) {
        self.desk.queue.instance = self.desk.chat.active_instance();
        self.desk.queue.tick(&self.desk.net, ctx);
    }

    /// What the views show in common: the desk's comparison and what it
    /// changed, the source, the review, the queue's plan.
    pub(super) fn link_views(&mut self, ctx: &egui::Context) {
        self.desk.graph.tick(&self.desk.net, ctx);
        // The desk's comparison: read whether or not its view is shown (the
        // code view and the graph show it too); asked again now and then
        // while it is.
        let shown = self.desk.panels.kinds().contains(&BufferKind::Changes) || self.quick_look.as_ref().is_some_and(|q| q.kind == BufferKind::Changes);
        self.desk.shared.changes.tick(&self.desk.net, ctx, shown, self.config.changes.refresh_s);
        let plan = self.desk.shared.source.get().plan().cloned();
        self.desk.shared.review.tick(&self.desk.net, ctx, plan.as_ref());
        // One truth for where code is: if the spec fetched a newer anchor for
        // the selection than the view on screen has, the view is stale.
        if let (Some(detail), Some(shown)) = (&self.desk.spec.detail, self.desk.graph.selected())
            && detail.component.id == shown.id
            && detail.component.anchor != shown.anchor
            && self.desk.anchor_checked.replace(detail.component.anchor.clone()) != Some(detail.component.anchor.clone())
        {
            self.desk.graph.invalidate();
        }
        self.link_queue(ctx);
    }
}
