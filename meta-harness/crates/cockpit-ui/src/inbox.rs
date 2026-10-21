//! The inbox: what needs the user and what happened (the open
//! notifications), one at a time. A view like the others: the cursor moves
//! over its parts (what it says, its request, what the agent said last);
//! on its request, the request's answers show under it (its actions, with
//! their keys); Enter opens its chat (a discussion waiting on you: the
//! discussion). It shows in a panel, or in the quick look over
//! one. What it asks for beyond itself (the next item on another server,
//! its chat, putting it away, an answer) it hands the shell as an ask.

use egui::{RichText, Vec2};
use egui_phosphor::regular as icon;
use harness_contracts::{Level, Notification, NotificationId, RuntimeRequest};

use crate::actions::{Action, ActionProvider, Group, MINOR, PRIMARY, RunCx, SECONDARY, Style};
use crate::agents::Act;
use crate::request_card::{Drafts, render_request};
use crate::kind::BufferKind;
use crate::nav::{Ask, Fold, Item, Motion, Move, Navigable, Place, Spot, Step, Target, Waiting};
use crate::shared::Shared;
use crate::hosted::{Status, Tone};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// The item shown, as last read from the store.
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub notification: Notification,
    /// Where it's from: `slot · kind · age · harness`.
    pub from: String,
    /// Its request, while the store has it.
    pub request: Option<RuntimeRequest>,
    /// What the agent said last.
    pub said: Option<String>,
}

/// A part of the item, where the cursor can be.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Says,
    /// Its request (its answers show under it).
    Request,
    Said,
}

#[derive(Default)]
pub struct InboxView {
    /// The item shown; none: the first open one.
    pub item: Option<NotificationId>,
    pub shown: Option<Shown>,
    /// Where it is among every server's open ones (1-based), and how many.
    pub at: Option<(usize, usize)>,
    cursor: usize,
    /// Where it's looking (the shared rules move it, a part a stop).
    scrolling: Measured,
    spots: Vec<Spot>,
    asks: Vec<Ask>,
    /// Answers being given to its request's questions.
    drafts: Drafts,
}

#[derive(Clone, Debug)]
enum Command {
    Step(i32),
    OpenChat,
    Dismiss,
    Answer(String),
    OpenPage(String),
}

impl InboxView {
    /// Shows item `id`, on what waits on you (its request) if it asks.
    pub fn show(&mut self, id: NotificationId) {
        if self.item.as_ref() != Some(&id) {
            self.cursor = usize::MAX;
            self.scrolling = Measured::default();
        }
        self.item = Some(id);
    }

    /// Reads what it shows from the desk's agents (each frame: it's a view
    /// of them), and where it is among every server's open ones.
    fn sync(&mut self, shared: &Shared) {
        let agents = shared.agents.get();
        let store = agents.store();
        let now_ms = crate::attention::now_ms();
        if self.item.is_none() {
            self.item = crate::attention::list(store, None).first().map(|n| n.id.clone());
            self.cursor = usize::MAX;
        }
        self.shown = self.item.as_ref().and_then(|id| store.notifications.get(id)).map(|n| {
            let harness = n.slot.as_ref().and_then(|s| store.slots.get(s)).and_then(|s| s.instance_id.as_ref()).and_then(|i| store.instances.get(i)).map(|i| i.display_name.clone());
            let from = [Some(crate::attention::detail(n, store, now_ms)), harness].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            let request = n.thread.as_ref().zip(n.request.as_ref()).and_then(|(t, r)| store.threads.get(t)?.request(r).cloned());
            let said = n.thread.as_ref().and_then(|t| agents.last_said(t)).filter(|s| n.detail.as_deref() != Some(s.as_str()));
            Shown { notification: n.clone(), from, request, said }
        });
        let open = shared.open.get();
        self.at = self.item.as_ref().and_then(|id| open.iter().position(|(s, n)| *s == shared.server && n == id).map(|i| (i + 1, open.len())));
        let parts = self.parts();
        if self.cursor == usize::MAX {
            self.cursor = parts.iter().position(|p| *p == Part::Request).unwrap_or(0);
        }
        self.cursor = self.cursor.min(parts.len().saturating_sub(1));
    }

    fn parts(&self) -> Vec<Part> {
        let Some(shown) = &self.shown else { return Vec::new() };
        let request = shown.request.as_ref().map(|_| Part::Request);
        std::iter::once(Part::Says).chain(request).chain(shown.said.as_ref().map(|_| Part::Said)).collect()
    }

    fn put_cursor(&mut self, at: usize, show: Show) {
        self.cursor = at.min(self.parts().len().saturating_sub(1));
        self.scrolling.show(self.cursor, show);
    }

    /// Moves by the shared rules.
    fn go(&mut self, go: Go) {
        let n = self.parts().len();
        let from = (self.cursor < n).then_some(self.cursor);
        if let Some(at) = self.scrolling.go(n, from, go) {
            self.cursor = at;
        }
    }

    /// Answers its request with `option` (and what was picked and typed for
    /// its questions).
    fn answer(&mut self, option: String) -> Step {
        let Some(shown) = &self.shown else { return Step::Nothing("Nothing to answer: it was answered already.".into()) };
        let Some(request) = &shown.request else { return Step::Nothing("Nothing to answer: it was answered already.".into()) };
        let says_yes = request.options.iter().find(|o| o.id == option).is_some_and(|o| matches!(o.decision, harness_contracts::ApprovalDecision::Accept | harness_contracts::ApprovalDecision::AcceptForSession | harness_contracts::ApprovalDecision::AcceptAlways));
        if says_yes && !self.drafts.complete(request) {
            return Step::Nothing("Answer the questions it needs first.".into());
        }
        let act = Act::Answer { thread: request.thread_id.clone(), request: request.id.clone(), response: self.drafts.response(request, &option) };
        let slot = shown.notification.slot.clone().unwrap_or_else(|| harness_contracts::SlotId::new(harness_contracts::LEAD));
        self.asks.push(Ask::Agents { slot, act });
        Step::Done
    }

    /// Opens what it's about: its agent's chat, or its terminal.
    fn open_chat(&mut self) -> Step {
        let Some(n) = self.shown.as_ref().map(|s| &s.notification) else { return Step::Nothing("Nothing needs you.".into()) };
        let (target, in_) = match (&n.slot, &n.terminal, &n.discussion) {
            // A discussion: it opens where discussions open (over what you're on).
            (_, _, Some(discussion)) => (Target::Discussion(discussion.clone()), None),
            (Some(slot), _, None) => (Target::Agent { slot: Some(slot.clone()), thread: n.thread.clone() }, Some(BufferKind::Chat)),
            (None, Some(terminal), None) => (Target::Terminal(terminal.clone()), Some(BufferKind::Terminal)),
            (None, None, None) => return Step::Nothing("It's from no agent's chat, terminal or discussion.".into()),
        };
        self.asks.push(Ask::Show { target, in_ });
        Step::Done
    }

    fn draw(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>) {
        let theme = crate::theme::look();
        let Some(shown) = self.shown.clone() else {
            if self.item.is_some() {
                ui.label(RichText::new("Already handled: the next one is a key away.").size(theme.text_body).color(colors().muted));
            }
            self.spots.clear();
            return;
        };
        let parts = self.parts();
        let mut drawn = Vec::new();
        let cursor = self.cursor;
        let mark = |ui: &mut egui::Ui, rect: egui::Rect, index: usize, drawn: &mut Vec<Spot>| {
            drawn.push(Spot { key: index.to_string(), rect });
            if index == cursor {
                ui.painter().rect_stroke(rect.expand(2.0), theme.radius_m, egui::Stroke::new(theme.line_thin, colors().focus), egui::StrokeKind::Outside);
            }
        };
        let area = self.scrolling.area(egui::ScrollArea::vertical().id_salt(("inbox", cx.at)).auto_shrink([false, true]), ui.input(|i| i.time));
        let output = area.show(ui, |ui| {
            let says = ui
                .vertical(|ui| {
                    ui.label(RichText::new(&shown.from).size(theme.text_small).color(colors().muted));
                    ui.label(RichText::new(&shown.notification.title).strong().size(theme.text_title).color(colors().text_strong));
                    if let Some(detail) = &shown.notification.detail {
                        ui.add(egui::Label::new(RichText::new(detail).size(theme.text_body)).wrap());
                    }
                })
                .response
                .rect;
            mark(ui, says, 0, &mut drawn);
            if shown.notification.thread.is_some() && shown.notification.request.is_some() {
                ui.add_space(theme.space_sm);
                match &shown.request {
                    Some(request) => {
                        let card = render_request(ui, request, &mut self.drafts);
                        if let Some(index) = parts.iter().position(|p| *p == Part::Request) {
                            mark(ui, card, index, &mut drawn);
                        }
                    }
                    None => {
                        ui.label(RichText::new("Its request is gone (its session ended).").size(theme.text_body).color(colors().muted));
                    }
                }
            }
            if let Some(said) = &shown.said {
                ui.add_space(theme.space_sm);
                let rect = ui
                    .vertical(|ui| {
                        ui.label(RichText::new("It said last").size(theme.text_small).color(colors().muted));
                        let said: String = said.chars().take(1200).collect();
                        ui.add(egui::Label::new(RichText::new(said).size(theme.text_body)).wrap());
                    })
                    .response
                    .rect;
                mark(ui, rect, parts.len() - 1, &mut drawn);
            }
            ui.allocate_space(Vec2::ZERO);
        });
        // Each part's stop, in order (all of them drawn, or none counted).
        if drawn.len() == parts.len() {
            for spot in &drawn {
                self.scrolling.item(spot.rect);
            }
        }
        self.scrolling.drawn(&output);
        if self.scrolling.busy() {
            ui.ctx().request_repaint();
        }
        self.spots = drawn;
    }
}

impl crate::buffer::Buffer for InboxView {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, _drawing: &mut crate::buffer::Drawing<'_>) {
        self.draw(ui, cx);
    }

    fn describe(&self) -> String {
        "what needs you".to_owned()
    }
}

impl Navigable for InboxView {
    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    fn share(&mut self, shared: &Shared) {
        self.sync(shared);
    }

    /// The item it shows.
    fn place(&self) -> Place {
        Place::of(&self.item)
    }

    fn place_in_words(&self, place: &Place) -> String {
        place.read::<Option<NotificationId>>().flatten().map(|id| id.to_string()).unwrap_or_default()
    }

    fn restore(&mut self, place: &Place) {
        if let Some(Some(id)) = place.read::<Option<NotificationId>>() {
            self.show(id);
        }
    }

    /// Any item.
    fn can_show(&self, target: &Target) -> bool {
        matches!(target, Target::Notification(_))
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let Target::Notification(id) = target else { return Err("Not something that needs you.".into()) };
        self.show(id.clone());
        Ok(())
    }

    fn target(&self) -> Option<Target> {
        self.item.clone().map(Target::Notification)
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        let n = self.parts().len();
        if n == 0 {
            return;
        }
        match motion {
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(i) => self.put_cursor(i.saturating_sub(1), Show::Middle),
            Motion::NextSibling | Motion::PrevSibling | Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// Opens its chat.
    fn enter(&mut self) -> Step {
        match self.parts().get(self.cursor) {
            Some(_) => self.open_chat(),
            None => Step::Nothing("Nothing needs you.".into()),
        }
    }

    fn leave(&mut self) -> Step {
        Step::Nothing("The inbox has nothing to go out of.".into())
    }

    fn fold(&mut self, _fold: Fold) {}

    fn items(&self) -> Vec<Item> {
        let Some(shown) = &self.shown else { return Vec::new() };
        self.parts()
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let text = match part {
                    Part::Says => format!("{} {}", shown.notification.title, shown.notification.detail.clone().unwrap_or_default()),
                    Part::Request => shown.request.as_ref().map(|r| r.title.clone()).unwrap_or_default(),
                    Part::Said => shown.said.clone().unwrap_or_default(),
                };
                Item::new(i.to_string(), "", text)
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.shown.as_ref().map(|_| self.cursor.to_string())
    }

    fn move_to(&mut self, key: &str) {
        if let Ok(at) = key.parse::<usize>() {
            self.put_cursor(at, Show::Middle);
        }
    }
}

impl InboxView {
    /// The shared moves where the cursor is, in the inbox's words: through
    /// an item's parts, and opening its chat from one.
    fn moves(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        let here = self.parts().get(self.cursor).is_some();
        let n = self.shown.as_ref().map(|s| &s.notification);
        out.push(match n {
            Some(n) if n.discussion.is_some() => Move::Enter.offer_if(here, "Open the discussion"),
            _ => Move::Enter.offer_if(here && n.is_some_and(|n| n.slot.is_some()), "Open its chat"),
        });
        out
    }
}

impl ActionProvider for InboxView {
    fn actions(&self) -> Vec<Action> {
        let act = |id: &str, label: &str, detail: &str, c: Command| Action::new(id, label, detail, c).focused().group(Group::Act);
        let mut out = self.moves();
        if self.at.is_some_and(|(_, of)| of > 1) || self.shown.is_none() && self.item.is_some() {
            out.push(act("inbox.prev", "Previous", "The item before, in what waits on you", Command::Step(-1)).importance(SECONDARY).header(Style::Button, "", Some(icon::CARET_LEFT)));
            out.push(act("inbox.next", "Next", "The next item, in what waits on you", Command::Step(1)).importance(PRIMARY).header(Style::Button, "", Some(icon::CARET_RIGHT)));
        }
        let Some(shown) = &self.shown else { return out };
        // Under its request while you're on it.
        let here = self.parts().get(self.cursor) == Some(&Part::Request);
        if let Some((request, url)) = shown.request.as_ref().and_then(|r| crate::request_card::page(r).map(|u| (r, u))) {
            out.push(crate::request_card::place_page(act(&format!("request.{}.page", request.id), &format!("Open the page: {}", request.title), "The page the agent asks you to open, in a new tab", Command::OpenPage(url)), here));
        }
        for answer in shown.request.iter().flat_map(crate::request_card::answers) {
            out.push(answer.place(act(&answer.id, &answer.label, "Answers the agent's request this way", Command::Answer(answer.option.clone())).importance(PRIMARY), here));
        }
        if shown.notification.discussion.is_some() {
            out.push(act("inbox.open_chat", "Open the discussion", "The discussion that waits on you", Command::OpenChat).importance(PRIMARY).header(Style::Button, "Open it", Some(icon::CHAT_CIRCLE_DOTS)));
        } else if shown.notification.slot.is_some() {
            out.push(act("inbox.open_chat", "Open its chat", "The agent's chat this came from", Command::OpenChat).importance(SECONDARY).header(Style::Button, "Open its chat", Some(icon::CHAT_TEARDROP_TEXT)));
        } else if shown.notification.terminal.is_some() {
            out.push(act("inbox.open_chat", "Open its terminal", "The terminal this came from", Command::OpenChat).importance(SECONDARY).header(Style::Button, "Open its terminal", Some(icon::TERMINAL_WINDOW)));
        }
        out.push(act("inbox.dismiss", "Put it away", "Off the list of what waits on you", Command::Dismiss).importance(MINOR).header(Style::Button, "Put away", Some(icon::ARCHIVE)));
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<Command>().cloned() else { return };
        let step = match command {
            Command::Step(by) => {
                self.asks.push(Ask::Waiting(Waiting::Step(by)));
                Step::Done
            }
            Command::OpenChat => self.open_chat(),
            Command::Dismiss => match &self.shown {
                Some(shown) => {
                    self.asks.push(Ask::Waiting(Waiting::Dismiss(shown.notification.id.clone())));
                    Step::Done
                }
                None => Step::Nothing("Already handled.".into()),
            },
            Command::Answer(option) => self.answer(option),
            Command::OpenPage(url) => {
                cx.ctx.open_url(egui::OpenUrl::new_tab(url));
                Step::Done
            }
        };
        if let Step::Nothing(why) = step {
            *cx.note = Some(why);
        }
    }
}

impl crate::hosted::Hosted for InboxView {
    /// What the item shown is: waiting on you, an update, handled, or none.
    fn title(&self) -> Status {
        match self.shown.as_ref().map(|s| s.notification.level) {
            Some(Level::Waiting) => Status::new(format!("{} Waiting on you", icon::BELL_RINGING), Tone::Warn),
            Some(Level::Update) => Status::new(format!("{} Update", icon::TRAY), Tone::Info),
            None if self.item.is_some() => Status::new(format!("{} Handled", icon::CHECK), Tone::Ok),
            None => Status::new(format!("{} Nothing needs you", icon::CHECK), Tone::Ok),
        }
    }

    /// Which one of how many.
    fn status(&self) -> Vec<Status> {
        self.at.map(|(at, of)| Status::new(format!("{at} of {of}"), Tone::Muted)).into_iter().collect()
    }
}
