//! The cockpit, driven from outside: what can be run, running it, reading
//! what's on screen, and clicks, typing and keys as the user would give
//! them. One set of calls with two ways in: the page's `window.nah` (scripts
//! in the page, a browser driver) and the server's cockpit socket (its MCP
//! tools, for agents; open only while the user lets agents drive). Calls
//! run inside a frame, on the paths keys and clicks take, so nothing here
//! is a second way of doing what the cockpit does.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One call.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Call {
    /// What's on screen: the mode, the panels, and every piece of text
    /// drawn, where.
    Read,
    /// What can be run now: ids, labels, keys and what each needs.
    Actions,
    /// Runs an action by id, with its value if it takes one.
    Run {
        id: String,
        #[serde(default)]
        value: Option<String>,
    },
    /// Clicks drawn text (the topmost match: exact, else containing it), or
    /// a point.
    Click {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        x: Option<f32>,
        #[serde(default)]
        y: Option<f32>,
    },
    /// Types text into whatever has the keyboard.
    Type { text: String },
    /// Presses keys in order, as `?` names them: `space p a`,
    /// `ctrl+enter`, `escape`, `j`.
    Keys { keys: String },
}

pub type Answer = Result<Value, String>;
pub type Responder = Box<dyn FnOnce(Answer)>;

/// Calls waiting for the next frame (both ways in put them here).
#[derive(Clone, Default)]
pub struct Door {
    inbox: Rc<RefCell<VecDeque<(Call, Responder)>>>,
    ctx: Option<egui::Context>,
}

impl Door {
    /// Wakes the cockpit for calls through it.
    pub fn wake(&mut self, ctx: &egui::Context) -> Self {
        self.ctx = Some(ctx.clone());
        self.clone()
    }

    /// The calls come through it since last taken.
    pub fn take(&self) -> Vec<(Call, Responder)> {
        self.inbox.borrow_mut().drain(..).collect()
    }

    /// Takes a call as JSON; the answer goes to `respond`.
    pub fn knock(&self, call: &str, respond: Responder) {
        match serde_json::from_str::<Call>(call) {
            Ok(call) => {
                self.inbox.borrow_mut().push_back((call, respond));
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint();
                }
            }
            Err(e) => respond(Err(format!("not a call: {e}"))),
        }
    }
}

/// A piece of text drawn this frame, where (screen points).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Drawn {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Drawn {
    fn center(&self) -> egui::Pos2 {
        egui::pos2(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Input to feed in, one frame's worth.
struct Feed {
    events: Vec<egui::Event>,
    modifiers: egui::Modifiers,
}

/// Where calls come in, and what they're waiting on.
pub struct Remote {
    door: Door,
    /// Input still to feed, a frame at a time.
    feed: VecDeque<Feed>,
    /// Answered once the input before them is all in and drawn.
    after_feed: Vec<(Responder, Value)>,
    /// The input ran out this frame.
    fed_out: bool,
    /// Answered at the end of this frame.
    end_of_frame: Vec<(Responder, Pending)>,
    /// The server's way in, while agents may drive.
    socket: Option<Socket>,
    /// The user lets agents drive the cockpit.
    pub agents: bool,
    socket_url: String,
    retry_at: f64,
    /// The socket opens with a one-time ticket, asked for first.
    ticket_door: crate::net::TicketDoor,
}

/// What an end-of-frame answer reports.
enum Pending {
    Screen,
    Ran,
    /// Clicks the drawn text.
    Click(String),
}

/// The app's state as a read reports it.
#[derive(Default)]
pub struct Status {
    pub mode: String,
    pub panels: Vec<Value>,
    pub note: Option<String>,
    pub leader: Option<String>,
    pub palette: bool,
}

impl Remote {
    /// Server `name`'s way in (through home, at `host_origin`).
    pub fn new(host_origin: &str, name: &str) -> Self {
        let base = host_origin.trim_end_matches('/');
        let through = if name == harness_contracts::home::HERE { String::new() } else { format!("/s/{name}") };
        let socket_url = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1) + &through + "/ws/cockpit";
        Self {
            door: Door::default(),
            feed: VecDeque::new(),
            after_feed: Vec::new(),
            fed_out: false,
            end_of_frame: Vec::new(),
            socket: None,
            agents: false,
            socket_url,
            retry_at: 0.0,
            ticket_door: crate::net::TicketDoor::default(),
        }
    }

    /// The calls come in since last frame (and the socket kept open or
    /// closed as the user says).
    pub fn take(&mut self, ctx: &egui::Context, net: &crate::net::NetClient) -> Vec<(Call, Responder)> {
        self.door.ctx.get_or_insert_with(|| ctx.clone());
        self.keep_socket(ctx, net);
        self.door.inbox.borrow_mut().drain(..).collect()
    }

    /// What's still to feed in and to answer moves to `to` (the desk shown
    /// now: the screen those answers are about is its).
    pub fn hand_over(&mut self, to: &mut Remote) {
        to.feed.extend(self.feed.drain(..));
        to.after_feed.append(&mut self.after_feed);
        to.end_of_frame.append(&mut self.end_of_frame);
    }

    /// Feeds the next frame's input in (from `raw_input_hook`).
    pub fn feed(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if let Some(Feed { events, modifiers }) = self.feed.pop_front() {
            raw.modifiers = modifiers;
            raw.events.extend(events);
            self.fed_out = self.feed.is_empty();
            ctx.request_repaint();
        }
    }

    /// Something waits for this frame to be drawn (and the app's state).
    pub fn waiting(&self) -> bool {
        !self.end_of_frame.is_empty()
    }

    /// Answers once this frame is drawn: the screen, or what running an action left.
    pub fn answer_screen(&mut self, respond: Responder) {
        self.end_of_frame.push((respond, Pending::Screen));
    }

    pub fn answer_ran(&mut self, respond: Responder) {
        self.end_of_frame.push((respond, Pending::Ran));
    }

    /// Clicks drawn text (found once this frame is drawn) or a point.
    pub fn click(&mut self, text: Option<String>, at: Option<egui::Pos2>, respond: Responder) {
        match (text, at) {
            (_, Some(at)) => self.click_at(at, None, respond),
            (Some(text), None) => self.end_of_frame.push((respond, Pending::Click(text))),
            (None, None) => respond(Err("click what? give `text`, or `x` and `y`".into())),
        }
    }

    fn click_at(&mut self, at: egui::Pos2, clicked: Option<String>, respond: Responder) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.queue(vec![egui::Event::PointerMoved(at)], egui::Modifiers::NONE);
        self.queue(vec![button(true)], egui::Modifiers::NONE);
        self.queue(vec![button(false)], egui::Modifiers::NONE);
        self.after_feed.push((respond, json!({ "clicked": clicked, "x": at.x, "y": at.y })));
    }

    pub fn type_text(&mut self, text: String, respond: Responder) {
        self.queue(vec![egui::Event::Text(text)], egui::Modifiers::NONE);
        self.after_feed.push((respond, json!({ "typed": true })));
    }

    pub fn keys(&mut self, keys: &str, respond: Responder) {
        let parsed: Result<Vec<_>, _> = keys.split_whitespace().map(key_press).collect();
        match parsed {
            Ok(presses) if !presses.is_empty() => {
                for chord in presses {
                    let (modifiers, down, up) = chord.press();
                    self.queue(down, modifiers);
                    self.queue(up, egui::Modifiers::NONE);
                }
                self.after_feed.push((respond, json!({ "pressed": keys })));
            }
            Ok(_) => respond(Err("no keys given".into())),
            Err(e) => respond(Err(e)),
        }
    }

    fn queue(&mut self, events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        self.feed.push_back(Feed { events, modifiers });
        self.fed_out = false;
    }

    /// After drawing: answers what waited for this frame.
    pub fn end_frame(&mut self, ctx: &egui::Context, status: Option<Status>) {
        if self.fed_out {
            self.fed_out = false;
            for (respond, value) in self.after_feed.drain(..) {
                respond(Ok(value));
            }
        }
        if self.end_of_frame.is_empty() {
            return;
        }
        // Only walked when asked.
        let drawn = drawn_texts(ctx);
        let status = status.unwrap_or_default();
        for (respond, pending) in std::mem::take(&mut self.end_of_frame) {
            match pending {
                Pending::Screen => respond(Ok(json!({
                    "mode": status.mode,
                    "panels": status.panels,
                    "note": status.note,
                    "leader": status.leader,
                    "palette": status.palette,
                    "texts": drawn,
                }))),
                Pending::Ran => respond(Ok(json!({ "note": status.note }))),
                Pending::Click(text) => match find(&drawn, &text) {
                    Some(d) => self.click_at(d.center(), Some(d.text.clone()), respond),
                    None => respond(Err(format!("{text:?} isn't on screen (read shows what is)"))),
                },
            }
        }
    }

    /// Opens the server's way in while agents may drive (with a ticket
    /// asked for first, from home: `home`; a dropped one again now and
    /// then), closes it when not; takes the calls that came.
    fn keep_socket(&mut self, ctx: &egui::Context, net: &crate::net::NetClient) {
        if !self.agents {
            self.socket = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        if self.socket.as_ref().is_some_and(Socket::closed) {
            self.socket = None;
            self.retry_at = now + RETRY_S;
        }
        if let Some(ticket) = self.ticket_door.take() {
            self.retry_at = now + RETRY_S;
            match ticket {
                Ok(ticket) => self.socket = Socket::open(&format!("{}?ticket={ticket}", self.socket_url), ctx),
                Err(e) => log::warn!("cockpit socket: no ticket: {}", e.message),
            }
        }
        if self.socket.is_none()
            && now >= self.retry_at
            && let Some(done) = self.ticket_door.start()
        {
            net.cockpit_ticket(ctx.clone(), done);
        }
        match &self.socket {
            Some(socket) => socket.take_calls(&self.door),
            None if !self.ticket_door.asking() => {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((self.retry_at - now).max(0.1)))
            }
            None => {}
        }
    }

    /// The socket is open (agents can drive now).
    pub fn connected(&self) -> bool {
        self.socket.as_ref().is_some_and(|s| s.open.get())
    }
}

const RETRY_S: f64 = 3.0;

/// The topmost drawn text that is `text`, else the topmost containing it.
fn find<'a>(drawn: &'a [Drawn], text: &str) -> Option<&'a Drawn> {
    let text = text.trim();
    drawn.iter().rev().find(|d| d.text.trim() == text).or_else(|| drawn.iter().rev().find(|d| d.text.contains(text)))
}

/// A key as an agent names it, in the keymap's own grammar (`J`, `+`,
/// `ctrl+o`, `Space`, `Enter`); a few short names agents use are taken
/// too (`esc`, `return`, `bs`, `pgup`, `pgdn`, any case).
fn key_press(name: &str) -> Result<crate::keymap::Chord, String> {
    let (mods, key) = match name.strip_suffix("++") {
        Some(mods) => (format!("{mods}+"), "+"),
        None => match name.rsplit_once('+') {
            Some((mods, key)) if !key.is_empty() => (format!("{mods}+"), key),
            _ => (String::new(), name),
        },
    };
    let key = match key.to_ascii_lowercase().as_str() {
        "esc" => "Escape".to_owned(),
        "return" => "Enter".to_owned(),
        "bs" => "Backspace".to_owned(),
        "pgup" => "PageUp".to_owned(),
        "pgdn" => "PageDown".to_owned(),
        lower if key.chars().count() > 1 => lower
            .split('_')
            .map(|w| {
                let mut chars = w.chars();
                chars.next().map(|c| c.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
            })
            .collect(),
        _ => key.to_owned(),
    };
    crate::keymap::Chord::try_from(format!("{mods}{key}")).map_err(|e| format!("{name:?}: {e}"))
}

/// Every piece of text drawn this frame and visible, back to front.
pub fn drawn_texts(ctx: &egui::Context) -> Vec<Drawn> {
    fn collect(shape: &egui::Shape, clip: egui::Rect, to_global: Option<egui::emath::TSTransform>, out: &mut Vec<Drawn>) {
        match shape {
            egui::Shape::Text(t) => {
                // Icon glyphs (the icon font's private-use characters) say nothing as text.
                let text: String = t.galley.text().chars().filter(|c| !('\u{e000}'..='\u{f8ff}').contains(c)).collect();
                let text = text.trim();
                let mut rect = egui::Rect::from_min_size(t.pos, t.galley.size());
                if let Some(to) = to_global {
                    rect = to.mul_rect(rect);
                }
                if !text.is_empty() && rect.intersects(clip) {
                    let r = rect.intersect(clip);
                    out.push(Drawn {
                        text: text.to_owned(),
                        x: r.min.x.round(),
                        y: r.min.y.round(),
                        w: r.width().round(),
                        h: r.height().round(),
                    });
                }
            }
            egui::Shape::Vec(list) => list.iter().for_each(|s| collect(s, clip, to_global, out)),
            _ => {}
        }
    }
    let screen = ctx.content_rect();
    let layers: Vec<egui::LayerId> = ctx.memory(|m| m.layer_ids().collect());
    let mut out = Vec::new();
    for layer in layers {
        let to_global = ctx.layer_transform_to_global(layer);
        let shapes: Vec<egui::epaint::ClippedShape> =
            ctx.graphics(|g| g.get(layer).map(|list| list.all_entries().cloned().collect()).unwrap_or_default());
        for clipped in &shapes {
            let clip = to_global.map_or(clipped.clip_rect, |to| to.mul_rect(clipped.clip_rect)).intersect(screen);
            collect(&clipped.shape, clip, to_global, &mut out);
        }
    }
    out
}

/// The server's way in: calls as `{"id", "call"}`, answers as `{"id", "ok"}`
/// or `{"id", "error"}`.
pub struct Socket {
    sender: RefCell<ewebsock::WsSender>,
    receiver: ewebsock::WsReceiver,
    open: Rc<std::cell::Cell<bool>>,
    gone: std::cell::Cell<bool>,
    /// Answers go out through here (calls answer later, from a frame).
    outbox: Rc<RefCell<Vec<String>>>,
}

#[derive(Deserialize)]
struct Incoming {
    id: u64,
    call: Value,
}

impl Socket {
    fn open(url: &str, ctx: &egui::Context) -> Option<Self> {
        let ctx = ctx.clone();
        let (sender, receiver) = ewebsock::connect_with_wakeup(url, ewebsock::Options::default(), move || ctx.request_repaint()).ok()?;
        Some(Self { sender: RefCell::new(sender), receiver, open: Rc::default(), gone: Default::default(), outbox: Rc::default() })
    }

    fn closed(&self) -> bool {
        self.gone.get()
    }

    /// Hands the calls that came to `door`; sends the answers ready.
    fn take_calls(&self, door: &Door) {
        while let Some(event) = self.receiver.try_recv() {
            match event {
                ewebsock::WsEvent::Opened => self.open.set(true),
                ewebsock::WsEvent::Message(ewebsock::WsMessage::Text(text)) => match serde_json::from_str::<Incoming>(&text) {
                    Ok(Incoming { id, call }) => {
                        let outbox = self.outbox.clone();
                        let answer = move |answer: Answer| {
                            let reply = match answer {
                                Ok(ok) => json!({ "id": id, "ok": ok }),
                                Err(error) => json!({ "id": id, "error": error }),
                            };
                            outbox.borrow_mut().push(reply.to_string());
                        };
                        door.knock(&call.to_string(), Box::new(answer));
                    }
                    Err(e) => log::warn!("cockpit socket: {e}"),
                },
                ewebsock::WsEvent::Message(_) => {}
                ewebsock::WsEvent::Error(_) | ewebsock::WsEvent::Closed => {
                    self.open.set(false);
                    self.gone.set(true);
                }
            }
        }
        self.flush();
    }

    fn flush(&self) {
        for reply in self.outbox.borrow_mut().drain(..) {
            self.sender.borrow_mut().send(ewebsock::WsMessage::Text(reply));
        }
    }
}

impl Remote {
    /// Sends the answers given this frame.
    pub fn flush(&self) {
        if let Some(socket) = &self.socket {
            socket.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_as_the_action_list_names_them() {
        // The press a key name makes: its key with its modifiers, the text it types.
        let press = |name: &str| {
            let (mods, down, _) = key_press(name).unwrap().press();
            let key = down.iter().find_map(|e| if let egui::Event::Key { key, .. } = e { Some(*key) } else { None });
            let text = down.iter().find_map(|e| if let egui::Event::Text(t) = e { Some(t.clone()) } else { None });
            (key, mods, text)
        };
        let (key, mods, text) = press("ctrl+enter");
        assert_eq!((key, mods.command, text), (Some(egui::Key::Enter), true, None));
        assert_eq!(press("space").2.as_deref(), Some(" "), "space types a space, as a browser gives it");
        let (key, mods, text) = press("G");
        assert_eq!((key, mods.shift, text.as_deref()), (Some(egui::Key::G), true, Some("G")));
        assert_eq!(press("+").2.as_deref(), Some("+"), "the keymap's own names: + types a plus");
        assert_eq!(press("esc").0, Some(egui::Key::Escape));
        assert_eq!(press("page_down").0, Some(egui::Key::PageDown));
        assert!(key_press("hyper+x").is_err());
        assert!(key_press("nonsense").is_err());
    }

    #[test]
    fn a_click_finds_the_topmost_exact_text_first() {
        let at = |text: &str, y| Drawn { text: text.into(), x: 0.0, y, w: 10.0, h: 10.0 };
        let drawn = [at("Connect", 0.0), at("Connect now", 20.0), at("Connect", 40.0)];
        assert_eq!(find(&drawn, "Connect").unwrap().y, 40.0);
        assert_eq!(find(&drawn, "now").unwrap().y, 20.0);
        assert!(find(&drawn, "Send").is_none());
    }

    #[test]
    fn a_call_that_isnt_one_is_answered_so() {
        let door = Door { inbox: Rc::default(), ctx: None };
        let answer = Rc::new(RefCell::new(None));
        let kept = answer.clone();
        door.knock(r#"{"op":"dance"}"#, Box::new(move |a| *kept.borrow_mut() = Some(a)));
        assert!(matches!(answer.borrow().as_ref(), Some(Err(e)) if e.starts_with("not a call")));
        door.knock(r#"{"op":"run","id":"chat.send"}"#, Box::new(|_| {}));
        assert_eq!(door.inbox.borrow().front().map(|(c, _)| c.clone()), Some(Call::Run { id: "chat.send".into(), value: None }));
    }
}
