//! The actions: gathering, binding, invoking, and the shell's own commands.

use super::*;

impl CockpitApp {
    /// The shell's own actions: slots, layout and overlays, and the keys
    /// every view shares, labelled for the focused view.
    pub(crate) fn shell_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let mut actions = Vec::new();
        let admin = |id: String, label: String, c: ShellCommand| Action::new(id, label, c).group(Group::View).importance(MINOR);
        let max = self.config.columns.max;
        for slot in 0..max {
            actions.push(admin(format!("slot.focus.{}", slot + 1), format!("Focus slot {}", slot + 1), S::FocusSlot(slot)));
        }
        actions.push(admin("slot.next".into(), "Next slot".into(), S::CycleSlot(1)));
        actions.push(admin("slot.prev".into(), "Previous slot".into(), S::CycleSlot(-1)));
        for kind in BufferKind::ALL {
            actions.push(admin(format!("buffer.{}", kind.name()), format!("Show {} in this slot", kind.title()), S::ShowBuffer(kind)));
        }
        for cols in 1..=max {
            actions.push(admin(format!("layout.{cols}"), format!("{cols}-column layout"), S::SetLayout(cols)));
        }
        actions.push(admin("column.add".into(), "Add a column".into(), S::SetLayout(self.desk.columns.len() + 1)));
        actions.push(admin("column.close".into(), "Close this column (its view is hidden)".into(), S::CloseColumn));
        // Every open view, to show here: on screen (and where) or hidden.
        let views: Vec<Choice> = self
            .desk.columns
            .panes()
            .iter()
            .enumerate()
            .map(|(slot, p)| (*p, format!("column {}", slot + 1)))
            .chain(self.desk.columns.put_away().map(|p| (p, "hidden".to_owned())))
            .map(|(p, where_)| Choice::new(format!("{}:{}", p.kind.name(), p.id), self.describe(p)).detail(Some(where_)).current(p == self.desk.columns.focused_pane()))
            .collect();
        actions.push(Action::new("buffer.list", "Open views: show one here", S::PickView).param(Param::Choice(views)).group(Group::View).importance(SECONDARY));
        actions.push(admin("buffer.delete".into(), "Delete this column's view (for good)".into(), S::DeleteView));
        for kind in BufferKind::ALL.into_iter().filter(|k| !k.one_only()) {
            actions.push(admin(format!("buffer.new.{}", kind.name()), format!("New {} view here", kind.name()), S::NewView(kind)));
        }
        actions.push(admin("inspector.toggle".into(), "Toggle element inspector".into(), S::ToggleInspector));
        actions.push(admin("issues.toggle".into(), "Toggle issues window".into(), S::ToggleIssues));
        let look = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::View);
        let open = self.quick_look.is_some();
        actions.push(look("quicklook.toggle", if open { "Close the quick look" } else { "Quick look: the full card (hold to peek)" }, S::QuickLook).importance(if open { SECONDARY } else { PRIMARY }).when(if open { When::Always } else { When::Component }));
        if self.quick_look.is_some() {
            actions.push(look("quicklook.to_column", "Open this card in a column", S::QuickLookToColumn).importance(PRIMARY));
        }
        actions.push(Action::new("talk.open", "Talk to the agent from here", S::Talk).group(Group::Act).importance(SECONDARY));
        actions.extend(self.attention_actions());
        actions.extend(self.server_actions());
        actions.push(
            Action::new("cockpit.agents", "Let agents drive the cockpit", S::Agents)
                .detail("Agents read the screen and run actions through the server's cockpit tools, as you would")
                .param(Param::Toggle { on: self.desk.remote.agents })
                .group(Group::Act)
                .importance(MINOR),
        );
        actions.push(admin("palette.open".into(), "Command palette".into(), S::OpenPalette));
        let index = |id: &str, label: &str, op: IndexOp| Action::new(id, label, S::Index(op)).group(Group::Act).importance(MINOR);
        actions.push(index("index.changed", "Re-read the code that changed", IndexOp::Changed));
        actions.push(index("index.rebuild", "Rebuild the index from scratch", IndexOp::Rebuild));
        actions.push(index("index.file", &format!("Re-read {}", self.desk.code.current_file), IndexOp::File));
        actions.extend(self.nav_actions());
        actions
    }

    /// The keys every view shares, labelled for the focused view and marked
    /// with when they apply.
    pub(super) fn nav_actions(&self) -> Vec<Action> {
        use BufferKind as K;
        use ShellCommand as S;
        let kind = self.focused_kind();
        let nav = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::Navigate);
        let mut out = vec![nav("nav.down", "Down", S::Motion(Motion::Down)).importance(SECONDARY), nav("nav.up", "Up", S::Motion(Motion::Up)).importance(SECONDARY)];
        match kind {
            K::Graph => {
                out.push(nav("nav.left", "Left", S::Motion(Motion::Left)).importance(MINOR));
                out.push(nav("nav.right", "Right", S::Motion(Motion::Right)).importance(MINOR));
            }
            K::Tree => {
                out.push(nav("nav.left", "Fold, or up to what holds it", S::Motion(Motion::Left)).importance(SECONDARY));
                out.push(nav("nav.right", "Unfold, or into its first child", S::Motion(Motion::Right)).importance(SECONDARY));
            }
            _ => {}
        }
        out.push(nav("nav.half_down", "Half a page down", S::Motion(Motion::HalfDown)).importance(MINOR));
        out.push(nav("nav.half_up", "Half a page up", S::Motion(Motion::HalfUp)).importance(MINOR));
        out.push(nav("nav.first", "First", S::Motion(Motion::First)).importance(MINOR));
        out.push(nav("nav.last", "Last", S::Motion(Motion::Last)).importance(MINOR));
        let siblings = match kind {
            K::Graph => Some(("Next sibling (same module)", "Previous sibling")),
            K::Tree => Some(("Next sibling row", "Previous sibling row")),
            K::Spec => Some(("Next section", "Previous section")),
            K::Code => Some(("Next definition at this level", "Previous definition at this level")),
            K::Queue | K::Chat => None,
        };
        if let Some((next, prev)) = siblings {
            out.push(nav("nav.next_sibling", next, S::Motion(Motion::NextSibling)).importance(MINOR));
            out.push(nav("nav.prev_sibling", prev, S::Motion(Motion::PrevSibling)).importance(MINOR));
        }
        let (enter, leave) = match kind {
            K::Graph => ("Focus on it", "Back to the whole view"),
            K::Tree => ("Unfold, or select in the graph", "Fold, or up to what holds it"),
            K::Code => ("Go to what's named here", "Out to the definition's start"),
            K::Spec => ("Open: fold a section, follow a relation, reply", "Up to the section title"),
            K::Queue => ("Show why it failed", "Hide that again"),
            K::Chat => ("Open the file it points to", "Follow the latest again"),
        };
        let enter = self.desk.view(kind).enter_label().unwrap_or_else(|| enter.to_owned());
        out.push(nav("nav.enter", &enter, S::Enter).importance(PRIMARY).when(When::Enter));
        let leave = self.desk.view(kind).leave_label().unwrap_or_else(|| leave.to_owned());
        out.push(nav("nav.leave", &leave, S::Leave).importance(SECONDARY).when(When::Leave));
        out.push(nav("nav.hint", "Jump to something on screen", S::Hint { open: false }).importance(PRIMARY));
        out.push(nav("nav.hint_open", "Open something on screen", S::Hint { open: true }).importance(SECONDARY).icon(egui_phosphor::regular::CURSOR_CLICK));
        out.push(nav("nav.search", "Search this view", S::Search).importance(PRIMARY).icon(egui_phosphor::regular::MAGNIFYING_GLASS));
        out.push(nav("nav.search_next", "Next match", S::SearchStep { forward: true }).importance(SECONDARY).when(When::Searched));
        out.push(nav("nav.search_prev", "Previous match", S::SearchStep { forward: false }).importance(MINOR).when(When::Searched));
        if kind != K::Chat {
            let fold = |id: &str, label: &str, f: Fold| Action::new(id, label, S::Fold(f)).group(Group::View).importance(MINOR).when(When::Target);
            out.push(fold("nav.fold_open", "Unfold", Fold::Open));
            out.push(fold("nav.fold_close", "Fold", Fold::Close));
            out.push(fold("nav.fold_toggle", "Fold or unfold", Fold::Toggle).importance(SECONDARY));
            out.push(fold("nav.fold_open_all", "Unfold everything", Fold::OpenAll));
            out.push(fold("nav.fold_close_all", "Fold everything", Fold::CloseAll));
        }
        out.push(nav("nav.mark_set", "Set a mark here", S::MarkSet).importance(MINOR).icon(egui_phosphor::regular::BOOKMARK_SIMPLE));
        out.push(nav("nav.mark_jump", "Jump to a mark", S::MarkJump).importance(MINOR).when(When::Marked));
        out.push(nav("nav.back", "Back (jump list)", S::Back).importance(SECONDARY).when(When::Back).icon(egui_phosphor::regular::ARROW_LEFT));
        out.push(nav("nav.forward", "Forward (jump list)", S::Forward).importance(MINOR).when(When::Forward).icon(egui_phosphor::regular::ARROW_RIGHT));
        out.push(Action::new("nav.keys", "Keys for this view", S::Keys).group(Group::View).importance(MINOR));
        out.push(nav("nav.line", "Go to line or row", S::Line).param(Param::Text { hint: "line or row number".into() }).importance(MINOR));
        let go = |id: &str, label: &str, op: GoTo, when: When, importance: u8| Action::new(id, label, S::GoTo(op)).group(Group::GoTo).when(when).importance(importance);
        out.push(go("goto.code", "Go to its code", GoTo::Code, When::Code, PRIMARY));
        out.push(go("goto.spec", "Go to its spec", GoTo::Spec, When::Component, SECONDARY));
        if kind != K::Graph {
            out.push(go("goto.graph", "Show it in the graph", GoTo::Graph, When::Component, SECONDARY));
        }
        if kind != K::Tree {
            out.push(go("goto.tree", "Show it in the outline", GoTo::Tree, When::Component, MINOR));
        }
        if kind != K::Queue && self.desk.queue.plan.is_some() {
            let has_job = self.desk.view(kind).target().and_then(|t| t.component().cloned()).is_none_or(|id| self.desk.queue.has_job(&id));
            out.push(go("goto.queue", "Go to its queue job", GoTo::Queue, if has_job { When::Component } else { When::Unavailable }, MINOR));
        }
        out.push(go("goto.uses", "Go to a use", GoTo::Uses, When::Reaches(GoTo::Uses), SECONDARY));
        out.push(go("goto.callers", "Go to a caller", GoTo::Callers, When::Reaches(GoTo::Callers), SECONDARY));
        out.push(go("goto.callees", "Go to a callee", GoTo::Callees, When::Reaches(GoTo::Callees), SECONDARY));
        out.push(go("goto.parent", "Go to what holds it", GoTo::Parent, When::Reaches(GoTo::Parent), MINOR));
        out.push(go("goto.members", "Go to a member", GoTo::Members, When::Reaches(GoTo::Members), MINOR));
        out.push(go("goto.implements", "Go to what it implements", GoTo::Implements, When::Reaches(GoTo::Implements), MINOR));
        out.push(go("goto.flows", "Follow its data flow", GoTo::Flows, When::Reaches(GoTo::Flows), MINOR));
        if let Some(choosing) = &self.desk.choosing {
            let name = |id: &ComponentId| self.component_name(id);
            let choices = choosing.candidates.iter().map(|c| Choice::new(c.id.as_str(), name(&c.id)).detail(Some(c.id.to_string()))).collect();
            out.push(Action::new("goto.pick", format!("Go to which {}?", choosing.op.one()), S::GotoPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// Everything invokable right now, bound to its keys: the focused view's
    /// actions first (they win key conflicts), then the shell, then the global
    /// actions of every other view.
    pub(crate) fn bound_actions(&self) -> Vec<Bound> {
        // Gathered once a frame: the key bar, the hints, the palette and the
        // keys all read the same list.
        if self.in_frame.get()
            && let Some(kept) = self.frame_actions.borrow().as_ref()
        {
            return kept.clone();
        }
        let bound = self.gather_actions();
        if self.in_frame.get() {
            *self.frame_actions.borrow_mut() = Some(bound.clone());
        }
        bound
    }

    pub(super) fn gather_actions(&self) -> Vec<Bound> {
        #[cfg(test)]
        self.gathered.set(self.gathered.get() + 1);
        let focused = self.focused_kind();
        let view = |kind: BufferKind, all: bool| self.desk.provider(kind).actions().into_iter().filter(move |a| all || a.scope == Scope::Global).map(move |a| (Origin::View(kind), a));
        // The quick look's notification first: its keys win while it's open.
        let mut actions: Vec<(Origin, Action)> = self.looked_at_actions().into_iter().map(|a| (Origin::Shell, a)).collect();
        actions.extend(view(focused, true));
        actions.extend(self.shell_actions().into_iter().map(|a| (Origin::Shell, a)));
        for kind in BufferKind::ALL.into_iter().filter(|k| *k != focused) {
            actions.extend(view(kind, false));
        }
        let keyed: KeyedActions = (focused, actions.iter().map(|(origin, a)| (*origin, a.id.clone(), a.aliases.clone())).collect());
        let mut cache = self.bound_cache.borrow_mut();
        if cache.as_ref().is_none_or(|(kept, _)| *kept != keyed) {
            let keys = self.bind_actions(actions.clone()).into_iter().map(|b| (b.leader, b.keys)).collect();
            *cache = Some((keyed, keys));
        }
        let keys = cache.as_ref().map(|(_, keys)| keys.clone()).unwrap_or_default();
        actions.into_iter().zip(keys).map(|((origin, action), (leader, keys))| Bound { action, origin, leader, keys }).collect()
    }

    /// The actions bound to their keys, the focused view's own getting the
    /// view menu's free letters.
    pub(super) fn bind_actions(&self, actions: Vec<(Origin, Action)>) -> Vec<Bound> {
        let focused = self.focused_kind();
        let mut bound = self.config.keymap.bind(actions);
        // The focused view's own menu: its actions without a leader path get
        // the next free keys under the view-menu key.
        let menu = self.config.keymap.view_menu;
        let mut taken: Vec<char> = bound.iter().filter_map(|b| b.leader.as_ref().filter(|l| l.len() == 2 && l[0] == menu).map(|l| l[1])).collect();
        for b in bound.iter_mut().filter(|b| b.origin == Origin::View(focused) && b.leader.is_none()) {
            if let Some(key) = crate::keymap::AUTO_KEYS.chars().find(|c| !taken.contains(c)) {
                taken.push(key);
                b.leader = Some(vec![menu, key]);
            }
        }
        bound
    }

    /// The palette's list: everything bound now, plus the other views' own
    /// actions (named for their view; they run there, no keys).
    pub(super) fn palette_actions(&self) -> Vec<Bound> {
        let mut out = self.bound_actions();
        let focused = self.focused_kind();
        for kind in BufferKind::ALL.into_iter().filter(|k| *k != focused) {
            for mut action in self.desk.provider(kind).actions().into_iter().filter(|a| a.scope == Scope::Focused) {
                action.label = format!("{}{} · {}", &kind.title()[..1], kind.title()[1..].to_lowercase(), action.label);
                out.push(Bound { action, origin: Origin::View(kind), leader: None, keys: Vec::new() });
            }
        }
        out
    }

    pub fn invoke(&mut self, invocation: Invocation, ctx: &egui::Context) {
        self.invoke_counted(invocation, None, ctx);
    }

    /// Runs an action with the count typed before its key, if one was: moves
    /// take it, a go-to lands in that column, a view's own actions repeat,
    /// anything else runs once.
    pub(super) fn invoke_counted(&mut self, invocation: Invocation, count: Option<u32>, ctx: &egui::Context) {
        // An action changes what can run as it goes: it sees the actions as
        // they are, and what's drawn after it sees what it left.
        let drawing = self.in_frame.replace(false);
        self.run_counted(invocation, count, ctx);
        self.in_frame.set(drawing);
        self.frame_actions.borrow_mut().take();
    }

    pub(super) fn run_counted(&mut self, invocation: Invocation, count: Option<u32>, ctx: &egui::Context) {
        let Invocation { action, origin, value } = invocation;
        match origin {
            Origin::Shell => {
                if let Some(command) = action.command::<ShellCommand>() {
                    self.run_shell(*command, value, count.map(|c| c as usize), ctx);
                } else if let Some(answer) = action.command::<attention::LookAnswer>() {
                    self.answer_looked_at(&answer.0.clone(), ctx);
                }
            }
            Origin::View(kind) => {
                let repeats = if kind == self.focused_kind() && action.param == Param::None { count.unwrap_or(1).max(1) } else { 1 };
                let before = action.jump.then(|| self.jump_here());
                for _ in 0..repeats {
                    self.run_on(kind, &action, value.clone(), ctx);
                }
                if let Some(before) = before {
                    self.after_jump(before);
                }
            }
        }
    }

    /// Runs an action by id the way its key would (chips and toolbars).
    pub(super) fn run_by_id(&mut self, id: &str, value: Option<String>, ctx: &egui::Context) {
        let bound = self.palette_actions();
        if let Some(b) = bound.into_iter().find(|b| b.action.id == id) {
            if value.is_none() && b.action.param != Param::None {
                self.open_palette(Some(&b));
            } else {
                self.invoke(Invocation { action: b.action, origin: b.origin, value }, ctx);
            }
        }
    }

    /// Opens the palette (at an action's parameter, if given) and gives it the keyboard.
    pub(super) fn open_palette(&mut self, action: Option<&Bound>) {
        match action {
            Some(action) => self.palette.open_for(action),
            None => self.palette.open(),
        }
        self.input_mode = InputMode::Palette;
    }

    /// Runs an action, or opens the palette at its parameter if it needs one.
    pub(super) fn run_bound(&mut self, bound: &Bound, count: Option<u32>, ctx: &egui::Context) {
        if bound.action.param == Param::None {
            self.invoke_counted(Invocation { action: bound.action.clone(), origin: bound.origin, value: None }, count, ctx);
        } else {
            self.open_palette(Some(bound));
        }
    }

    pub(super) fn run_shell(&mut self, command: ShellCommand, value: Option<String>, typed: Option<usize>, ctx: &egui::Context) {
        let kind = self.focused_kind();
        let count = typed.unwrap_or(1).max(1);
        match command {
            ShellCommand::AttentionNextWaiting
            | ShellCommand::AttentionPick
            | ShellCommand::AttentionStep(_)
            | ShellCommand::AttentionOpenSlot
            | ShellCommand::AttentionDismiss
            | ShellCommand::BrowserAlerts => self.run_attention(command, value, ctx),
            ShellCommand::ServerSwitch
            | ShellCommand::ServerAdd
            | ShellCommand::ServerConnect
            | ShellCommand::ServerDisconnect
            | ShellCommand::ServerRemove
            | ShellCommand::ServerShutdown
            | ShellCommand::ServerShutdownConfirm => self.run_server(command, value, ctx),
            ShellCommand::FocusSlot(slot) => {
                self.desk.columns.focus(slot);
            }
            ShellCommand::CycleSlot(dir) => self.desk.columns.cycle(dir),
            ShellCommand::ShowBuffer(kind) => {
                let pane = self.desk.columns.show(self.desk.columns.focused(), kind);
                self.use_pane(pane);
            }
            ShellCommand::SetLayout(cols) if cols > self.config.columns.max => self.note = Some(format!("{} columns at most (`[columns] max` in your UI config).", self.config.columns.max)),
            ShellCommand::SetLayout(cols) => self.desk.columns.set_count(cols, self.config.columns.max),
            ShellCommand::CloseColumn => self.close_column(self.desk.columns.focused()),
            ShellCommand::DeleteView => self.delete_view(),
            ShellCommand::NewView(kind) => self.new_view(kind),
            ShellCommand::PickView => {
                let picked = value.as_deref().and_then(|v| {
                    let (kind, id) = v.split_once(':')?;
                    let kind = BufferKind::ALL.into_iter().find(|k| k.name() == kind)?;
                    Some(Pane::new(kind, id.parse().ok()?))
                });
                match picked.filter(|p| self.desk.columns.has(*p)) {
                    Some(pane) => self.show_view(pane),
                    None => self.note = Some("That view isn't open any more.".into()),
                }
            }
            ShellCommand::ToggleInspector => self.feedback.toggle_inspection(),
            ShellCommand::ToggleIssues => {
                self.feedback.is_issues_window_open = !self.feedback.is_issues_window_open;
                if self.feedback.is_issues_window_open {
                    self.feedback.load_issues(&self.desk.net, ctx.clone());
                }
            }
            ShellCommand::QuickLook => self.toggle_quick_look(ctx),
            ShellCommand::QuickLookToColumn => self.quick_look_to_column(),
            ShellCommand::Talk => {
                self.talk.get_or_insert_with(Default::default);
                self.input_mode = InputMode::Insert { slot: self.desk.columns.focused() };
            }
            ShellCommand::Agents => {
                self.desk.remote.agents = value.as_deref() == Some("true");
                self.note = Some(if self.desk.remote.agents { "Agents can drive the cockpit (the server's cockpit tools)." } else { "Agents can't drive the cockpit." }.into());
            }
            ShellCommand::OpenPalette => self.open_palette(None),
            ShellCommand::Motion(motion) => {
                let before = self.jump_here();
                self.desk.view_mut(kind).motion(motion, count);
                // Big moves are jumps; small ones aren't.
                if matches!(motion, Motion::First | Motion::Last | Motion::Line(_)) {
                    self.after_jump(before);
                }
            }
            ShellCommand::Line => match value.as_deref().map(str::trim).and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => self.run_shell(ShellCommand::Motion(Motion::Line(n)), None, None, ctx),
                None => self.note = Some("Give a line number.".into()),
            },
            ShellCommand::Enter | ShellCommand::Leave => {
                let before = self.jump_here();
                let step = if command == ShellCommand::Enter { self.desk.view_mut(kind).enter() } else { self.desk.view_mut(kind).leave() };
                self.take_step(step, before, ctx);
            }
            ShellCommand::Fold(fold) => self.desk.view_mut(kind).fold(fold),
            ShellCommand::Hint { open } => self.start_hint(open),
            ShellCommand::Search => {
                self.capture = Some(Capture::Search(Searching { kind, query: String::new(), before: self.jump_here() }));
                self.input_mode = InputMode::Capture { slot: self.desk.columns.focused() };
                self.desk.view_mut(kind).save_view();
                // An empty search is a search too (a filter typed into goes blank).
                self.desk.view_mut(kind).on_search(SearchEvent::Typing(""));
            }
            ShellCommand::SearchStep { forward } => self.search_step(forward, count),
            ShellCommand::MarkSet => self.start_capture(Capture::MarkSet),
            ShellCommand::MarkJump => self.start_capture(Capture::MarkJump),
            ShellCommand::Back | ShellCommand::Forward => {
                let here = self.jump_here();
                let mut to = None;
                for _ in 0..count {
                    let step = if command == ShellCommand::Back { self.desk.jumps.back(to.clone().unwrap_or_else(|| here.clone())) } else { self.desk.jumps.forward(to.clone().unwrap_or_else(|| here.clone())) };
                    match step {
                        Some(j) => to = Some(j),
                        None => break,
                    }
                }
                match to {
                    Some(jump) => self.restore_jump(jump, ctx),
                    None => self.note = Some(if command == ShellCommand::Back { "Nothing to go back to." } else { "Nothing to go forward to." }.into()),
                }
            }
            ShellCommand::Keys => self.key_sheet = Some(String::new()),
            ShellCommand::GoTo(op) => {
                if let Some(column) = typed
                    && !self.land_in(column)
                {
                    return;
                }
                self.goto(op, ctx);
                self.landing = None;
            }
            ShellCommand::GotoPick => self.goto_pick(value, ctx),
            ShellCommand::Index(op) => self.run_index(op, ctx),
        }
    }

    pub(super) fn start_capture(&mut self, capture: Capture) {
        self.capture = Some(capture);
        self.input_mode = InputMode::Capture { slot: self.desk.columns.focused() };
        self.pending_since = 0.0;
    }

    /// What Enter or Backspace (or `F`) came to.
    pub(super) fn take_step(&mut self, step: Step, before: Jump, ctx: &egui::Context) {
        match step {
            Step::Done => self.after_jump(before),
            Step::GoTo(op) => {
                self.after_jump(before);
                self.goto(op, ctx);
            }
            Step::Nothing(why) => self.note = Some(why),
        }
    }
}
