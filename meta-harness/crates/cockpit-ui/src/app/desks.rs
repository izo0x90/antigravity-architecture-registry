//! A desk per server: the one shown, the others put away. Home's socket
//! brings every server's events, each to its desk; every desk takes its
//! events each frame, and only the one shown draws, takes keys and fetches
//! what its views show (a desk fetches when it's shown). The servers'
//! actions: switch, add, connect, disconnect, remove, shut down.

use harness_contracts::home::HERE;
use harness_contracts::{ClientEvent, HomeEvent, LinkState, ServerView};
use serde::{Deserialize, Serialize};

use super::*;

/// The cockpit's own state, kept on home: the server shown, whether
/// system notifications are on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shell {
    pub current: String,
    #[serde(default)]
    pub alerts: bool,
}

/// How long before asking home for the shell's state again.
const SHELL_RETRY_S: f64 = 5.0;

/// Loads the shell's state from home, then saves it a moment after it
/// changes.
#[derive(Default)]
pub struct ShellKeeper {
    asked: bool,
    /// When to ask again, after home didn't answer.
    retry_at: f64,
    loaded: bool,
    reply: Arc<Mutex<Option<NetResult<Option<serde_json::Value>>>>>,
    saved: Option<Shell>,
    changed_at: Option<f64>,
    /// The server it showed, to show again once home lists it.
    pub want: Option<String>,
}

impl CockpitApp {
    /// What home's socket said: the servers (a desk made for each new one,
    /// a removed one's put away for good) and each server's events, to its
    /// desk (`Resync`: it drops what it holds and refetches when shown).
    pub(super) fn take_from_home(&mut self, ctx: &egui::Context) {
        self.home.poll_ws(ctx);
        let said = self.home.from_home.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default();
        for (came_ms, event) in said {
            match event {
                HomeEvent::Servers { servers } => self.servers_now(servers, ctx),
                HomeEvent::Event { server, event } => match self.desk_mut(&server) {
                    Some(desk) => {
                        if event.event == ClientEvent::Resync {
                            desk.net.reconnected = true;
                        }
                        if let Ok(mut incoming) = desk.net.incoming.lock() {
                            incoming.push((came_ms, *event));
                        }
                    }
                    None => log::warn!("an event for {server}, which has no desk"),
                },
            }
        }
    }

    fn desk_mut(&mut self, server: &str) -> Option<&mut Desk> {
        if self.desk.server == server { Some(&mut self.desk) } else { self.away.get_mut(server) }
    }

    /// Every desk: the one shown, then the others.
    pub(super) fn desks(&self) -> impl Iterator<Item = &Desk> {
        std::iter::once(&self.desk).chain(self.away.values())
    }

    fn servers_now(&mut self, servers: Vec<ServerView>, ctx: &egui::Context) {
        for s in servers.iter().filter(|s| s.name != HERE) {
            if self.desk.server != s.name && !self.away.contains_key(&s.name) {
                let desk = Desk::new(&self.host_origin, &s.name);
                self.away.insert(s.name.clone(), desk);
            }
        }
        self.away.retain(|name, _| servers.iter().any(|s| &s.name == name));
        if self.desk.server != HERE && !servers.iter().any(|s| s.name == self.desk.server) {
            let gone = self.desk.server.clone();
            self.switch_to(HERE, ctx);
            self.away.remove(&gone);
            self.note = Some(format!("{gone} was removed: showing this server's own."));
        }
        // A home with no project of its own shows another server's (none
        // yet: it says how to link one).
        let projectless = servers.iter().any(|s| s.name == HERE && !s.project);
        let had = std::mem::replace(&mut self.servers, servers);
        if projectless && self.desk.server == HERE {
            match self.servers.iter().find(|s| s.name != HERE).map(|s| s.name.clone()) {
                Some(other) => self.switch_to(&other, ctx),
                None if had.is_empty() => {
                    let key = self.bound_actions().into_iter().find(|b| b.action.id == "server.add").map(|b| format!(" ({})", b.key_hint())).unwrap_or_default();
                    self.note = Some(format!("This home serves no project: link a server{key} to see one."));
                }
                None => {}
            }
        }
    }

    /// The desks not shown take their events (each, in turn, as the one shown).
    pub(super) fn tick_away(&mut self, ctx: &egui::Context) {
        let names: Vec<String> = self.away.keys().cloned().collect();
        for name in names {
            self.as_shown(&name, |app| {
                app.take_events(ctx);
                app.serve_calls_away(ctx);
            });
        }
    }

    /// Runs `f` with server `name`'s desk as the one shown, then puts it back.
    fn as_shown(&mut self, name: &str, f: impl FnOnce(&mut Self)) {
        let Some(mut desk) = self.away.remove(name) else { return };
        std::mem::swap(&mut self.desk, &mut desk);
        f(self);
        std::mem::swap(&mut self.desk, &mut desk);
        self.away.insert(name.to_owned(), desk);
    }

    /// Shows server `name`'s desk (the quick look closes; a desk shown the
    /// first time loads what it shows).
    pub(super) fn switch_to(&mut self, name: &str, ctx: &egui::Context) {
        if self.desk.server == name {
            return;
        }
        let Some(mut desk) = self.away.remove(name) else {
            self.note = Some(format!("There's no server named {name:?} (home lists them)."));
            return;
        };
        self.close_quick_look();
        self.input_mode = InputMode::Normal;
        self.desk.put_away();
        std::mem::swap(&mut self.desk, &mut desk);
        desk.remote.hand_over(&mut self.desk.remote);
        self.away.insert(desk.server.clone(), desk);
        if !self.desk.opened {
            self.open_desk(ctx);
        }
    }

    /// How many wait on the user at each server.
    fn waiting_at(&self, server: &str) -> usize {
        self.desks().find(|d| d.server == server).map_or(0, |d| crate::attention::counts(d.shared.agents.get().store()).0)
    }

    /// The shell's state: loaded once from home (the server it showed is
    /// shown again once its desk is there), saved a moment after it changes.
    pub(super) fn keep_shell(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        if !self.shell.asked && now >= self.shell.retry_at {
            self.shell.asked = true;
            let reply = self.shell.reply.clone();
            self.home.ui_shell(ctx.clone(), move |r| {
                if let Ok(mut slot) = reply.lock() {
                    *slot = Some(r);
                }
            });
        }
        if let Some(reply) = self.shell.reply.lock().ok().and_then(|mut r| r.take()) {
            self.shell.loaded = reply.is_ok();
            match reply.map(|v| v.map(serde_json::from_value::<Shell>)) {
                Ok(Some(Ok(kept))) => {
                    self.alerts.restore(kept.alerts);
                    self.shell.want = Some(kept.current.clone());
                    self.shell.saved = Some(kept);
                }
                Ok(Some(Err(e))) => self.note = Some(format!("The cockpit's own state on home can't be read ({e}): starting afresh.")),
                Ok(None) => {}
                // Home doesn't answer (the SRV light says so): asked again
                // in a while; nothing is saved over it till it's read.
                Err(e) => {
                    log::warn!("the cockpit's own state couldn't be read from home: {}", e.message);
                    self.shell.asked = false;
                    self.shell.retry_at = now + SHELL_RETRY_S;
                }
            }
        }
        if let Some(want) = self.shell.want.clone()
            && (want == self.desk.server || self.away.contains_key(&want) || (!self.servers.is_empty() && !self.servers.iter().any(|s| s.name == want)))
        {
            self.shell.want = None;
            self.switch_to(&want, ctx);
        }
        if !self.shell.loaded || self.shell.want.is_some() {
            return;
        }
        let shell = Shell { current: self.desk.server.clone(), alerts: self.alerts.on() };
        if self.shell.saved.as_ref() == Some(&shell) {
            self.shell.changed_at = None;
            return;
        }
        let since = *self.shell.changed_at.get_or_insert(now);
        if now - since >= self.config.net.save_after_ms as f64 / 1000.0 {
            let later = self.notes_later.clone();
            if let Ok(value) = serde_json::to_value(&shell) {
                self.home.put_ui_shell(value, ctx.clone(), move |r| {
                    if let (Err(e), Ok(mut notes)) = (r, later.lock()) {
                        notes.push(format!("The cockpit's own state wasn't saved on home: {}", e.message));
                    }
                });
            }
            self.shell.saved = Some(shell);
            self.shell.changed_at = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(self.config.net.save_after_ms));
        }
    }

    /// The servers' actions.
    pub(super) fn server_actions(&self) -> Vec<Action> {
        use ShellCommand as S;
        let act = |id: &str, label: &str, detail: &str, c: ShellCommand| Action::new(id, label, detail, c).group(Group::Act).importance(MINOR);
        let link = |s: &ServerView| match &s.link {
            LinkState::Connected => "connected".to_owned(),
            LinkState::Connecting => "connecting".to_owned(),
            LinkState::Lost { reason } => format!("lost: {reason}"),
            LinkState::Disconnected => "disconnected".to_owned(),
        };
        let mut known: Vec<&ServerView> = self.servers.iter().filter(|s| s.project).collect();
        known.sort_by_key(|s| std::cmp::Reverse(self.waiting_at(&s.name)));
        let choice = |s: &ServerView| {
            let waiting = self.waiting_at(&s.name);
            let detail = if waiting > 0 { format!("{waiting} waiting · {}", link(s)) } else { link(s) };
            Choice::new(&s.name, &s.name).detail(Some(detail)).current(s.name == self.desk.server)
        };
        let mut out = Vec::new();
        if known.len() > 1 {
            out.push(act("server.switch", "Show another server", "Its project, on its own desk (the others stay live)", S::ServerSwitch).param(Param::Choice(known.iter().map(|s| choice(s)).collect())).importance(SECONDARY));
        }
        out.push(act("server.add", "Link another server", "Its name, address and token: the cockpit follows it from then on", S::ServerAdd).param(Param::Text { hint: "name http://host:port secret (`meta-harness token` there)".into(), start: String::new() }));
        let others = |keep: fn(&LinkState) -> bool| -> Vec<Choice> { self.servers.iter().filter(|s| s.name != HERE && keep(&s.link)).map(choice).collect() };
        for (id, label, detail, command, keep) in [
            ("server.connect", "Follow a server again", "Its events and calls come through again", S::ServerConnect, (|l: &LinkState| matches!(l, LinkState::Disconnected | LinkState::Lost { .. })) as fn(&LinkState) -> bool),
            ("server.disconnect", "Stop following a server (it runs on)", "The cockpit stops listening to it; it keeps running", S::ServerDisconnect, |l: &LinkState| matches!(l, LinkState::Connected | LinkState::Connecting)),
            ("server.remove", "Unlink a server", "Forgets it (it keeps running; link it again to follow it)", S::ServerRemove, |_: &LinkState| true),
        ] {
            let choices = others(keep);
            if !choices.is_empty() {
                out.push(act(id, label, detail, command).param(Param::Choice(choices)));
            }
        }
        if !self.servers.is_empty() {
            out.push(act("server.shutdown", "Shut a server down", "It stops (asks first): its agents and terminals end", S::ServerShutdown).param(Param::Choice(self.servers.iter().map(choice).collect())));
        }
        if let Some(name) = &self.confirming_shutdown {
            let choices = vec![Choice::new("yes", format!("Shut {name} down")), Choice::new("no", "Keep it running")];
            out.push(act("server.shutdown_confirm", &format!("Shut {name} down?"), "Its agents and terminals end with it", S::ServerShutdownConfirm).param(Param::Choice(choices)).importance(PRIMARY));
        }
        out
    }

    pub(super) fn run_server(&mut self, command: ShellCommand, value: Option<String>, ctx: &egui::Context) {
        let later = self.notes_later.clone();
        let say = move |what: String| {
            move |r: NetResult<()>| {
                if let (Err(e), Ok(mut notes)) = (r, later.lock()) {
                    notes.push(format!("{what}: {}", e.message));
                }
            }
        };
        match (command, value) {
            (ShellCommand::ServerSwitch, Some(name)) => self.switch_to(&name, ctx),
            (ShellCommand::ServerAdd, Some(text)) => match text.split_whitespace().collect::<Vec<_>>().as_slice() {
                [name, url, secret] => self.home.add_server(name, url, secret, ctx.clone(), say(format!("{name} wasn't added"))),
                _ => self.note = Some("A server is `name http://host:port secret` (its secret: `meta-harness token` there).".into()),
            },
            (ShellCommand::ServerConnect, Some(name)) => self.home.server_link(&name, true, ctx.clone(), say(format!("{name} wasn't followed"))),
            (ShellCommand::ServerDisconnect, Some(name)) => self.home.server_link(&name, false, ctx.clone(), say(format!("{name} wasn't let go"))),
            (ShellCommand::ServerRemove, Some(name)) => self.home.remove_server(&name, ctx.clone(), say(format!("{name} wasn't removed"))),
            (ShellCommand::ServerShutdown, Some(name)) => {
                self.confirming_shutdown = Some(name);
                self.run_by_id("server.shutdown_confirm", None, ctx);
            }
            (ShellCommand::ServerShutdownConfirm, Some(answer)) => {
                if let Some(name) = self.confirming_shutdown.take()
                    && answer == "yes"
                {
                    NetClient::for_server(&self.host_origin, &name).shutdown(ctx.clone(), say(format!("{name} wasn't shut down")));
                    self.note = Some(format!("Asked {name} to shut down."));
                }
            }
            _ => {}
        }
    }

}

