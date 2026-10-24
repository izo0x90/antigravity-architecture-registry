//! The actions: gathering, binding, invoking, and the shell's own commands.

use super::*;
use crate::keymap::Host;
use crate::hosted::ItemButtons;

impl CockpitApp {
    /// The outermost level, everywhere: the shell's own (panels and
    /// layouts, the inspector, what needs you, servers, the palette, the
    /// index).
    pub(crate) fn shell_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let mut actions = Vec::new();
        let admin = |id: String, label: String, detail: String, c: ShellCommand| Action::new(id, label, detail, c).group(Group::View).importance(MINOR);
        let max = self.config.panels.max;
        for panel in 0..max {
            actions.push(admin(format!("panel.focus.{}", panel + 1), format!("Focus panel {}", panel + 1), format!("The keys go to the buffer in panel {} (counted from the left)", panel + 1), S::FocusPanel(panel)));
        }
        actions.push(admin("panel.next".into(), "Next panel".into(), "The keys go to the panel to the right (from the last, the first)".into(), S::CyclePanel(1)));
        actions.push(admin("panel.prev".into(), "Previous panel".into(), "The keys go to the panel to the left (from the first, the last)".into(), S::CyclePanel(-1)));
        for cols in 1..=max {
            actions.push(admin(format!("layout.{cols}"), format!("{cols}-panel layout"), format!("{cols} panels side by side: buffers that don't fit are hidden, not lost"), S::SetLayout(cols)));
        }
        actions.push(admin("panel.add".into(), "Add a panel".into(), "One more panel, on the right: a hidden buffer comes back in it".into(), S::SetLayout(self.desk.panels.len() + 1)));
        actions.push(admin("panel.fewer".into(), "One panel fewer (its buffer is hidden, not lost)".into(), "The rightmost panel goes; its buffer stays open, hidden".into(), S::SetLayout(self.desk.panels.len().saturating_sub(1).max(1))));
        let bar = self.item_buttons_now() == ItemButtons::Bar;
        actions.push(Action::new("panel.item_buttons", "Show the buttons of what you're on in a bar along each panel's bottom (else floating under it)", "Where the buttons for the item under the cursor show, in every panel", S::ItemButtons).param(Param::Toggle { on: bar }).group(Group::View).importance(MINOR));
        actions.push(admin("inspector.toggle".into(), "Toggle element inspector".into(), "For reporting the cockpit itself: point at anything drawn to file feedback on it".into(), S::ToggleInspector));
        actions.push(admin("issues.toggle".into(), "Toggle issues window".into(), "The feedback filed on the cockpit, and what came of it".into(), S::ToggleIssues));
        actions.extend(self.attention_actions());
        actions.extend(self.server_actions());
        actions.push(
            Action::new("cockpit.agents", "Let agents drive the cockpit", "Agents read the screen and run actions through the server's cockpit tools, as you would", S::Agents)
                .param(Param::Toggle { on: self.desk.remote.agents })
                .group(Group::Act)
                .importance(MINOR),
        );
        actions.push(admin("palette.open".into(), "Action list".into(), "What can be done here, found by typing".into(), S::OpenPalette));
        // A menu of the leader's (each group's button opens its own).
        let kind = self.focused_kind();
        let choices = self.menu_keys().into_iter().map(|key| Choice::new(key.to_string(), self.group_name(&[key], kind))).collect();
        actions.push(Action::new(MENU, "Open a menu", "A group of actions, as the leader and its key open it", S::Menu).param(Param::Choice(choices)).group(Group::View).importance(MINOR));
        let index = |id: &str, label: &str, detail: &str, op: IndexOp| Action::new(id, label, detail, S::Index(op)).group(Group::Act).importance(MINOR);
        actions.push(index("index.changed", "Re-read the code that changed", "The index takes in the files changed since it last read them", IndexOp::Changed));
        actions.push(index("index.rebuild", "Rebuild the index from scratch", "Reads every file again (slow on a big project): when the index seems wrong", IndexOp::Rebuild));
        actions
    }

    /// The level around the panel, its host: a panel's own (what it
    /// shows, closing it), or the quick look's.
    pub(crate) fn host_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        if here.at == Host::Look && self.quick_look.is_some() {
            let look = |id: &str, label: &str, detail: &str, c: ShellCommand| Action::new(id, label, detail, c).group(Group::View);
            return vec![
                look("quicklook.toggle", "Close the quick look", "Back to the panel it floats over", S::QuickLook).importance(PRIMARY),
                look("quicklook.to_panel", "Open it in a panel", "What the quick look shows, in a panel of its own", S::QuickLookToPanel).importance(PRIMARY),
            ];
        }
        let admin = |id: String, label: String, detail: String, c: ShellCommand| Action::new(id, label, detail, c).group(Group::View).importance(MINOR);
        let mut actions = vec![Action::new("panel.close", "Close this panel (its buffer is hidden, not lost)", "The panel goes; its buffer stays open, hidden, and comes back with more panels", S::ClosePanel).group(Group::View).importance(MINOR)];
        for kind in BufferKind::ALL {
            actions.push(admin(format!("buffer.{}", kind.name()), format!("Show {} in this panel", kind.name()), format!("The {} buffer you used last, here (or a new one beside it)", kind.name()), S::ShowBuffer(kind)));
        }
        // Every open view, to show here: on screen (and where) or hidden.
        let views: Vec<Choice> = self
            .desk.panels
            .buffers()
            .iter()
            .enumerate()
            .map(|(panel, p)| (*p, format!("panel {}", panel + 1)))
            .chain(self.desk.panels.put_away().map(|p| (p, "hidden".to_owned())))
            .map(|(p, where_)| Choice::new(format!("{}:{}", p.kind.name(), p.id), self.describe(p)).detail(Some(where_)).current(p == self.desk.panels.focused_buffer()))
            .collect();
        actions.push(Action::new("buffer.list", "Open buffers: show one here", "Every open buffer, on screen or hidden: pick one to show it in this panel", S::PickView).param(Param::Choice(views)).group(Group::View).importance(SECONDARY));
        actions.push(admin("buffer.delete".into(), "Delete this panel's buffer (for good)".into(), "The buffer and its state go; the panel shows another".into(), S::DeleteView));
        for kind in BufferKind::ALL.into_iter().filter(|k| !k.one_only()) {
            actions.push(admin(format!("buffer.new.{}", kind.name()), format!("New {} buffer here", kind.name()), format!("Another {} buffer, beside the ones open, starting where the last was", kind.name()), S::NewView(kind)));
        }
        actions
    }

    /// What every panel has besides its view's own, the same for every
    /// view: hints, search, marks and the jump list, its keys, a line or
    /// row by number, the go-tos from its cursor, the quick look on it,
    /// talking to the agent from here.
    pub(crate) fn panel_actions(&self, here: Here) -> Vec<Action> {
        use ShellCommand as S;
        let nav = |id: &str, label: &str, detail: &str, c: ShellCommand| Action::new(id, label, detail, c).group(Group::Navigate);
        let mut out = vec![
            nav("nav.hint", "Jump to something on screen", "Labels what's drawn: type a label to put the cursor there", S::Hint { open: false }).importance(PRIMARY),
            nav("nav.hint_open", "Open something on screen", "Labels what's drawn: type a label to open it, as going into it would", S::Hint { open: true }).importance(SECONDARY).icon(egui_phosphor::regular::CURSOR_CLICK),
            nav("nav.search", "Search this buffer", "Finds as you type, names first (letters in order), then text", S::Search).importance(PRIMARY).icon(egui_phosphor::regular::MAGNIFYING_GLASS),
            nav("nav.search_next", "Next match", "The next thing the last search found", S::SearchStep { forward: true }).importance(SECONDARY).when(When::Searched),
            nav("nav.search_prev", "Previous match", "The thing before, of what the last search found", S::SearchStep { forward: false }).importance(MINOR).when(When::Searched),
            nav("nav.mark_set", "Set a mark here", "A letter to come back here with (going to the mark brings you back)", S::MarkSet).importance(MINOR).icon(egui_phosphor::regular::BOOKMARK_SIMPLE),
            nav("nav.mark_jump", "Jump to a mark", "Back to where a letter was set, in whichever buffer", S::MarkJump).importance(MINOR).when(When::Marked),
            nav("nav.back", "Back (jump list)", "Where you were before the last jump, in any buffer or panel", S::Back).importance(SECONDARY).when(When::Back).icon(egui_phosphor::regular::ARROW_LEFT),
            nav("nav.forward", "Forward (jump list)", "Where you went back from", S::Forward).importance(MINOR).when(When::Forward).icon(egui_phosphor::regular::ARROW_RIGHT),
            Action::new("nav.keys", "Action list: everything", "Every action, faded where it doesn't apply, and the keys that run nothing themselves", S::Keys).group(Group::View).importance(MINOR),
            Action::new("talk.open", "Talk to the agent from here", "The lead's chat over what's on screen, ready to type and send", S::Talk).group(Group::Act).importance(SECONDARY),
            Action::new(
                "terminal.popup",
                if self.popped_terminal() { "Hide the pop-up terminal" } else { "Pop up a terminal over everything" },
                "A terminal of its own over whatever's on screen; the same key hides it",
                S::PopTerminal,
            )
            .group(Group::Act)
            .importance(SECONDARY),
            nav("nav.line", "Go to line or row", "By its number (a terminal's lines by the numbers they keep for life)", S::Line).param(Param::Text { hint: "line or row number".into(), start: String::new() }).importance(MINOR),
        ];
        out.extend(self.tag_actions(here));
        out.extend(self.link_actions(here));
        out.extend(self.item_actions(here));
        out.extend(self.discuss_actions(here));
        // Each go-to's destination says whether it can show what the
        // cursor is on; the relational ones, whether its relations lead
        // anywhere.
        let target = self.desk.view(here.kind).target();
        for op in GoTo::ALL {
            let when = match op.view() {
                Some(view) if target.as_ref().is_some_and(|t| self.desk.view(view).can_show(t)) => When::Always,
                Some(_) => When::Unavailable,
                None => When::Reaches(op),
            };
            out.push(Action::new(op.id(), op.label(), op.detail(), S::GoTo(op)).group(Group::GoTo).when(when).importance(op.importance()));
        }
        // On what you're on: its full card (open, it's the quick look's).
        if self.quick_look.is_none() {
            out.push(Action::new("quicklook.toggle", "Quick look: the full card (hold to peek)", "The full card of what you're on, floating over the panel: hold the key to peek", S::QuickLook).group(Group::View).importance(PRIMARY).when(When::Component));
        }
        if let Some(choosing) = &self.desk.choosing {
            let name = |id: &ComponentId| self.component_name(id);
            let choices = choosing.candidates.iter().map(|c| Choice::new(c.id.as_str(), name(&c.id)).detail(Some(c.id.to_string()))).collect();
            out.push(Action::new("goto.pick", format!("Go to which {}?", choosing.op.one()), "It leads to several: pick one", S::GotoPick).param(Param::Choice(choices)).group(Group::GoTo));
        }
        out
    }

    /// Where you are: the view with the keyboard, and what holds it (the
    /// quick look, or its panel).
    pub(crate) fn here(&self) -> Here {
        match &self.quick_look {
            Some(look) => Here { at: Host::Look, kind: look.kind() },
            None => Here { at: Host::Panel(self.desk.panels.focused()), kind: self.desk.panels.focused_kind() },
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
        let bound = self.gather(self.here());
        if self.in_frame.get() {
            *self.frame_actions.borrow_mut() = Some(bound.clone());
        }
        bound
    }

    /// What a view shows where it is (`here`), the same way: gathered,
    /// placed, in layers and bound to keys (its groups read them; a panel
    /// not focused says none on its controls: they don't run it until it
    /// is).
    pub(crate) fn shown_at(&self, here: Here) -> Vec<Bound> {
        if here == self.here() { self.bound_actions() } else { self.gather(here) }
    }

    /// The list for `here`: every level of where you are, innermost first,
    /// each offering its actions the same way: the panel (its view's own,
    /// its moves among them, and what every panel has), its host (the
    /// panel's or the quick look's), and everywhere (the shell's, and the
    /// other views' global actions). An action's layer is the level that
    /// offered it, or what you're on when its control is on the item. Each
    /// is placed (the panel's, its host's and the top bar's placements:
    /// how it shows, never its level) and bound to keys.
    pub(super) fn gather(&self, here: Here) -> Vec<Bound> {
        #[cfg(test)]
        self.gathered.set(self.gathered.get() + 1);
        let kind = here.kind;
        let view = |k: BufferKind, all: bool| self.desk.view(k).actions().into_iter().filter(move |a| all || a.scope == Scope::Global).map(move |a| (Origin::View(k), a));
        let mut levels: Vec<(Layer, Origin, Action)> = Vec::new();
        levels.extend(view(kind, true).map(|(o, a)| (Layer::Panel, o, a)));
        levels.extend(self.panel_actions(here).into_iter().map(|a| (Layer::Panel, Origin::Shell, a)));
        levels.extend(self.host_actions(here).into_iter().map(|a| (Layer::Host, Origin::Shell, a)));
        levels.extend(self.shell_actions().into_iter().map(|a| (Layer::Everywhere, Origin::Shell, a)));
        // Another view's global actions are here for their keys and the
        // palette; their controls are their own panel's, not drawn here.
        for k in BufferKind::ALL.into_iter().filter(|k| *k != kind) {
            levels.extend(view(k, false).map(|(o, mut a)| {
                a.control = None;
                (Layer::Everywhere, o, a)
            }));
        }
        for (id, control) in self.placements(here) {
            let found = levels.iter().position(|(_, o, a)| *o == Origin::View(kind) && a.id == id).or_else(|| levels.iter().position(|(_, o, a)| *o == Origin::Shell && a.id == id));
            if let Some(i) = found {
                levels[i].2.control = Some(control);
            }
        }
        let tagged: Vec<Layer> = levels.iter().map(|(level, _, action)| if action.control.as_ref().is_some_and(|c| matches!(c.at, crate::actions::At::Item | crate::actions::At::Typing)) { Layer::On } else { *level }).collect();
        let actions: Vec<(Origin, Action)> = levels.into_iter().map(|(_, o, a)| (o, a)).collect();
        // The keys given out where it is, again only when its actions changed
        // (compared as they are, nothing built to compare).
        let mut cache = self.bound_cache.borrow_mut();
        let at = cache.iter().position(|((h, _), _)| *h == here);
        let same = |(_, kept): &KeyedActions| kept.len() == actions.len() && kept.iter().zip(&actions).all(|((o, id, aliases), (origin, a))| o == origin && *id == a.id && *aliases == a.aliases);
        let keys: Vec<Keys> = match at {
            Some(at) if same(&cache[at].0) => cache[at].1.clone(),
            _ => {
                let keys: Vec<Keys> = self.bind_actions(actions.clone(), kind).into_iter().map(|b| (b.leader, b.keys, b.gestures)).collect();
                let kept = ((here, actions.iter().map(|(origin, a)| (*origin, a.id.clone(), a.aliases.clone())).collect()), keys.clone());
                match at {
                    Some(at) => cache[at] = kept,
                    None => {
                        // Where it was gathered lately (the hosts on screen).
                        if cache.len() == KEPT_KEYS {
                            cache.remove(0);
                        }
                        cache.push(kept);
                    }
                }
                keys
            }
        };
        drop(cache);
        actions.into_iter().zip(keys).zip(tagged).map(|(((origin, action), (leader, keys, gestures)), layer)| Bound { action, origin, leader, keys, gestures, layer }).collect()
    }

    /// Every placement for `here`: the panel's (on its top line, or under
    /// what you're on), the top bar's. (A host's own are in its groups.)
    fn placements(&self, here: Here) -> Vec<(String, crate::actions::Control)> {
        self.desk.view(here.kind).placed().into_iter().chain(self.top_bar_placements()).collect()
    }

    /// The actions bound to their keys, the focused view's own getting the
    /// view menu's free letters.
    pub(super) fn bind_actions(&self, actions: Vec<(Origin, Action)>, focused: BufferKind) -> Vec<Bound> {
        let mut bound = self.config.keymap.bind(actions);
        // The focused view's own menu: its actions without a leader path get
        // the next free keys under the view-menu key.
        let menu = self.config.keymap.view_menu;
        let mut taken: Vec<char> = bound.iter().filter_map(|b| b.leader.as_ref().filter(|l| l.len() == 2 && l[0] == menu).map(|l| l[1])).collect();
        // The shared moves a view offers have their keys in the keymap.
        for b in bound.iter_mut().filter(|b| b.origin == Origin::View(focused) && b.leader.is_none() && b.action.command::<Move>().is_none()) {
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
            // Their own actions; not their shared moves, which act where you are.
            for mut action in self.desk.view(kind).actions().into_iter().filter(|a| a.scope == Scope::Focused && a.command::<Move>().is_none()) {
                action.label = format!("{} · {}", kind.words(), action.label);
                out.push(Bound { action, origin: Origin::View(kind), leader: None, keys: Vec::new(), gestures: Vec::new(), layer: Layer::Everywhere });
            }
        }
        out
    }

    pub fn invoke(&mut self, invocation: Invocation, ctx: &egui::Context) {
        self.invoke_counted(invocation, None, ctx);
    }

    /// Runs an action with the count typed before its key, if one was: moves
    /// take it, a go-to lands in that panel, a view's own actions repeat,
    /// anything else runs once.
    pub(super) fn invoke_counted(&mut self, invocation: Invocation, count: Option<u32>, ctx: &egui::Context) {
        // An action changes what can run as it goes: it sees the actions as
        // they are, and what's drawn after it sees what it left.
        let drawing = self.in_frame.replace(false);
        #[cfg(test)]
        self.ran.borrow_mut().push(invocation.action.id.clone());
        self.run_counted(invocation, count, ctx);
        // What the buffers asked for, done now (the inbox's next item, a chat…).
        self.take_asks(ctx);
        self.share_with_buffers();
        self.in_frame.set(drawing);
        self.forget_frame();
    }

    pub(super) fn run_counted(&mut self, invocation: Invocation, count: Option<u32>, ctx: &egui::Context) {
        let Invocation { action, origin, value } = invocation;
        match origin {
            Origin::Shell => {
                if let Some(command) = action.command::<ShellCommand>() {
                    self.run_shell(command.clone(), value, count.map(|c| c as usize), ctx);
                }
            }
            // A shared move a view offered: run the same way for every view.
            Origin::View(kind) if action.command::<Move>().is_some() => {
                if let Some(m) = action.command::<Move>() {
                    self.run_move(kind, *m, count.map(|c| c as usize), ctx);
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
                self.open_palette(&b);
            } else {
                self.invoke(Invocation { action: b.action, origin: b.origin, value }, ctx);
            }
        }
    }

    /// Opens the action list, as far as `reach`, and gives it the keyboard.
    pub(super) fn open_list(&mut self, reach: crate::sheet::Reach) {
        self.palette.open(reach);
        self.palette_takes_keys();
    }

    /// Opens the palette at an action's parameter and gives it the keyboard.
    pub(super) fn open_palette(&mut self, action: &Bound) {
        self.palette.open_for(action);
        self.palette_takes_keys();
    }

    /// The palette has the keyboard (what had it is kept, to go back to).
    fn palette_takes_keys(&mut self) {
        if self.input_mode != InputMode::Palette {
            self.palette_from = self.input_mode;
        }
        self.input_mode = InputMode::Palette;
    }

    /// Runs an action (a switch's key flips it), or opens the palette at
    /// its parameter if it needs one (where it doesn't apply, why not is
    /// said instead, as the action list does).
    pub(super) fn run_bound(&mut self, bound: &Bound, count: Option<u32>, ctx: &egui::Context) {
        let offered = crate::sheet::Offered { applies: self.applies(bound, self.here()), bound: bound.clone() };
        if let Some(why) = offered.refused() {
            self.note = Some(why);
            return;
        }
        match &bound.action.param {
            Param::None => self.invoke_counted(Invocation { action: bound.action.clone(), origin: bound.origin, value: None }, count, ctx),
            Param::Toggle { on } => self.invoke(Invocation { action: bound.action.clone(), origin: bound.origin, value: Some((!on).to_string()) }, ctx),
            _ => self.open_palette(bound),
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
            ShellCommand::FocusPanel(panel) => {
                self.desk.panels.focus(panel);
            }
            ShellCommand::CyclePanel(dir) => self.desk.panels.cycle(dir),
            ShellCommand::ShowBuffer(kind) => {
                let buffer = self.desk.panels.show(self.desk.panels.focused(), kind);
                self.use_buffer(buffer);
            }
            ShellCommand::SetLayout(cols) if cols > self.config.panels.max => self.note = Some(format!("{} panels at most (`[panels] max` in your UI config).", self.config.panels.max)),
            ShellCommand::SetLayout(cols) => self.desk.panels.set_count(cols, self.config.panels.max),
            ShellCommand::ClosePanel => self.close_panel(self.desk.panels.focused()),
            ShellCommand::DeleteView => self.delete_view(),
            ShellCommand::NewView(kind) => self.new_view(kind),
            ShellCommand::PickView => {
                let picked = value.as_deref().and_then(|v| {
                    let (kind, id) = v.split_once(':')?;
                    let kind = BufferKind::ALL.into_iter().find(|k| k.name() == kind)?;
                    Some(BufferId::new(kind, id.parse().ok()?))
                });
                match picked.filter(|p| self.desk.panels.has(*p)) {
                    Some(buffer) => self.show_view(buffer),
                    None => self.note = Some("That buffer isn't open any more.".into()),
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
            ShellCommand::QuickLookToPanel => self.quick_look_to_panel(),
            ShellCommand::Talk => self.talk(ctx),
            ShellCommand::PopTerminal => self.pop_terminal(ctx),
            ShellCommand::ItemButtons => {
                let bar = value.as_deref() == Some("true");
                self.item_buttons = Some(if bar { ItemButtons::Bar } else { ItemButtons::Floating });
                self.note = Some(if bar { "The buttons of what you're on are in a bar along each panel's bottom." } else { "The buttons of what you're on float under it." }.into());
            }
            ShellCommand::Agents => {
                self.desk.remote.agents = value.as_deref() == Some("true");
                self.note = Some(if self.desk.remote.agents { "Agents can drive the cockpit (the server's cockpit tools)." } else { "Agents can't drive the cockpit." }.into());
            }
            ShellCommand::OpenPalette => self.open_list(crate::sheet::Reach::Here),
            ShellCommand::Line => match value.as_deref().map(str::trim).and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => self.run_move(kind, Move::Motion(Motion::Line(n)), None, ctx),
                None => self.note = Some("Give a line number.".into()),
            },
            ShellCommand::Hint { open } => self.start_hint(open),
            ShellCommand::Search => {
                self.capture = Some(Capture::Search(Searching { kind, query: String::new(), before: self.jump_here() }));
                self.input_mode = InputMode::Capture { at: self.here().at };
                self.desk.view_mut(kind).save_view();
                // An empty search is a search too (a filter typed into goes blank).
                self.desk.view_mut(kind).on_search(SearchEvent::Typing(""));
            }
            ShellCommand::SearchStep { forward } => self.search_step(forward, count),
            ShellCommand::MarkSet => self.start_capture(Capture::MarkSet),
            ShellCommand::Tag | ShellCommand::Untag(_) | ShellCommand::TagGoTo | ShellCommand::TagGoToPick | ShellCommand::TagStep { .. } | ShellCommand::TagLevel | ShellCommand::TagEdit(..) | ShellCommand::TagEditWhich(_) | ShellCommand::TagInsert => self.run_tag(&command, value, ctx),
            ShellCommand::LinkThing => self.run_link(value),
            ShellCommand::LinkInclude => self.cycle_link(),
            ShellCommand::ItemState(plan, item) => self.set_item_state(&plan, &item, value, ctx),
            ShellCommand::Discuss | ShellCommand::DiscussLevel | ShellCommand::DiscussAsk | ShellCommand::DiscussWrite | ShellCommand::DiscussOpen => self.run_discuss(&command, value, ctx),
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
                    Some(jump) => self.restore_jump(jump),
                    None => self.note = Some(if command == ShellCommand::Back { "Nothing to go back to." } else { "Nothing to go forward to." }.into()),
                }
            }
            // The action list on everything.
            ShellCommand::Keys => self.open_list(crate::sheet::Reach::Everything),
            ShellCommand::Menu => {
                let path: Vec<char> = value.as_deref().and_then(|v| v.chars().next()).into_iter().collect();
                self.open_menu(&path, ctx);
            }
            ShellCommand::GoTo(op) => {
                if let Some(number) = typed
                    && !self.land_in(number)
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

    /// A shared move, on the view of `kind`: the same for every view. Big
    /// moves and going in or out are jumps; small ones aren't.
    pub(crate) fn run_move(&mut self, kind: BufferKind, m: Move, count: Option<usize>, ctx: &egui::Context) {
        let before = self.jump_here();
        match m {
            Move::Motion(motion) => {
                self.desk.view_mut(kind).motion(motion, count.unwrap_or(1).max(1));
                if matches!(motion, Motion::First | Motion::Last | Motion::Line(_)) {
                    self.after_jump(before);
                }
            }
            Move::Enter | Move::Leave => {
                let step = if m == Move::Enter { self.desk.view_mut(kind).enter() } else { self.desk.view_mut(kind).leave() };
                self.take_step(step, before, ctx);
            }
            Move::Fold(fold) => self.desk.view_mut(kind).fold(fold),
        }
    }

    pub(super) fn start_capture(&mut self, capture: Capture) {
        self.capture = Some(capture);
        self.input_mode = InputMode::Capture { at: self.here().at };
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
