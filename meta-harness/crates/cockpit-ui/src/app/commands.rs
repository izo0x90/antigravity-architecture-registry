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
        actions.push(admin("column.fewer".into(), "One column fewer (its view is hidden, not lost)".into(), S::SetLayout(self.desk.columns.len().saturating_sub(1).max(1))));
        actions.extend(self.column_actions());
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
        actions.extend(self.look_actions());
        actions.push(Action::new("talk.open", "Talk to the agent from here", S::Talk).group(Group::Act).importance(SECONDARY).layer(Layer::Panel));
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

    /// A column's own: closing it (its host places it on its header).
    pub(super) fn column_actions(&self) -> Vec<Action> {
        vec![Action::new("column.close", "Close this column (its view is hidden, not lost)", ShellCommand::CloseColumn).group(Group::View).importance(MINOR)]
    }

    /// The quick look's own: opening and closing it (hold its key to peek),
    /// and sending its card to a column; open, their controls on its header.
    pub(super) fn look_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let look = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::View);
        let Some(open) = &self.quick_look else {
            // On what you're on: its full card.
            return vec![look("quicklook.toggle", "Quick look: the full card (hold to peek)", S::QuickLook).importance(PRIMARY).when(When::Component).layer(Layer::Panel)];
        };
        let mut out = vec![look("quicklook.toggle", "Close the quick look", S::QuickLook).importance(PRIMARY)];
        if open.kind == BufferKind::Spec {
            out.push(look("quicklook.to_column", "Open this card in a column", S::QuickLookToColumn).importance(PRIMARY));
        }
        out
    }

    /// The keys every view shares, labelled for the focused view and marked
    /// with when they apply.
    pub(super) fn nav_actions(&self) -> Vec<Action> {
        use BufferKind as K;
        use ShellCommand as S;
        let kind = self.focused_kind();
        let nav = |id: &str, label: &str, c: ShellCommand| Action::new(id, label, c).group(Group::Navigate).layer(Layer::Panel);
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
            K::Changes => Some(("Next file", "Previous file")),
            K::Queue | K::Chat | K::Inbox => None,
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
            K::Inbox => ("Answer with it, or open its chat", ""),
            K::Changes => ("Open it in the code", "Up to its file"),
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
        // Each go-to's destination says whether it can show what the
        // cursor is on; the relational ones, whether its relations lead
        // anywhere.
        let target = self.desk.view(kind).target();
        for op in GoTo::ALL {
            let when = match op.view() {
                Some(view) if target.as_ref().is_some_and(|t| self.desk.view(view).can_show(t)) => When::Always,
                Some(_) => When::Unavailable,
                None => When::Reaches(op),
            };
            out.push(Action::new(op.id(), op.label(), S::GoTo(op)).group(Group::GoTo).when(when).importance(op.importance()).layer(Layer::Panel));
        }
        if let Some(choosing) = &self.desk.choosing {
            let name = |id: &ComponentId| self.component_name(id);
            let choices = choosing.candidates.iter().map(|c| Choice::new(c.id.as_str(), name(&c.id)).detail(Some(c.id.to_string()))).collect();
            out.push(Action::new("goto.pick", format!("Go to which {}?", choosing.op.one()), S::GotoPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// Where you are: the view with the keyboard, and what holds it (the
    /// quick look, or its column).
    pub(crate) fn here(&self) -> Here {
        match &self.quick_look {
            Some(look) => Here { slot: look.slot, kind: look.kind, look: true },
            None => Here { slot: self.desk.columns.focused(), kind: self.desk.columns.focused_kind(), look: false },
        }
    }

    /// Everything invokable where you are, bound to its keys and in its
    /// layer: the one list key handling, the key bar, the `?` sheet, the
    /// leader menu, the palette and every control read. Gathered once a
    /// frame.
    pub(crate) fn bound_actions(&self) -> Vec<Bound> {
        if self.in_frame.get()
            && let Some(kept) = self.frame_actions.borrow().as_ref()
        {
            return kept.clone();
        }
        let bound = self.gather(self.here(), true);
        if self.in_frame.get() {
            *self.frame_actions.borrow_mut() = Some(bound.clone());
        }
        bound
    }

    /// What a view shows where it is (`here`), the same way: gathered,
    /// placed and in layers; bound to keys only when it's where you are
    /// (`keyed`): a column not focused shows its controls without keys
    /// (they don't run it until it is).
    pub(crate) fn shown_at(&self, here: Here) -> Vec<Bound> {
        if here == self.here() { self.bound_actions() } else { self.gather(here, false) }
    }

    /// The list for `here`: its view's actions (and every other view's
    /// global ones), the shell's (with its host's own: the quick look's or
    /// the column's); each placed (the panel's, its host's and the top
    /// bar's placements); bound to keys; in its layer.
    pub(super) fn gather(&self, here: Here, keyed: bool) -> Vec<Bound> {
        #[cfg(test)]
        if keyed {
            self.gathered.set(self.gathered.get() + 1);
        }
        let kind = here.kind;
        let view = |k: BufferKind, all: bool| self.desk.provider(k).actions().into_iter().filter(move |a| all || a.scope == Scope::Global).map(move |a| (Origin::View(k), a));
        let mut actions: Vec<(Origin, Action)> = view(kind, true).collect();
        actions.extend(self.shell_actions().into_iter().map(|a| (Origin::Shell, a)));
        // Another view's global actions are here for their keys and the
        // palette; their controls are their own panel's, not drawn here.
        for k in BufferKind::ALL.into_iter().filter(|k| *k != kind) {
            actions.extend(view(k, false).map(|(o, mut a)| {
                a.control = None;
                (o, a)
            }));
        }
        // Placed: by its panel, its host, the top bar; each placement says
        // its layer.
        let mut layers: Vec<Option<Layer>> = vec![None; actions.len()];
        for (id, control, layer) in self.placements(here) {
            let found = actions.iter().position(|(o, a)| *o == Origin::View(kind) && a.id == id).or_else(|| actions.iter().position(|(o, a)| *o == Origin::Shell && a.id == id));
            if let Some(i) = found {
                actions[i].1.control = Some(control);
                layers[i] = Some(layer);
            }
        }
        let layer_of = |(origin, action): &(Origin, Action), placed: Option<Layer>| -> Layer {
            let on = action.control.as_ref().is_some_and(|c| matches!(c.at, crate::actions::At::Item | crate::actions::At::Typing));
            match placed {
                Some(_) if on => Layer::On,
                Some(layer) => layer,
                None if on => Layer::On,
                None if *origin == Origin::View(kind) => Layer::Panel,
                None => action.layer.unwrap_or(Layer::Everywhere),
            }
        };
        let tagged: Vec<Layer> = actions.iter().zip(&layers).map(|(a, placed)| layer_of(a, *placed)).collect();
        let keys: Vec<Keys> = if keyed {
            let keyed: KeyedActions = (kind, actions.iter().map(|(origin, a)| (*origin, a.id.clone(), a.aliases.clone())).collect());
            let mut cache = self.bound_cache.borrow_mut();
            if cache.as_ref().is_none_or(|(kept, _)| *kept != keyed) {
                let keys = self.bind_actions(actions.clone(), kind).into_iter().map(|b| (b.leader, b.keys, b.gestures)).collect();
                *cache = Some((keyed, keys));
            }
            cache.as_ref().map(|(_, keys)| keys.clone()).unwrap_or_default()
        } else {
            vec![(None, Vec::new(), Vec::new()); actions.len()]
        };
        actions.into_iter().zip(keys).zip(tagged).map(|(((origin, action), (leader, keys, gestures)), layer)| Bound { action, origin, leader, keys, gestures, layer }).collect()
    }

    /// Every placement for `here`, with its layer: the panel's (on its top
    /// line, or under what you're on), its host's (a column's tabs), the
    /// top bar's (everywhere's).
    fn placements(&self, here: Here) -> Vec<(String, crate::actions::Control, Layer)> {
        use crate::actions::{At, Control, Style};
        use egui_phosphor::regular as icon;
        let panel = self.desk.panel(here.kind).placed().into_iter().map(|(id, c)| (id, c, Layer::Panel));
        let button = |caption: &str, glyph: &'static str| Control::new(At::Header, Style::Button, caption, Some(glyph));
        let host = if here.look {
            vec![("quicklook.to_column".to_owned(), button("Open in a column", icon::COLUMNS)), ("quicklook.toggle".to_owned(), button("", icon::X))]
        } else {
            let mut own = self.tabs(here.slot);
            if self.desk.columns.len() > 1 {
                own.push(("column.close".to_owned(), button("", icon::X)));
            }
            own
        };
        panel.chain(host.into_iter().map(|(id, c)| (id, c, Layer::Host))).chain(self.top_bar_placements().into_iter().map(|(id, c)| (id, c, Layer::Everywhere))).collect()
    }

    /// A column's tabs: the shell's `buffer.*` actions, the shown one named
    /// (when there's room) and lit.
    fn tabs(&self, slot: usize) -> Vec<(String, crate::actions::Control)> {
        use crate::actions::{At, Control, Style, Tone};
        let active = self.desk.columns.kind(slot);
        // Narrow (once its width is known): icons only.
        let compact = self.slot_rects.get(slot).is_some_and(|r| r.is_positive() && r.width() < 340.0);
        BufferKind::ALL
            .into_iter()
            .map(|kind| {
                let title = kind.title();
                let caption = if !compact && active == kind { format!("{}{}", &title[..1], title[1..].to_lowercase()) } else { String::new() };
                let tab = Control::new(At::Header, Style::Button, caption, Some(kind.icon()));
                (format!("buffer.{}", kind.name()), tab.tone(if active == kind { Tone::Accent } else { Tone::Muted }))
            })
            .collect()
    }

    /// The actions bound to their keys, the focused view's own getting the
    /// view menu's free letters.
    pub(super) fn bind_actions(&self, actions: Vec<(Origin, Action)>, focused: BufferKind) -> Vec<Bound> {
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
                out.push(Bound { action, origin: Origin::View(kind), leader: None, keys: Vec::new(), gestures: Vec::new(), layer: Layer::Everywhere });
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
        #[cfg(test)]
        self.ran.borrow_mut().push(invocation.action.id.clone());
        self.run_counted(invocation, count, ctx);
        // What the inbox asked for, done now (its next item, its chat…).
        self.inbox_asks(ctx);
        self.sync_inbox();
        self.in_frame.set(drawing);
        self.frame_actions.borrow_mut().take();
    }

    pub(super) fn run_counted(&mut self, invocation: Invocation, count: Option<u32>, ctx: &egui::Context) {
        let Invocation { action, origin, value } = invocation;
        match origin {
            Origin::Shell => {
                if let Some(command) = action.command::<ShellCommand>() {
                    self.run_shell(*command, value, count.map(|c| c as usize), ctx);
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

    /// Runs an action (a switch's key flips it), or opens the palette at
    /// its parameter if it needs one.
    pub(super) fn run_bound(&mut self, bound: &Bound, count: Option<u32>, ctx: &egui::Context) {
        match &bound.action.param {
            Param::None => self.invoke_counted(Invocation { action: bound.action.clone(), origin: bound.origin, value: None }, count, ctx),
            Param::Toggle { on } => self.invoke(Invocation { action: bound.action.clone(), origin: bound.origin, value: Some((!on).to_string()) }, ctx),
            _ => self.open_palette(Some(bound)),
        }
    }

    pub(super) fn run_shell(&mut self, command: ShellCommand, value: Option<String>, typed: Option<usize>, ctx: &egui::Context) {
        let kind = self.focused_kind();
        let count = typed.unwrap_or(1).max(1);
        match command {
            ShellCommand::AttentionNextWaiting | ShellCommand::AttentionPick | ShellCommand::Alerts => self.run_attention(command, value, ctx),
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
