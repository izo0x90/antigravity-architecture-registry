//! Dispatch to the views, jumps between them, and what they show in common.

use super::*;

impl CockpitApp {
    /// Runs a view's action with what it may touch (disjoint fields).
    pub(super) fn run_on(&mut self, kind: BufferKind, action: &Action, value: Option<String>, ctx: &egui::Context) {
        let at = self.host_of(kind);
        let (buffer, net) = self.desk.view_with_net(kind);
        buffer.run(action, value, &mut RunCx { net, ctx, input_mode: &mut self.input_mode, at, note: &mut self.note, asked: &mut self.asked });
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
    pub fn situation(&self) -> std::rc::Rc<Situation> {
        self.situation_of(self.focused_kind())
    }

    /// The same, for the view of `kind` (where it is, focused or not):
    /// worked out once a frame.
    pub fn situation_of(&self, kind: BufferKind) -> std::rc::Rc<Situation> {
        if let Some((_, kept)) = self.frame_situations.borrow().iter().find(|(k, _)| *k == kind) {
            return kept.clone();
        }
        let situation = std::rc::Rc::new(self.situation_now(kind));
        if self.in_frame.get() {
            self.frame_situations.borrow_mut().push((kind, situation.clone()));
        }
        situation
    }

    /// Whether `bound` applies where it's offered (`here`): judged in its
    /// own view (another view's, there; the shell's, where it's offered).
    pub fn applies(&self, bound: &Bound, here: Here) -> bool {
        let kind = match bound.origin {
            Origin::View(kind) => kind,
            Origin::Shell => here.kind,
        };
        bound.action.when.holds(&self.situation_of(kind))
    }

    fn situation_now(&self, kind: BufferKind) -> Situation {
        Situation {
            target: self.desk.view(kind).target(),
            searched: self.desk.searches.get(&kind).is_some_and(|q| !q.is_empty()),
            can_back: self.desk.jumps.can_back(),
            can_forward: self.desk.jumps.can_forward(),
            marked: !self.desk.marks.is_empty(),
            reaches: self.reaches_of(kind),
        }
    }

    /// The relational go-tos that lead somewhere from the view's target,
    /// if its relations are here.
    pub(super) fn reaches_of(&self, kind: BufferKind) -> Option<Vec<GoTo>> {
        let target = self.desk.view(kind).target()?;
        let detail = self.desk.details.get(target.component()?, self.desk.shared.source.get())?;
        let ops = [GoTo::Uses, GoTo::Callers, GoTo::Callees, GoTo::Parent, GoTo::Members, GoTo::Implements, GoTo::Flows];
        Some(ops.into_iter().filter(|op| crate::goto::reaches(*op, detail)).collect())
    }

    /// What the buffers show in common, read where it's kept: the desk's
    /// comparison and the plan's review (each buffer reads them itself).
    pub(super) fn link_views(&mut self, ctx: &egui::Context) {
        // The desk's comparison: read whether or not the changes are shown
        // (the code view and the graph show it too); asked again now and
        // then while they are.
        let shown = self.desk.panels.kinds().contains(&BufferKind::Changes) || self.quick_look.as_ref().is_some_and(|q| q.kind() == BufferKind::Changes);
        self.desk.shared.changes.tick(&self.desk.net, ctx, shown, self.config.changes.refresh_s);
        let plan = self.desk.shared.source.get().plan().cloned();
        self.desk.shared.review.tick(&self.desk.net, ctx, plan.as_ref(), &self.desk.shared.discussions.get());
    }
}
