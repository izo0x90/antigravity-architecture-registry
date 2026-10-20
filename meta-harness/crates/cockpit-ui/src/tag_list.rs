//! The Tags buffer: every tag, its colour and note, and what it's on (each
//! thing: its name, what it says now or that it's gone, who tagged it).
//! Managing them is here: rename, recolour, note, take one off a thing,
//! delete; Enter on a thing goes to it as a go-to does. What it changes it
//! asks the desk's tags to (`Ask::Tags`); it reads them through a reader.

use egui::{RichText, Vec2};
use egui_phosphor::regular as icon;
use harness_contracts::{Subject, TAG_COLOURS, Tag, TagId, on_things};

use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, Param, RunCx, SECONDARY, When};
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Move, Navigable, Place, Spot, Step, Target, Things};
use crate::shared::{Reader, Shared};
use crate::tags::{TagAct, Tags, first_line, thing_name};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// How long the cursor rests on a tag before what its things say now is
/// read (moving through the list reads nothing).
const READ_AFTER: f64 = 0.3;

/// A row: a tag, or a thing it's on.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Tag(TagId),
    On(TagId, Subject),
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Tag(id) => id.to_string(),
            Row::On(id, subject) => format!("{id} {}", serde_json::to_string(subject).unwrap_or_default()),
        }
    }

    fn tag(&self) -> &TagId {
        match self {
            Row::Tag(id) | Row::On(id, _) => id,
        }
    }
}

#[derive(Clone, Debug)]
enum Command {
    Rename,
    Recolour,
    Note,
    ClearNote,
    Untag,
    Delete,
}

#[derive(Default)]
pub struct TagList {
    tags: Reader<Tags>,
    /// The tags' version its rows are of.
    synced: Option<u64>,
    rows: Vec<Row>,
    /// Each row's key (made when the rows are: a thing's is long).
    keys: Vec<String>,
    cursor: usize,
    /// The tag the cursor came to, and when (what its things say now is
    /// read once it rests there).
    resting: Option<(TagId, f64)>,
    /// The tag whose things' "now" was asked for last (and the tags as told
    /// then: told again, it's asked again).
    asked_for: Option<(TagId, u64)>,
    scrolling: Measured,
    spots: Vec<Spot>,
    asks: Vec<Ask>,
}

impl TagList {
    /// Its rows from the tags as they are (when they changed): each tag,
    /// its things under it. The cursor stays on its row.
    fn sync(&mut self) {
        let version = self.tags.get().version();
        if self.synced == Some(version) {
            return;
        }
        self.synced = Some(version);
        let at = self.rows.get(self.cursor).cloned();
        self.rows = self.tags.get().list().iter().flat_map(|t| std::iter::once(Row::Tag(t.id.clone())).chain(t.on.iter().map(|on| Row::On(t.id.clone(), on.subject.clone())))).collect();
        self.keys = self.rows.iter().map(Row::key).collect();
        self.cursor = at.and_then(|r| self.rows.iter().position(|x| *x == r)).unwrap_or(self.cursor).min(self.rows.len().saturating_sub(1));
    }

    /// What the cursor's tag's things say now, read once it rests on it
    /// (once per visit; again once the tags are told again).
    fn read_resting(&mut self, now: f64, ctx: &egui::Context) {
        let Some(tag) = self.rows.get(self.cursor).map(|r| r.tag().clone()) else { return };
        let told = self.tags.get().told_again();
        if self.asked_for.as_ref() == Some(&(tag.clone(), told)) {
            return;
        }
        match &self.resting {
            Some((resting, since)) if *resting == tag => {
                if now - since >= READ_AFTER {
                    self.asked_for = Some((tag.clone(), told));
                    self.asks.push(Ask::Tags(TagAct::Read { tag }));
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_secs_f64(READ_AFTER));
                }
            }
            _ => {
                self.resting = Some((tag, now));
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(READ_AFTER));
            }
        }
    }

    /// What `f` makes of the cursor's tag.
    fn tag_here<R>(&self, f: impl FnOnce(&Tag) -> R) -> Option<R> {
        let tags = self.tags.get();
        self.rows.get(self.cursor).and_then(|r| tags.get(r.tag())).map(f)
    }

    /// The thing the cursor's on, when it's on one (still tagged).
    fn thing_here(&self) -> Option<(TagId, Subject)> {
        let Some(Row::On(id, subject)) = self.rows.get(self.cursor) else { return None };
        self.tags.get().get(id)?.on.iter().any(|on| on.subject == *subject).then(|| (id.clone(), subject.clone()))
    }

    fn go(&mut self, go: Go) {
        let n = self.rows.len();
        let from = (self.cursor < n).then_some(self.cursor);
        if let Some(at) = self.scrolling.go(n, from, go) {
            self.cursor = at;
        }
    }

    /// Goes to `subject` where it's shown (the go-to path).
    fn go_to(&mut self, subject: &Subject) -> Step {
        match Target::of_subject(subject) {
            Some(target) => {
                self.asks.push(Ask::Show { target, in_: None });
                Step::Done
            }
            None => Step::Nothing("Nothing in this cockpit shows that kind of thing.".into()),
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, cx: &RunCx<'_>) {
        let (look, theme) = (crate::theme::look(), colors());
        if self.rows.is_empty() {
            ui.label(RichText::new("No tags yet: on something that can be tagged (a terminal's lines, a command it ran), \u{201c}Tag what you're on\u{201d} tags it, and it shows here.").size(look.text_body).color(theme.muted));
            self.spots.clear();
            return;
        }
        let tags = self.tags.get();
        let mut drawn = Vec::new();
        let area = self.scrolling.area(egui::ScrollArea::vertical().id_salt(("tags", cx.at)).auto_shrink([false, true]), ui.input(|i| i.time));
        let output = area.show(ui, |ui| {
            for (i, row) in self.rows.iter().enumerate() {
                let Some(tag) = tags.get(row.tag()) else { continue };
                let rect = match row {
                    Row::Tag(_) => {
                        ui.add_space(look.space_s);
                        ui.horizontal(|ui| {
                            let (dot, _) = ui.allocate_exact_size(Vec2::splat(look.text_body), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), look.text_body / 3.0, theme.tag(tag.colour));
                            ui.label(RichText::new(&tag.name).strong().size(look.text_title).color(theme.text_strong));
                            ui.label(RichText::new(on_things(tag.on.len())).size(look.text_small).color(theme.muted));
                            if !tag.note.is_empty() {
                                let by = if tag.noted_by.is_empty() { String::new() } else { format!(" ({})", who(&tag.noted_by)) };
                                ui.label(RichText::new(format!("{}{by}", tag.note)).size(look.text_body).color(theme.text));
                            }
                        })
                        .response
                        .rect
                    }
                    Row::On(id, subject) => {
                        let Some(on) = tag.on.iter().find(|on| on.subject == *subject) else { continue };
                        let (held, said) = match tags.now(id).and_then(|n| n.get(subject)) {
                            Some((held, text)) => (Some(*held), text.as_deref().map(first_line)),
                            None => (None, None),
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(look.space_l);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(thing_name(&on.what)).size(look.text_body).color(theme.text));
                                if let Some(said) = said {
                                    ui.label(RichText::new(said).monospace().size(look.text_small).color(theme.muted));
                                }
                                if let Some(held) = held.filter(|h| *h != harness_contracts::Held::Source) {
                                    ui.label(RichText::new(held.words()).size(look.text_small).color(theme.warn));
                                }
                                if !on.by.is_empty() {
                                    ui.label(RichText::new(format!("tagged by {}", who(&on.by))).size(look.text_small).color(theme.faint));
                                }
                            });
                        })
                        .response
                        .rect
                    }
                };
                if i == self.cursor {
                    ui.painter().rect_stroke(rect.expand(2.0), look.radius_m, egui::Stroke::new(look.line_thin, theme.focus), egui::StrokeKind::Outside);
                }
                drawn.push(Spot { key: self.keys[i].clone(), rect });
            }
        });
        if drawn.len() == self.rows.len() {
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

/// Who changed something, to the user: you, or an agent's slot.
fn who(by: &str) -> &str {
    if by == "user" { "you" } else { by }
}

impl crate::buffer::Buffer for TagList {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, _drawing: &mut crate::buffer::Drawing<'_>) {
        self.read_resting(ui.input(|i| i.time), ui.ctx());
        self.draw(ui, cx);
    }

    fn describe(&self) -> String {
        "the tags".to_owned()
    }
}

impl Navigable for TagList {
    fn take_asks(&mut self) -> Vec<Ask> {
        std::mem::take(&mut self.asks)
    }

    fn share(&mut self, shared: &Shared) {
        self.tags.follow(&shared.tags);
        self.sync();
    }

    /// The row it's on.
    fn place(&self) -> Place {
        Place::of(&self.cursor_key())
    }

    fn restore(&mut self, place: &Place) {
        if let Some(Some(key)) = place.read::<Option<String>>() {
            self.move_to(&key);
        }
    }

    fn target(&self) -> Option<Target> {
        Target::of_subject(&self.thing_here()?.1)
    }

    /// A thing's row: the thing (another tag can be put on it here too).
    fn things(&self, key: &str) -> Things {
        let tags = self.tags.get();
        match self.rows.iter().zip(&self.keys).find(|(_, k)| *k == key).map(|(row, _)| row) {
            Some(Row::On(tag, subject)) => {
                let what = tags.get(tag).and_then(|t| t.on.iter().find(|on| on.subject == *subject)).map(|on| thing_name(&on.what)).unwrap_or_default();
                Things::one(subject.clone(), what)
            }
            _ => Things::default(),
        }
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
                self.cursor = i.saturating_sub(1).min(self.rows.len().saturating_sub(1));
                self.scrolling.show(self.cursor, Show::Middle);
            }
            // From tag to tag.
            Motion::NextSibling | Motion::PrevSibling => {
                let forward = motion == Motion::NextSibling;
                let tags: Vec<usize> = self.rows.iter().enumerate().filter(|(_, r)| matches!(r, Row::Tag(_))).map(|(i, _)| i).collect();
                let next = if forward { tags.iter().find(|i| **i > self.cursor) } else { tags.iter().rev().find(|i| **i < self.cursor) };
                if let Some(at) = next.copied() {
                    self.cursor = at;
                    self.scrolling.show(at, Show::InView);
                }
            }
            Motion::Left | Motion::Right | Motion::HalfLeft | Motion::HalfRight | Motion::FewLeft | Motion::FewRight => {}
        }
    }

    /// On a thing: goes to it; on a tag: to the first thing it's on.
    fn enter(&mut self) -> Step {
        match self.thing_here().map(|(_, s)| s).or_else(|| self.tag_here(|t| t.on.first().map(|o| o.subject.clone())).flatten()) {
            Some(subject) => self.go_to(&subject),
            None => Step::Nothing("It's on nothing.".into()),
        }
    }

    fn leave(&mut self) -> Step {
        Step::Nothing("The tags have nothing to go out of.".into())
    }

    fn fold(&mut self, _fold: Fold) {}

    fn items(&self) -> Vec<Item> {
        let tags = self.tags.get();
        self.rows
            .iter()
            .zip(&self.keys)
            .filter_map(|(row, key)| {
                let tag = tags.get(row.tag())?;
                Some(match row {
                    Row::Tag(_) => Item::new(key.clone(), tag.name.clone(), tag.note.clone()),
                    Row::On(_, subject) => {
                        let on = tag.on.iter().find(|on| on.subject == *subject)?;
                        let said = tags.now(row.tag()).and_then(|n| n.get(subject)).and_then(|(_, t)| t.clone()).unwrap_or_default();
                        Item::new(key.clone(), "", format!("{} {said}", on.what))
                    }
                })
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.keys.get(self.cursor).cloned()
    }

    fn move_to(&mut self, key: &str) {
        if let Some(at) = self.keys.iter().position(|k| k == key) {
            self.cursor = at;
            self.scrolling.show(at, Show::Middle);
        }
    }
}

impl ActionProvider for TagList {
    fn actions(&self) -> Vec<Action> {
        let act = |id: &str, label: &str, detail: &str, c: Command| Action::new(id, label, detail, c).focused().group(Group::Act).importance(MINOR);
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        let mut out = crate::nav::list_moves();
        out.push(Move::Enter.offer_if(!self.rows.is_empty(), "Go to it"));
        out.push(Move::Motion(Motion::NextSibling).offer_if(self.rows.len() > 1, "Next tag"));
        out.push(Move::Motion(Motion::PrevSibling).offer_if(self.rows.len() > 1, "Previous tag"));
        let tag = self.tag_here(|t| (t.name.clone(), t.colour, !t.note.is_empty()));
        let on_tag = tag.is_some();
        out.push(act("tags.rename", "Rename the tag", "A new name for it (what it's on stays)", Command::Rename).param(Param::Text { hint: "a new name (letters, digits, - and _)".into() }).when(when(on_tag)).importance(SECONDARY));
        let theme = colors();
        let colours = (0..TAG_COLOURS).map(|i| Choice::new(i.to_string(), theme.tag_name(i)).current(tag.as_ref().is_some_and(|t| t.1 == i))).collect();
        out.push(act("tags.recolour", "Give the tag another colour", "One of the theme's tag colours", Command::Recolour).param(Param::Choice(colours)).when(when(on_tag)));
        out.push(act("tags.note", "Write the tag's note", "What it's for, said wherever the tag is (agents read it too)", Command::Note).param(Param::Text { hint: "its note".into() }).when(when(on_tag)));
        out.push(act("tags.note_clear", "Clear the tag's note", "It has no note from now on", Command::ClearNote).when(when(tag.as_ref().is_some_and(|t| t.2))));
        out.push(act("tags.untag", "Take the tag off this", "The tag stays on everything else", Command::Untag).when(when(self.thing_here().is_some())));
        let sure = tag.as_ref().map(|t| vec![Choice::new("yes", format!("Delete {} (from everything it's on)", t.0)), Choice::new("no", "Keep it")]).unwrap_or_default();
        out.push(act("tags.delete", "Delete the tag", "Off everything it's on, for good", Command::Delete).param(Param::Choice(sure)).when(when(on_tag)).icon(icon::TRASH));
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<Command>().cloned() else { return };
        let Some(tag) = self.tag_here(|t| t.id.clone()) else {
            *cx.note = Some("The cursor isn't on a tag.".into());
            return;
        };
        let act = match (command, value) {
            (Command::Rename, Some(name)) => TagAct::Rename { tag, name: name.trim().to_owned() },
            (Command::Recolour, Some(colour)) => match colour.parse() {
                Ok(colour) => TagAct::Recolour { tag, colour },
                Err(_) => return,
            },
            (Command::Note, Some(note)) => TagAct::Note { tag, note: note.trim().to_owned() },
            (Command::ClearNote, _) => TagAct::Note { tag, note: String::new() },
            (Command::Untag, _) => match self.thing_here() {
                Some((_, subject)) => TagAct::Untag { tag, subject },
                None => return,
            },
            (Command::Delete, Some(yes)) if yes == "yes" => TagAct::Delete { tag },
            _ => return,
        };
        self.asks.push(Ask::Tags(act));
    }
}

impl Hosted for TagList {
    fn title(&self) -> Status {
        Status::new(format!("{} Tags", icon::TAG), Tone::Muted)
    }

    fn status(&self) -> Vec<Status> {
        let tags = self.tags.get();
        let n = tags.list().len();
        let mut out = vec![Status::new(if n == 1 { "1 tag".to_owned() } else { format!("{n} tags") }, Tone::Muted)];
        if let Some(why) = self.rows.get(self.cursor).and_then(|r| tags.unread(r.tag())) {
            out.push(Status::new(format!("What its things say now couldn't be read: {why}"), Tone::Warn));
        }
        out
    }
}
