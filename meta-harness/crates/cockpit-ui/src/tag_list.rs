//! The Tags buffer: every tag, its colour and note, and what it's on (each
//! thing: what it is, what it says now or that it's gone, who tagged it).
//! Managing them is here: rename, recolour, note, take one off a thing,
//! delete; Enter on a thing goes to it as a go-to does. What it changes it
//! asks the desk's tags to (`Ask::Tags`); it reads them through a reader.

use egui::{RichText, Vec2};
use egui_phosphor::regular as icon;
use harness_contracts::{Subject, TAG_COLOURS, Tag, TagId};

use crate::actions::{Action, ActionProvider, Choice, Group, MINOR, Param, RunCx, SECONDARY, When};
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Move, Navigable, Place, Spot, Step, Target};
use crate::shared::{Reader, Shared};
use crate::tags::{TagAct, Tags};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// The tag colours' names (by number: the theme's `tag_0`…`tag_7`).
const COLOURS: [&str; TAG_COLOURS as usize] = ["amber", "teal", "pink", "violet", "lime", "sky", "rose", "gold"];

/// A row: a tag, or a thing it's on (its place in the tag's list).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Tag(TagId),
    On(TagId, usize),
}

impl Row {
    fn key(&self) -> String {
        match self {
            Row::Tag(id) => id.to_string(),
            Row::On(id, n) => format!("{id}#{n}"),
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
    Untag,
    Delete,
}

#[derive(Default)]
pub struct TagList {
    tags: Reader<Tags>,
    rows: Vec<Row>,
    cursor: usize,
    /// The tag whose things' "now" was asked for last.
    asked_for: Option<TagId>,
    scrolling: Measured,
    spots: Vec<Spot>,
    asks: Vec<Ask>,
}

impl TagList {
    /// Its rows from the tags as they are: each tag, its things under it.
    fn sync(&mut self) {
        let at = self.rows.get(self.cursor).cloned();
        self.rows = self.tags.get().list().iter().flat_map(|t| std::iter::once(Row::Tag(t.id.clone())).chain((0..t.on.len()).map(|n| Row::On(t.id.clone(), n)))).collect();
        self.cursor = at.and_then(|r| self.rows.iter().position(|x| *x == r)).unwrap_or(self.cursor).min(self.rows.len().saturating_sub(1));
        // What the cursor's tag's things say now, asked for once per visit.
        let tag = self.rows.get(self.cursor).map(|r| r.tag().clone());
        if tag.is_some() && tag != self.asked_for {
            self.asked_for = tag.clone();
            if let Some(tag) = tag {
                self.asks.push(Ask::Tags(TagAct::Read { tag }));
            }
        }
    }

    fn tag_here(&self) -> Option<Tag> {
        let tags = self.tags.get();
        self.rows.get(self.cursor).and_then(|r| tags.get(r.tag()).cloned())
    }

    /// The thing the cursor's on, when it's on one.
    fn thing_here(&self) -> Option<(TagId, Subject)> {
        let Some(Row::On(id, n)) = self.rows.get(self.cursor) else { return None };
        Some((id.clone(), self.tags.get().get(id)?.on.get(*n)?.subject.clone()))
    }

    fn go(&mut self, go: Go) {
        let n = self.rows.len();
        let from = (self.cursor < n).then_some(self.cursor);
        if let Some(at) = self.scrolling.go(n, from, go) {
            self.cursor = at;
        }
    }

    /// Goes to `subject` in the buffer that shows its kind.
    fn go_to(&mut self, subject: Subject) -> Step {
        match crate::tags::shown_in(&subject) {
            Some(in_) => {
                self.asks.push(Ask::Show { target: Target::Subject(subject), in_ });
                Step::Done
            }
            None => Step::Nothing("Nothing in this cockpit shows that kind of thing.".into()),
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui, cx: &RunCx<'_>) {
        let (look, theme) = (crate::theme::look(), colors());
        if self.rows.is_empty() {
            ui.label(RichText::new("No tags yet: tag what you're on (its key is in the key bar) and it shows here.").size(look.text_body).color(theme.muted));
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
                            let count = if tag.on.len() == 1 { "on 1 thing".to_owned() } else { format!("on {} things", tag.on.len()) };
                            ui.label(RichText::new(count).size(look.text_small).color(theme.muted));
                            if !tag.note.is_empty() {
                                let by = if tag.noted_by.is_empty() { String::new() } else { format!(" ({})", tag.noted_by) };
                                ui.label(RichText::new(format!("{}{by}", tag.note)).size(look.text_body).color(theme.text));
                            }
                        })
                        .response
                        .rect
                    }
                    Row::On(id, n) => {
                        let Some(on) = tag.on.get(*n) else { continue };
                        let read = tags.read(id).and_then(|r| r.on.iter().find(|t| t.tagged.subject == on.subject)).and_then(|t| t.now.clone());
                        let what = read.as_ref().map_or_else(|| crate::tags::in_words(&on.subject), |n| n.what.clone());
                        let now = match read.as_ref().map(|r| r.now.clone()) {
                            Some(Some(now)) if now == on.said => None,
                            Some(Some(now)) => Some(format!("now: {}", first_line(&now))),
                            Some(None) => Some("gone (what it said is kept)".to_owned()),
                            None => None,
                        };
                        ui.horizontal(|ui| {
                            ui.add_space(look.space_l);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(what).size(look.text_body).color(theme.text));
                                ui.label(RichText::new(first_line(&on.said)).monospace().size(look.text_small).color(theme.muted));
                                if let Some(now) = now {
                                    ui.label(RichText::new(now).size(look.text_small).color(theme.warn));
                                }
                                if !on.by.is_empty() {
                                    ui.label(RichText::new(format!("tagged by {}", on.by)).size(look.text_small).color(theme.faint));
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
                drawn.push(Spot { key: row.key(), rect });
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

/// A text's first line, cut short.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().chars().take(120).collect()
}

impl crate::buffer::Buffer for TagList {
    fn render(&mut self, ui: &mut egui::Ui, cx: &mut RunCx<'_>, _drawing: &mut crate::buffer::Drawing<'_>) {
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
        self.thing_here().map(|(_, subject)| Target::Subject(subject))
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
        match self.thing_here().map(|(_, s)| s).or_else(|| self.tag_here()?.on.first().map(|o| o.subject.clone())) {
            Some(subject) => self.go_to(subject),
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
            .filter_map(|row| {
                let tag = tags.get(row.tag())?;
                Some(match row {
                    Row::Tag(_) => Item::new(row.key(), tag.name.clone(), tag.note.clone()),
                    Row::On(_, n) => {
                        let on = tag.on.get(*n)?;
                        Item::new(row.key(), "", format!("{} {}", crate::tags::in_words(&on.subject), on.said))
                    }
                })
            })
            .collect()
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.rows.get(self.cursor).map(Row::key)
    }

    fn move_to(&mut self, key: &str) {
        if let Some(at) = self.rows.iter().position(|r| r.key() == key) {
            self.cursor = at;
            self.scrolling.show(at, Show::Middle);
        }
    }
}

impl ActionProvider for TagList {
    fn actions(&self) -> Vec<Action> {
        let act = |id: &str, label: &str, c: Command| Action::new(id, label, c).focused().group(Group::Act).importance(MINOR);
        let when = |ok: bool| if ok { When::Always } else { When::Unavailable };
        let mut out = crate::nav::list_moves();
        out.push(Move::Enter.offer_if(!self.rows.is_empty(), "Go to it"));
        out.push(Move::Motion(Motion::NextSibling).offer_if(self.rows.len() > 1, "Next tag"));
        out.push(Move::Motion(Motion::PrevSibling).offer_if(self.rows.len() > 1, "Previous tag"));
        let tag = self.tag_here();
        let on_tag = tag.is_some();
        out.push(act("tags.rename", "Rename the tag", Command::Rename).param(Param::Text { hint: "a new name (letters, digits, - and _)".into() }).when(when(on_tag)).importance(SECONDARY));
        let colours = COLOURS.iter().enumerate().map(|(i, name)| Choice::new(i.to_string(), *name).current(tag.as_ref().is_some_and(|t| usize::from(t.colour) == i))).collect();
        out.push(act("tags.recolour", "Give the tag another colour", Command::Recolour).param(Param::Choice(colours)).when(when(on_tag)));
        out.push(act("tags.note", "Write the tag's note", Command::Note).param(Param::Text { hint: "its note (empty: none)".into() }).when(when(on_tag)));
        out.push(act("tags.untag", "Take the tag off this", Command::Untag).when(when(self.thing_here().is_some())));
        let sure = tag.as_ref().map(|t| vec![Choice::new("yes", format!("Delete {} (from everything it's on)", t.name))]).unwrap_or_default();
        out.push(act("tags.delete", "Delete the tag", Command::Delete).param(Param::Choice(sure)).when(when(on_tag)).icon(icon::TRASH));
        out
    }

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>) {
        let Some(command) = action.command::<Command>().cloned() else { return };
        let Some(tag) = self.tag_here().map(|t| t.id) else {
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
            (Command::Untag, _) => match self.thing_here() {
                Some((_, subject)) => TagAct::Untag { tag, subject },
                None => return,
            },
            (Command::Delete, Some(_)) => TagAct::Delete { tag },
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
        let n = self.tags.get().list().len();
        vec![Status::new(if n == 1 { "1 tag".to_owned() } else { format!("{n} tags") }, Tone::Muted)]
    }
}
