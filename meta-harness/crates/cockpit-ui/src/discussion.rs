//! The discussion buffer: one discussion. At the top, the thing it's on as
//! it is now (or as it was when the discussion started, once it changed or
//! went: said so); then everything said, as every conversation draws it;
//! then the typing box. It opens in the quick look (or a panel). Its
//! actions (reply, settle, reopen with why, make it a change request) have
//! the keymap's keys and are drawn by its host; it adds no buttons of its
//! own. It reads the discussions through a reader (the desk's, from the
//! event stream) and asks the server what the thing says now.

use std::sync::{Arc, Mutex};

use egui::RichText;
use egui_phosphor::regular as icon;
use harness_contracts::{Discussion, DiscussionId, DiscussionKind, DiscussionStatus, OnNow};

use crate::actions::{Action, ActionProvider, Group, MINOR, PRIMARY, RunCx, SECONDARY, Style};
use crate::discussions::Discussions;
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Move as NavMove, Navigable, Place, Spot, Step, Target, Things};
use crate::net::{NetClient, NetResult};
use crate::shared::{Reader, Shared};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// What its typing box is writing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Writing {
    /// Something said in it.
    #[default]
    Reply,
    /// Why it's open again.
    Reopen,
    /// Why it asks for a change (its answer named none).
    Change,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Reply,
    Settle,
    Reopen,
    Change,
    Send,
    Cancel,
}

/// What came back from the server: what the thing says now, or a change
/// sent (an error said).
enum Reply {
    On(DiscussionId, NetResult<OnNow>),
    Sent(Result<(), String>),
}

#[derive(Default)]
pub struct DiscussionView {
    discussions: Reader<Discussions>,
    /// The discussion shown.
    pub id: Option<DiscussionId>,
    /// What it's on says (for which discussion), as last read; asked again
    /// when the architecture changed or what's said in it did.
    on_now: Option<(DiscussionId, Result<OnNow, String>)>,
    asked: Option<(DiscussionId, usize)>,
    /// What was last sent (and what for), until the server has it: not
    /// sent, it comes back to the box.
    unsent: Option<(DiscussionId, crate::conversation::TypingBox, Writing)>,
    /// What's written in its box, and what for.
    pub writing: crate::conversation::TypingBox,
    pub purpose: Writing,
    /// The cursor: 0, what it's on; then each thing said.
    cursor: usize,
    scrolling: Measured,
    spots: Vec<Spot>,
    typing: Option<egui::Rect>,
    markdown: egui_commonmark::CommonMarkCache,
    note: Option<String>,
    asks: Vec<Ask>,
    inbox: Arc<Mutex<Vec<Reply>>>,
}

fn push(inbox: &Arc<Mutex<Vec<Reply>>>, reply: Reply) {
    if let Ok(mut i) = inbox.lock() {
        i.push(reply);
    }
}

impl DiscussionView {
    /// The discussion shown, as the desk has it now.
    fn shown(&self) -> Option<Discussion> {
        self.with(Discussion::clone)
    }

    /// `f` of the discussion shown, read where it's held (not copied).
    fn with<R>(&self, f: impl FnOnce(&Discussion) -> R) -> Option<R> {
        self.discussions.get().get(self.id.as_ref()?).map(f)
    }

    /// Its stops: what it's on, then each thing said.
    fn stops(&self) -> usize {
        self.with(|d| 1 + d.said.len()).unwrap_or(0)
    }

    fn go(&mut self, go: Go) {
        if let Some(at) = self.scrolling.go(self.stops(), Some(self.cursor), go) {
            self.cursor = at;
        }
    }

    fn sent(&self) -> impl FnOnce(NetResult<Discussion>) + Send + 'static {
        let inbox = self.inbox.clone();
        move |r: NetResult<Discussion>| push(&inbox, Reply::Sent(r.map(|_| ()).map_err(String::from)))
    }

    /// Opens its box for `purpose`: the keyboard goes there.
    fn write(&mut self, purpose: Writing) {
        if self.purpose != purpose {
            self.writing = Default::default();
        }
        self.purpose = purpose;
        self.asks.push(Ask::Type);
    }

    /// Sends what its box holds (a reply, why it's reopened, why it asks
    /// for a change).
    fn send(&mut self, net: &NetClient, ctx: &egui::Context) {
        let Some(id) = self.id.clone() else { return };
        if self.writing.text.trim().is_empty() {
            return;
        }
        let written = std::mem::take(&mut self.writing);
        let message = written.message();
        self.unsent = Some((id.clone(), written, self.purpose));
        let action = match self.purpose {
            Writing::Reply => "reply",
            Writing::Reopen => "reopen",
            Writing::Change => "change",
        };
        net.discussion_act(&id, action, serde_json::json!(message), ctx.clone(), self.sent());
        self.purpose = Writing::Reply;
    }

    /// What it's on, as it is now (or as it was), at the top.
    fn draw_on(&self, ui: &mut egui::Ui, d: &Discussion) {
        let look = crate::theme::look();
        ui.label(RichText::new(format!("{} {}", icon::CROSSHAIR, d.on.what)).strong().size(look.text_body));
        match self.on_now.as_ref().filter(|(id, _)| *id == d.id).map(|(_, r)| r) {
            None => {
                ui.label(RichText::new("Reading what it says…").size(look.text_small).color(colors().muted));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(format!("What it says couldn't be read: {e}")).size(look.text_small).color(colors().warn));
            }
            Some(Ok(now)) => {
                ui.label(RichText::new(now.held.words()).size(look.text_small).color(if now.held == harness_contracts::Held::Source { colors().muted } else { colors().warn }));
                if let Some(text) = &now.text {
                    let shown: Vec<&str> = text.lines().collect();
                    ui.label(RichText::new(shown.join("\n")).monospace().size(look.text_small).color(colors().text));
                    if now.lines > shown.len() as u64 {
                        ui.label(RichText::new(format!("({} lines in all)", now.lines)).size(look.text_small).color(colors().faint));
                    }
                }
            }
        }
    }

    /// What its box is for, in words.
    fn box_says(&self, d: &Discussion) -> String {
        match self.purpose {
            Writing::Reply => format!("Reply in {} (# names a tag)", d.id),
            Writing::Reopen => format!("Why reopen {}?", d.id),
            Writing::Change => format!("Why {} asks for a change", d.id),
        }
    }
}

impl crate::buffer::Buffer for DiscussionView {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, drawing: &mut crate::buffer::Drawing<'_>) {
        let Some(d) = self.shown() else {
            ui.label(RichText::new(match &self.id {
                Some(id) => format!("There's no discussion {id} (deleted, or not told yet)."),
                None => "No discussion shown: open one (from what it's on, the plan tree, the inbox).".to_owned(),
            }).color(colors().muted));
            self.spots.clear();
            return;
        };
        let look = crate::theme::look();
        crate::conversation::heading(ui, &d);
        ui.add_space(look.space_xs);
        // The typing box, under what's said (drawn first: it keeps its place).
        let mut markdown = std::mem::take(&mut self.markdown);
        let mut spots = Vec::new();
        let cursor = self.cursor;
        let box_height = look.text_body * 4.0 + look.space_l * 2.0;
        let area = self.scrolling.area(egui::ScrollArea::vertical().id_salt("discussion").auto_shrink([false, false]).max_height((ui.available_height() - box_height).max(look.text_body * 4.0)), ui.input(|i| i.time));
        let output = area.show(ui, |ui| {
            let stop = |ui: &mut egui::Ui, n: usize, spots: &mut Vec<Spot>, add: &mut dyn FnMut(&mut egui::Ui)| {
                let fill = if n == cursor { colors().selection } else { egui::Color32::TRANSPARENT };
                let shown = egui::Frame::NONE.fill(fill).corner_radius(look.radius_s).inner_margin(crate::theme::margin(look.space_xs)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    add(ui);
                });
                spots.push(Spot { key: n.to_string(), rect: shown.response.rect });
            };
            stop(ui, 0, &mut spots, &mut |ui| self.draw_on(ui, &d));
            ui.add_space(look.space_s);
            for (i, said) in d.said.iter().enumerate() {
                stop(ui, i + 1, &mut spots, &mut |ui| crate::conversation::message(ui, &mut markdown, &crate::conversation::said(&d, said)));
            }
        });
        for s in &spots {
            self.scrolling.item(s.rect);
        }
        self.scrolling.drawn(&output);
        self.spots = spots.into_iter().filter(|s| s.rect.intersects(output.inner_rect)).collect();
        self.markdown = markdown;
        if d.status != DiscussionStatus::Settled || self.purpose == Writing::Reopen {
            ui.label(RichText::new(self.box_says(&d)).size(look.text_small).color(colors().muted));
            let typed = self.writing.draw(ui, egui::Id::new(("discussion_box", cx.at)), 2, "Write it here…", drawing.typing);
            if typed.clicked {
                *cx.input_mode = crate::keymap::InputMode::Insert { at: cx.at };
            }
            if let Some(c) = typed.trigger {
                self.asks.push(Ask::Trigger(c));
            }
            self.typing = drawing.typing.then_some(typed.rect);
            if typed.sent.is_some() {
                self.send(cx.net, cx.ctx);
                *cx.input_mode = crate::keymap::InputMode::Normal;
            }
        } else {
            self.typing = None;
        }
        if let Some(note) = &self.note {
            ui.label(RichText::new(note).size(look.text_small).color(colors().warn));
        }
    }

    /// What it's on is read when it isn't yet (or what's said changed).
    fn tick(&mut self, net: &NetClient, ctx: &egui::Context) {
        for reply in std::mem::take(&mut *self.inbox.lock().unwrap_or_else(std::sync::PoisonError::into_inner)) {
            match reply {
                // A read of one shown before (answered late): not this one's.
                Reply::On(id, read) if self.asked.as_ref().is_some_and(|(asked, _)| *asked == id) => self.on_now = Some((id, read.map_err(String::from))),
                Reply::On(..) => {}
                Reply::Sent(Ok(())) => (self.note, self.unsent) = (None, None),
                // Not sent: what was written comes back to its box.
                Reply::Sent(Err(e)) => {
                    self.note = Some(e);
                    // (Only into the discussion it was written for, still shown.)
                    if let Some((_, written, purpose)) = self.unsent.take().filter(|(id, ..)| self.id.as_ref() == Some(id) && self.writing.text.trim().is_empty()) {
                        self.purpose = purpose;
                        self.writing.put_back(written);
                    }
                }
            }
        }
        let Some(now) = self.with(|d| (d.id.clone(), d.said.len())) else { return };
        if self.asked.as_ref() == Some(&now) {
            return;
        }
        let id = now.0.clone();
        self.asked = Some(now);
        let inbox = self.inbox.clone();
        net.discussion_on(&id.clone(), ctx.clone(), move |r| push(&inbox, Reply::On(id, r)));
    }

    /// The architecture changed: what it's on is read again.
    fn on_event(&mut self, event: &arch_graph::ArchEvent) {
        use harness_contracts::Subject as S;
        // Only what the code or a plan says changes with it (not a
        // terminal's or a chat's).
        let of_code = self.with(|d| matches!(d.on.subject, S::Component { .. } | S::CodeLines { .. } | S::CodeFile { .. } | S::Plan { .. } | S::PlanComponent { .. } | S::PlanSection { .. } | S::PlanRelation { .. } | S::PlanItem { .. })).unwrap_or(false);
        if of_code && matches!(event, arch_graph::ArchEvent::Changed { .. }) {
            self.asked = None;
        }
    }

    fn invalidate(&mut self) {
        self.asked = None;
    }

    fn describe(&self) -> String {
        self.id.as_ref().map_or_else(|| "no discussion".to_owned(), |id| format!("discussion {id}"))
    }
}

impl Navigable for DiscussionView {
    /// Which discussion.
    fn place(&self) -> Place {
        Place::of(&self.id)
    }

    fn restore(&mut self, place: &Place) {
        if let Some(Some(id)) = place.read::<Option<DiscussionId>>() {
            let _ = self.show_target(&Target::Discussion(id));
        }
    }

    /// On what it's on: that; on what's said: the discussion.
    fn target(&self) -> Option<Target> {
        self.with(|d| if self.cursor == 0 { Target::of_subject(&d.on.subject) } else { Some(Target::Discussion(d.id.clone())) }).flatten()
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        match motion {
            Motion::Down => self.go(Go::Down(count)),
            Motion::Up => self.go(Go::Up(count)),
            Motion::HalfDown => self.go(Go::HalfDown(count)),
            Motion::HalfUp => self.go(Go::HalfUp(count)),
            Motion::First => self.go(Go::First),
            Motion::Last => self.go(Go::Last),
            Motion::Line(i) => {
                self.cursor = i.saturating_sub(1).min(self.stops().saturating_sub(1));
                self.scrolling.show(self.cursor, Show::Middle);
            }
            _ => {}
        }
    }

    /// On what it's on: goes to it; on what's said: replies.
    fn enter(&mut self) -> Step {
        let Some(d) = self.shown() else { return Step::Nothing("No discussion shown.".into()) };
        if self.cursor == 0 {
            return match Target::of_subject(&d.on.subject) {
                Some(target) => {
                    self.asks.push(Ask::Show { target, in_: None });
                    Step::Done
                }
                None => Step::Nothing(format!("Nothing in this cockpit shows {}.", d.on.what)),
            };
        }
        if d.status == DiscussionStatus::Settled {
            return Step::Nothing(format!("{} is settled: reopen it to say more.", d.id));
        }
        self.write(Writing::Reply);
        Step::Done
    }

    fn leave(&mut self) -> Step {
        self.cursor = 0;
        self.scrolling.show(0, Show::InView);
        Step::Done
    }

    fn fold(&mut self, _fold: Fold) {}

    /// What's said, to search.
    fn items(&self) -> Vec<Item> {
        let Some(d) = self.shown() else { return Vec::new() };
        std::iter::once(Item::new("0", d.on.what.clone(), String::new())).chain(d.said.iter().enumerate().map(|(i, s)| Item::new((i + 1).to_string(), d.to_user(&s.by).to_owned(), s.text.clone()))).collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.id.as_ref().map(|_| self.cursor.to_string())
    }

    fn move_to(&mut self, key: &str) {
        if let Ok(at) = key.parse::<usize>() {
            self.cursor = at.min(self.stops().saturating_sub(1));
            self.scrolling.show(self.cursor, Show::InView);
        }
    }

    /// The discussion, then what it's on (it can be tagged, or discussed).
    fn things(&self, _key: &str) -> Things {
        let known = harness_contracts::Known::default();
        self.with(|d| Things { at: vec![crate::nav::Thing::named(harness_contracts::Subject::Discussion { id: d.id.clone() }, &known), crate::nav::Thing { subject: d.on.subject.clone(), words: d.on.what.clone() }], unit: 0 }).unwrap_or_default()
    }

    fn typing(&self) -> Option<&crate::conversation::TypingBox> {
        self.with(|_| ()).is_some().then_some(&self.writing)
    }

    fn typing_box(&mut self) -> Option<&mut crate::conversation::TypingBox> {
        self.with(|_| ()).is_some().then_some(&mut self.writing)
    }

    fn can_show(&self, target: &Target) -> bool {
        matches!(target, Target::Discussion(_))
    }

    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let Target::Discussion(id) = target else { return Err("Not a discussion.".into()) };
        if self.id.as_ref() != Some(id) {
            self.id = Some(id.clone());
            self.cursor = 0;
            self.purpose = Writing::Reply;
            self.writing = Default::default();
            self.scrolling = Measured::default();
            self.note = None;
        }
        Ok(())
    }

    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    fn share(&mut self, shared: &Shared) {
        self.discussions.follow(&shared.discussions);
    }
}

impl ActionProvider for DiscussionView {
    fn actions(&self) -> Vec<Action> {
        let f = |id: &str, label: &str, detail: &str, c: Command| Action::new(id, label, detail, c).focused().group(Group::Act);
        let mut out = crate::nav::list_moves();
        // (What its actions need of it, not a copy of all that's said.)
        struct Standing {
            what: String,
            status: DiscussionStatus,
            kind: DiscussionKind,
            asked: String,
        }
        let Some(d) = self.with(|d| Standing { what: d.on.what.clone(), status: d.status, kind: d.kind, asked: d.asked.clone() }) else { return out };
        out.push(NavMove::Enter.offer(if self.cursor == 0 { format!("Go to {}", d.what) } else { "Reply".to_owned() }));
        out.push(NavMove::Leave.offer_if(self.cursor > 0, "Up to what it's on"));
        if self.typing.is_some() {
            out.push(f("discussion.send", "Send what's written", "Said in the discussion", Command::Send).importance(PRIMARY).typed(crate::keymap::Sent::Send).on_typing(Style::Button, "Send", Some(icon::PAPER_PLANE_RIGHT)));
            out.push(f("discussion.cancel", "Stop writing", "What's written stays in the box", Command::Cancel).importance(PRIMARY).on_typing(Style::Button, "Stop", None));
        }
        let open = d.status != DiscussionStatus::Settled;
        out.push(f("discussion.reply", "Reply", "Say something in it (it stays where it stands)", Command::Reply).importance(if open { PRIMARY } else { MINOR }).when(if open { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        // Answered, or asked of you (once you've said your answer).
        let asks_you = d.asked == harness_contracts::USER && d.status == DiscussionStatus::Open;
        let settles = d.status == DiscussionStatus::Answered || asks_you;
        out.push(
            f("discussion.settle", if asks_you { "Settle it (your answer's said)" } else { "Settle it (accept its answer)" }, "Settled: it holds nothing any more (a plan's approval)", Command::Settle)
                .importance(if settles { PRIMARY } else { MINOR })
                .when(if settles { crate::actions::When::Always } else { crate::actions::When::Unavailable })
                .header(Style::Button, "Settle", Some(icon::CHECK)),
        );
        out.push(
            f("discussion.reopen", "Reopen it (say why)", "Open again: the agent asked answers again", Command::Reopen)
                .importance(SECONDARY)
                .when(if d.status != DiscussionStatus::Open { crate::actions::When::Always } else { crate::actions::When::Unavailable })
                .header(Style::Button, "Reopen", Some(icon::ARROW_COUNTER_CLOCKWISE)),
        );
        // (A change request asks an agent slot: its answer names what it changed.)
        let question = d.kind == DiscussionKind::Question && open && d.asked != harness_contracts::USER;
        out.push(f("discussion.change", "Make it a change request", "Its answer has to name what changed (answered: say why it's open again)", Command::Change).importance(MINOR).when(if question { crate::actions::When::Always } else { crate::actions::When::Unavailable }));
        out
    }

    fn run(&mut self, action: &Action, _value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(d) = self.shown() else { return };
        match action.command::<Command>() {
            Some(Command::Reply) => self.write(Writing::Reply),
            Some(Command::Reopen) => self.write(Writing::Reopen),
            Some(Command::Settle) => cx.net.discussion_act(&d.id, "settle", serde_json::json!({}), cx.ctx.clone(), self.sent()),
            // Answered: open again, so why; else at once.
            Some(Command::Change) if d.status == DiscussionStatus::Answered => self.write(Writing::Change),
            Some(Command::Change) => cx.net.discussion_act(&d.id, "change", serde_json::json!({}), cx.ctx.clone(), self.sent()),
            Some(Command::Send) => {
                self.send(cx.net, cx.ctx);
                *cx.input_mode = crate::keymap::InputMode::Normal;
                cx.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(("discussion_box", cx.at))));
            }
            Some(Command::Cancel) => {
                *cx.input_mode = crate::keymap::InputMode::Normal;
                cx.ctx.memory_mut(|m| m.surrender_focus(egui::Id::new(("discussion_box", cx.at))));
            }
            None => {}
        }
    }
}

impl Hosted for DiscussionView {
    fn title(&self) -> Status {
        self.with(|d| Status::new(format!("{} {} · {}", icon::CHAT_CIRCLE_DOTS, d.id, d.kind.words()), Tone::Info).detail(d.on.what.clone())).unwrap_or_else(|| Status::new(format!("{} Discussion", icon::CHAT_CIRCLE_DOTS), Tone::Muted))
    }

    /// Whose move it is.
    fn status(&self) -> Vec<Status> {
        self.with(|d| match crate::conversation::whose(d) {
            (words, true) => Status::new(words, Tone::Warn),
            (words, _) if d.status == DiscussionStatus::Settled => Status::new(words, Tone::Muted),
            (words, _) => Status::new(words, Tone::Info),
        })
        .into_iter()
        .collect()
    }

    fn typing_rect(&self) -> Option<egui::Rect> {
        self.typing
    }
}
