//! The desk's terminals: which the server runs and how each is (from the
//! event stream), and opening, restarting and closing them (what a buffer
//! asks, `Ask::Terminals`). Kept on the desk (`Shared::terminals`); a
//! buffer showing one reads it through a reader and holds its own socket
//! to it (`TermLink`), so the server sees each panel as a reader at its
//! own size.

mod link;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use harness_contracts::{ClientEvent, Terminal, TerminalId};

pub use link::TermLink;

use crate::net::{NetClient, NetResult};

/// What a buffer asks the terminals to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermAct {
    /// A new terminal (the buffer that asked shows it: `made`).
    Open,
    /// Its shell started again (it ended).
    Restart,
    /// Its shell ended, and it's forgotten.
    Close,
}

enum Reply {
    Opened(NetResult<Terminal>),
    Restarted(TerminalId, NetResult<Terminal>),
    Closed(TerminalId, NetResult<()>),
}

type Mailbox = Arc<Mutex<Vec<Reply>>>;

#[derive(Default)]
pub struct Terminals {
    list: BTreeMap<TerminalId, Terminal>,
    /// What went wrong, by terminal (none: opening one).
    errors: BTreeMap<Option<TerminalId>, String>,
    /// How many were opened here, and the last one.
    made: (u64, Option<TerminalId>),
    opening: bool,
    mailbox: Mailbox,
}

impl Terminals {
    /// Takes what the server says about its terminals.
    pub fn apply(&mut self, event: &ClientEvent) {
        match event {
            ClientEvent::TerminalUpdated { terminal } => {
                self.list.insert(terminal.id.clone(), terminal.clone());
            }
            ClientEvent::TerminalClosed { terminal_id } => {
                self.list.remove(terminal_id);
            }
            // The whole state follows: start from nothing.
            ClientEvent::Resync => self.list.clear(),
            _ => {}
        }
    }

    pub fn list(&self) -> impl Iterator<Item = &Terminal> {
        self.list.values()
    }

    pub fn get(&self, id: &TerminalId) -> Option<&Terminal> {
        self.list.get(id)
    }

    /// The one `by` steps from `id` in the list (wrapping), or the first.
    pub fn beside(&self, id: Option<&TerminalId>, by: i32) -> Option<TerminalId> {
        let ids: Vec<&TerminalId> = self.list.keys().collect();
        let at = id.and_then(|id| ids.iter().position(|i| *i == id));
        let n = ids.len() as i64;
        if n == 0 {
            return None;
        }
        let next = match at {
            Some(at) => (at as i64 + i64::from(by)).rem_euclid(n),
            None => 0,
        };
        Some(ids[next as usize].clone())
    }

    /// How many were opened here, and the last one (a buffer that asked
    /// for one shows it once the count moves).
    pub fn made(&self) -> (u64, Option<&TerminalId>) {
        (self.made.0, self.made.1.as_ref())
    }

    pub fn opening(&self) -> bool {
        self.opening
    }

    /// What went wrong with `id` (none: opening one).
    pub fn error(&self, id: Option<&TerminalId>) -> Option<&String> {
        self.errors.get(&id.cloned())
    }

    /// Does what a buffer asked.
    pub fn act(&mut self, id: Option<TerminalId>, act: TermAct, net: &NetClient, ctx: &egui::Context) {
        self.errors.remove(&id);
        let mailbox = self.mailbox.clone();
        let post = move |reply| {
            if let Ok(mut m) = mailbox.lock() {
                m.push(reply);
            }
        };
        match (act, id) {
            (TermAct::Open, _) => {
                self.opening = true;
                net.open_terminal(ctx.clone(), move |r| post(Reply::Opened(r)));
            }
            (TermAct::Restart, Some(id)) => net.restart_terminal(&id.clone(), ctx.clone(), move |r| post(Reply::Restarted(id, r))),
            (TermAct::Close, Some(id)) => net.close_terminal(&id.clone(), ctx.clone(), move |r| post(Reply::Closed(id, r))),
            (_, None) => {}
        }
    }

    /// Takes the replies to what was asked.
    pub fn tick(&mut self) {
        let replies = self.mailbox.lock().map(|mut m| std::mem::take(&mut *m)).unwrap_or_default();
        for reply in replies {
            match reply {
                Reply::Opened(Ok(terminal)) => {
                    self.opening = false;
                    self.made = (self.made.0 + 1, Some(terminal.id.clone()));
                    self.list.insert(terminal.id.clone(), terminal);
                }
                Reply::Opened(Err(e)) => {
                    self.opening = false;
                    self.errors.insert(None, format!("Couldn't open a terminal: {e}"));
                }
                Reply::Restarted(_, Ok(terminal)) => {
                    self.list.insert(terminal.id.clone(), terminal);
                }
                Reply::Restarted(id, Err(e)) => {
                    self.errors.insert(Some(id), format!("Couldn't start it again: {e}"));
                }
                Reply::Closed(id, Ok(())) => {
                    self.list.remove(&id);
                }
                Reply::Closed(id, Err(e)) => {
                    self.errors.insert(Some(id), format!("Couldn't close it: {e}"));
                }
            }
        }
    }

    /// A reply arrived (tests: what the server would have answered).
    #[cfg(test)]
    pub(crate) fn answered_open(&mut self, result: NetResult<Terminal>) {
        if let Ok(mut m) = self.mailbox.lock() {
            m.push(Reply::Opened(result));
        }
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{TermSize, TerminalStatus};

    use super::*;

    pub(crate) fn terminal(id: &str) -> Terminal {
        Terminal { id: TerminalId::new(id), title: "bash".into(), status: TerminalStatus::Running, size: TermSize { cols: 80, rows: 24 }, shell: "/bin/bash".into(), folder: "/p".into() }
    }

    #[test]
    fn the_list_follows_the_server_and_one_opened_here_is_told_as_made() {
        let mut terminals = Terminals::default();
        terminals.apply(&ClientEvent::TerminalUpdated { terminal: terminal("t1") });
        terminals.apply(&ClientEvent::TerminalUpdated { terminal: terminal("t2") });
        terminals.apply(&ClientEvent::TerminalClosed { terminal_id: TerminalId::new("t1") });
        assert_eq!(terminals.list().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
        let (net, ctx) = (NetClient::new("http://127.0.0.1:9"), egui::Context::default());
        terminals.act(None, TermAct::Open, &net, &ctx);
        assert!(terminals.opening());
        assert_eq!(net.sent.lock().unwrap().as_slice(), ["POST /api/terminals"]);
        terminals.answered_open(Ok(terminal("t3")));
        terminals.tick();
        assert_eq!(terminals.made(), (1, Some(&TerminalId::new("t3"))));
        assert!(!terminals.opening() && terminals.get(&TerminalId::new("t3")).is_some());
        assert_eq!(terminals.beside(Some(&TerminalId::new("t3")), 1), Some(TerminalId::new("t2")), "stepping wraps");
        terminals.apply(&ClientEvent::Resync);
        assert_eq!(terminals.list().count(), 0, "a resync starts again from what follows");
    }
}
