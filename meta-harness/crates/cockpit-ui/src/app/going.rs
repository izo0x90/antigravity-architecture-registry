//! Go-tos: from what the cursor is on to where it leads.

use super::*;

impl CockpitApp {
    pub(super) fn component_name(&self, id: &ComponentId) -> String {
        self.desk.graph.component(id).map(|c| c.name.clone()).unwrap_or_else(|| crate::nav::short_name(id))
    }

    /// A go-to's count names the panel it lands in (1 is the leftmost; one
    /// past the last adds a panel). Not the panel it starts from: false,
    /// and the user is told why.
    pub(super) fn land_in(&mut self, number: usize) -> bool {
        let count = self.desk.panels.len();
        let Some(panel) = number.checked_sub(1) else { return true };
        if panel == self.desk.panels.focused() {
            self.note = Some(format!("Panel {number} is this one: give another panel's number (1 to {count}, or {} for a new one).", count + 1));
            return false;
        }
        if panel > count || panel >= self.config.panels.max {
            self.note = Some(format!("There are {count} panels: give 1 to {count}, or {} for a new one.", count + 1));
            return false;
        }
        if panel == count {
            self.desk.panels.set_count(count + 1, self.config.panels.max);
        }
        self.landing = Some(panel);
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
            if matches!(target, Target::Message { .. } | Target::Change { .. }) && target.code().is_some() {
                return self.show_target_in(BufferKind::Code, &target, from);
            }
        }
        // Not a component, nor a place the index can say one is at (a
        // message's turn): its destination shows it as it is, if it can.
        if target.component().is_none()
            && !target.in_code()
            && let Some(view) = op.view().filter(|v| self.desk.view(*v).can_show(&target))
        {
            return self.show_target_in(view, &target, from);
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
                self.desk.pending_goto = Some(Pending { wait: Wait::At { op }, from, origin, panel: self.landing });
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
                        self.desk.pending_goto = Some(Pending { wait: Wait::Relations { op, component: id }, from, origin, panel: self.landing });
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
                self.desk.choosing = Some(Choosing { op, from, origin, candidates: found, panel: self.landing });
                if let Some(pick) = self.bound_actions().into_iter().find(|b| b.action.id == "goto.pick") {
                    self.open_palette(Some(&pick));
                }
            }
        }
    }

    pub(super) fn goto_pick(&mut self, value: Option<String>, ctx: &egui::Context) {
        let (Some(choosing), Some(value)) = (self.desk.choosing.take(), value) else { return };
        if let Some(c) = choosing.candidates.iter().find(|c| c.id.as_str() == value) {
            self.landing = choosing.panel;
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
        // Where it started, when that view can show it (in the panel a
        // count named, a view of that kind there); else the graph.
        let target = Target::Component(to.id);
        let kind = if self.desk.view(origin).can_show(&target) { origin } else { BufferKind::Graph };
        self.show_target_in(kind, &target, from);
    }

    /// Shows a component in a view and moves focus there.
    pub(super) fn show_in(&mut self, kind: BufferKind, id: ComponentId, from: Jump) {
        self.show_target_in(kind, &Target::Component(id), from);
    }

    /// Where a buffer's `Ask::Show` lands: `target` in a buffer of `kind`,
    /// as a go-to lands it (a component's code found first).
    pub(super) fn land(&mut self, kind: BufferKind, target: &Target, from: Jump, ctx: &egui::Context) {
        match (kind, target) {
            (BufferKind::Code, Target::Component(id)) => self.open_component_code(id.clone(), None, from, ctx),
            _ => self.show_target_in(kind, target, from),
        }
    }

    /// Shows `target` in a view of `kind` (as it says) and moves focus
    /// there; when it can't, says why and nothing moves. It lands in a
    /// panel: the quick look (over one) closes.
    pub(super) fn show_target_in(&mut self, kind: BufferKind, target: &Target, from: Jump) {
        self.close_quick_look();
        let used = self.desk.used_buffer(kind);
        if let Err(why) = self.desk.view_mut(kind).show_target(target) {
            self.note = Some(why);
            return;
        }
        // The view it lands in (a panel's own of that kind), shown there
        // too when it's another.
        self.focus_kind(kind);
        if self.desk.used_buffer(kind) != used {
            let _ = self.desk.view_mut(kind).show_target(target);
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
                self.show_target_in(BufferKind::Code, &Target::Span { file, start: start as usize, end: end as usize }, from);
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
                self.desk.pending_goto = Some(Pending { wait: Wait::Code { component: id, line }, from, origin: self.focused_kind(), panel: self.landing });
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
            let Some(Pending { wait: Wait::At { op }, from, origin, panel }) = self.desk.pending_goto.clone() else { continue };
            self.desk.pending_goto = None;
            self.landing = panel;
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
        if ready && let Some(Pending { wait, from, origin, panel }) = self.desk.pending_goto.take() {
            self.landing = panel;
            match wait {
                Wait::Relations { op, component } => self.goto_component(op, component, from, origin, ctx),
                Wait::Code { component, line } => self.open_component_code(component, line, from, ctx),
                Wait::At { .. } => {}
            }
        }
        self.landing = None;
    }
}
