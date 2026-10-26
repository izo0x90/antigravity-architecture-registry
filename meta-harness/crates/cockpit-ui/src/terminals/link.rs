//! One reader's socket to a terminal: home's ticket first, then the socket
//! (through home to another server). What it's sent comes out in order;
//! what's sent before it's open waits and goes first, in order. A reader
//! that stops reading for long (its buffer hidden) has what piled up
//! dropped and starts again from the whole screen.

use std::sync::{Arc, Mutex};

use harness_contracts::{FromTerminal, TerminalId, ToTerminal};

use crate::net::{NetClient, TicketDoor};

/// More messages than this waiting unread: start again.
const MOST_WAITING: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkState {
    Connecting,
    Open,
    /// It closed (or never opened): open a new one to go on.
    Closed,
}

#[derive(Default)]
struct Inbox {
    events: Vec<ewebsock::WsEvent>,
    /// More than `MOST_WAITING` came unread: they were dropped.
    overflowed: bool,
}

impl Inbox {
    /// One event from the socket (what piled up unread is dropped when too much).
    fn received(&mut self, event: ewebsock::WsEvent) {
        if self.events.len() >= MOST_WAITING {
            self.events.clear();
            self.overflowed = true;
        }
        self.events.push(event);
    }
}

pub struct TermLink {
    url: String,
    door: TicketDoor,
    #[cfg(not(test))]
    sender: Option<ewebsock::WsSender>,
    inbox: Arc<Mutex<Inbox>>,
    state: LinkState,
    /// Sent before it opened, to go once it does.
    waiting: Vec<String>,
    /// Tests: what it sent.
    #[cfg(test)]
    pub(crate) sent: Vec<ToTerminal>,
}

/// What a poll brought.
#[derive(Default)]
pub struct Polled {
    pub said: Vec<FromTerminal>,
    /// It closed, or fell so far behind it must start again.
    pub lost: bool,
}

impl TermLink {
    /// Starts connecting to terminal `id` on `net`'s server.
    pub fn open(net: &NetClient, id: &TerminalId, ctx: &egui::Context) -> Self {
        let url = net.socket_url(&format!("/ws/terminals/{}", urlencoding::encode(id.as_str())));
        let mut link = Self {
            url,
            door: TicketDoor::default(),
            #[cfg(not(test))]
            sender: None,
            inbox: Arc::default(),
            state: LinkState::Connecting,
            waiting: Vec::new(),
            #[cfg(test)]
            sent: Vec::new(),
        };
        if let Some(done) = link.door.start() {
            net.home_ticket(ctx.clone(), done);
        }
        link
    }

    pub fn state(&self) -> LinkState {
        self.state
    }

    /// Sends `said` (once it's open, if it isn't yet).
    pub fn send(&mut self, said: &ToTerminal) {
        #[cfg(test)]
        self.sent.push(said.clone());
        let Ok(text) = serde_json::to_string(said) else { return };
        match self.state {
            LinkState::Open => self.send_now(text),
            LinkState::Connecting => self.waiting.push(text),
            LinkState::Closed => {}
        }
    }

    #[cfg(not(test))]
    fn send_now(&mut self, text: String) {
        if let Some(sender) = &mut self.sender {
            sender.send(ewebsock::WsMessage::Text(text));
        }
    }

    #[cfg(test)]
    fn send_now(&mut self, _text: String) {}

    /// What it was sent since last asked; opens the socket once the
    /// ticket's in.
    pub fn poll(&mut self, ctx: &egui::Context) -> Polled {
        match self.door.take() {
            Some(Ok(ticket)) => self.connect(&ticket, ctx),
            Some(Err(_)) => self.state = LinkState::Closed,
            None => {}
        }
        let (events, overflowed) = match self.inbox.lock() {
            Ok(mut inbox) => (std::mem::take(&mut inbox.events), std::mem::take(&mut inbox.overflowed)),
            Err(_) => (Vec::new(), false),
        };
        let mut polled = Polled { lost: overflowed, ..Polled::default() };
        for event in events {
            match event {
                ewebsock::WsEvent::Opened => {
                    self.state = LinkState::Open;
                    for text in std::mem::take(&mut self.waiting) {
                        self.send_now(text);
                    }
                }
                ewebsock::WsEvent::Message(ewebsock::WsMessage::Text(text)) => match serde_json::from_str::<FromTerminal>(&text) {
                    Ok(said) => polled.said.push(said),
                    Err(e) => log::warn!("A terminal said something unreadable: {e}"),
                },
                ewebsock::WsEvent::Closed | ewebsock::WsEvent::Error(_) => {
                    self.state = LinkState::Closed;
                    polled.lost = true;
                }
                _ => {}
            }
        }
        polled
    }

    #[cfg(not(test))]
    fn connect(&mut self, ticket: &str, ctx: &egui::Context) {
        let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
        let handler = Box::new(move |event: ewebsock::WsEvent| {
            if let Ok(mut inbox) = inbox.lock() {
                inbox.received(event);
            }
            ctx.request_repaint();
            std::ops::ControlFlow::Continue(())
        });
        match ewebsock::ws_connect(format!("{}?ticket={ticket}", self.url), ewebsock::Options::default(), handler) {
            Ok(sender) => self.sender = Some(sender),
            Err(_) => self.state = LinkState::Closed,
        }
    }

    #[cfg(test)]
    fn connect(&mut self, _ticket: &str, _ctx: &egui::Context) {}

    /// Tests: the socket opened, and the server said `said`.
    #[cfg(test)]
    pub(crate) fn arrive(&mut self, said: Vec<FromTerminal>) {
        let mut inbox = self.inbox.lock().unwrap();
        if self.state == LinkState::Connecting {
            inbox.received(ewebsock::WsEvent::Opened);
        }
        for s in said {
            inbox.received(ewebsock::WsEvent::Message(ewebsock::WsMessage::Text(serde_json::to_string(&s).unwrap())));
        }
    }

    /// Tests: where it connects.
    #[cfg(test)]
    pub(crate) fn url(&self) -> &str {
        &self.url
    }
}

#[cfg(test)]
mod tests {
    use harness_contracts::{Mods, ScreenUpdate, TermKey, TermSize, TerminalInput};

    use super::*;

    fn screen(seq: u64) -> FromTerminal {
        FromTerminal::Screen {
            update: ScreenUpdate {
                seq,
                size: TermSize { cols: 80, rows: 24 },
                full: false,
                rows: Vec::new(),
                cursor: Default::default(),
                top: 0,
                oldest: 0,
                alternate: false,
                title: String::new(),
                shell: 1,
                commands: Vec::new(),
            },
        }
    }

    #[test]
    fn a_terminal_on_another_server_is_reached_through_home_with_homes_ticket() {
        let net = NetClient::for_server("http://home:8080", "w1");
        let link = TermLink::open(&net, &TerminalId::new("t1"), &egui::Context::default());
        assert_eq!(link.url(), "ws://home:8080/s/w1/ws/terminals/t1");
        assert_eq!(net.sent.lock().unwrap().as_slice(), ["POST /api/cockpit/ticket"], "home's ticket, not the server's");
    }

    #[test]
    fn what_is_sent_before_it_opens_goes_once_it_does_in_order_and_what_comes_is_read_in_order() {
        let (net, ctx) = (NetClient::new("http://home:8080"), egui::Context::default());
        let mut link = TermLink::open(&net, &TerminalId::new("t1"), &ctx);
        let key = |k| ToTerminal::Input { input: TerminalInput::Key { key: k, mods: Mods::default() } };
        link.send(&key(TermKey::Up));
        link.send(&key(TermKey::Enter));
        assert_eq!((link.state(), link.waiting.len()), (LinkState::Connecting, 2), "kept until it opens");
        link.arrive(vec![screen(1), screen(2)]);
        let polled = link.poll(&ctx);
        assert_eq!(link.state(), LinkState::Open);
        assert!(link.waiting.is_empty(), "sent once it opened");
        assert_eq!(link.sent, [key(TermKey::Up), key(TermKey::Enter)]);
        assert_eq!(polled.said, [screen(1), screen(2)]);
        assert!(!polled.lost);
    }

    #[test]
    fn a_reader_that_fell_far_behind_starts_again() {
        let (net, ctx) = (NetClient::new("http://home:8080"), egui::Context::default());
        let mut link = TermLink::open(&net, &TerminalId::new("t1"), &ctx);
        link.arrive((0..MOST_WAITING as u64 + 5).map(screen).collect());
        let polled = link.poll(&ctx);
        assert!(polled.lost, "what piled up was dropped: the whole screen is asked for again");
        assert!(polled.said.len() < 10);
    }
}
