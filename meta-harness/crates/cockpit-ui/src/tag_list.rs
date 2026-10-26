//! The Tags buffer: every tag on a line of its own (a solid tag in its
//! colour, its name, how many things it's on, its note), folded to start;
//! unfolded, what it's on (each thing: its name, the start of what it says
//! or where it went, who tagged it and when). It adds no keys, buttons or
//! pickers of its own: the shared moves, folds and search move through
//! it, Enter on a thing goes to it as a go-to does, and the shell's tag
//! actions (rename, colour, note, delete, take off) act on its rows (it
//! says which tag a row is, `tag_at`). It reads the tags through a reader
//! and asks the desk's tags for what their things say (`Ask::Tags`).

use std::collections::BTreeSet;

use egui::RichText;
use egui_phosphor::regular as icon;
use harness_contracts::{Subject, Tag, TagId, on_things};

use crate::actions::{Action, ActionProvider, At, Control, RunCx, Style};
use crate::conversation::first_line;
use crate::hosted::{Hosted, Status, Tone};
use crate::nav::{Ask, Fold, Item, Motion, Move, Navigable, Place, Spot, Step, Target, Things};
use crate::shared::{Reader, Shared};
use crate::tags::{TagAct, Tags, thing_name};
use crate::theme::colors;
use crate::viewport::{Go, Measured, Show};

/// How long the cursor rests on a tag before what its things say now is
/// read (moving through the list reads nothing).
const READ_AFTER: f64 = 0.3;
/// A row's height until it's drawn: a tag's line, a thing's three lines.
const TAG_ROW: f32 = 30.0;
const ON_ROW: f32 = 54.0;

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

#[derive(Default)]
pub struct TagList {
    tags: Reader<Tags>,
    /// The server's clock (its tags' times are on it).
    clock: crate::agents::ServerClock,
    /// The tags' version its rows are of.
    synced: Option<u64>,
    /// The tags unfolded (each folds to its line to start).
    open: BTreeSet<TagId>,
    rows: Vec<Row>,
    /// Each row's key (made when the rows are: a thing's is long).
    keys: Vec<String>,
    /// For each row, which of its tag's things it is (a tag's row: 0).
    ons: Vec<usize>,
    /// Each row's height as last drawn (a guess until then): the rows are
    /// laid out by them, and only those in view are drawn.
    heights: Vec<f32>,
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
        // A tag gone is no longer unfolded.
        let ids: BTreeSet<TagId> = self.tags.get().list().iter().map(|t| t.id.clone()).collect();
        self.open.retain(|id| ids.contains(id));
        let open = &self.open;
        let rows: Vec<(Row, usize)> = self
            .tags
            .get()
            .list()
            .iter()
            .flat_map(|t| {
                std::iter::once((Row::Tag(t.id.clone()), 0)).chain(
                    t.on.iter().enumerate().filter(|_| open.contains(&t.id)).map(|(i, on)| (Row::On(t.id.clone(), on.subject.clone()), i)),
                )
            })
            .collect();
        // A row's height is kept from the last time it was drawn.
        let was: std::collections::HashMap<&str, f32> = self.keys.iter().zip(&self.heights).map(|(k, h)| (k.as_str(), *h)).collect();
        let keys: Vec<String> = rows.iter().map(|(r, _)| r.key()).collect();
        self.heights = rows
            .iter()
            .zip(&keys)
            .map(|((r, _), k)| {
                was.get(k.as_str()).copied().unwrap_or(match r {
                    Row::Tag(_) => TAG_ROW,
                    Row::On(..) => ON_ROW,
                })
            })
            .collect();
        (self.rows, self.ons) = rows.into_iter().unzip();
        self.keys = keys;
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

    /// The row of `key` (the cursor's first: it's the one asked about).
    fn row(&self, key: &str) -> Option<&Row> {
        if self.keys.get(self.cursor).is_some_and(|k| k == key) {
            return self.rows.get(self.cursor);
        }
        self.rows.iter().zip(&self.keys).find(|(_, k)| *k == key).map(|(row, _)| row)
    }

    /// Unfolds (or folds) tag `id`: its things show (or go, the cursor on
    /// it then).
    fn set_open(&mut self, id: &TagId, open: bool) {
        let changed = if open { self.open.insert(id.clone()) } else { self.open.remove(id) };
        if changed {
            if !open && matches!(self.rows.get(self.cursor), Some(Row::On(tag, _)) if tag == id) {
                self.cursor = self.rows.iter().position(|r| *r == Row::Tag(id.clone())).unwrap_or(self.cursor);
            }
            self.synced = None;
            self.sync();
        }
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
            ui.label(RichText::new("No tags yet: on something that can be tagged (a terminal's lines or a command it ran, a chat's message or turn, a line of code or a definition), \u{201c}Tag what you're on\u{201d} (M) tags it, and it shows here.").size(look.text_body).color(theme.muted));
            self.spots.clear();
            return;
        }
        let tags = self.tags.get();
        let now = self.clock.now_ms();
        // The rows laid out by their heights; only those in view are drawn.
        let tops: Vec<f32> = self
            .heights
            .iter()
            .scan(0.0, |top, h| {
                let at = *top;
                *top += h;
                Some(at)
            })
            .collect();
        let total: f32 = self.heights.iter().sum();
        let mut drawn: Vec<(usize, f32)> = Vec::new();
        let area =
            self.scrolling.area(egui::ScrollArea::vertical().id_salt(("tags", cx.at)).auto_shrink([false, true]), ui.input(|i| i.time));
        let output = area.show_viewport(ui, |ui, view| {
            ui.set_height(total);
            let (origin, width) = (ui.min_rect().min, ui.available_width());
            let first = tops.partition_point(|t| *t <= view.min.y).saturating_sub(1);
            for (i, row) in self.rows.iter().enumerate().skip(first).take_while(|(i, _)| tops[*i] < view.max.y) {
                let Some(tag) = tags.get(row.tag()) else { continue };
                let place = egui::Rect::from_min_size(origin + egui::vec2(0.0, tops[i]), egui::vec2(width, self.heights[i]));
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(place));
                let ui = &mut ui;
                let rect = match row {
                    Row::Tag(id) => {
                        ui.add_space(look.space_s);
                        let folded = !self.open.contains(id);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(crate::nav::fold_mark(!folded)).size(look.text_small).color(theme.faint));
                            ui.label(crate::theme::tag_mark(ui, theme.tag(tag.colour), look.text_title));
                            ui.label(RichText::new(&tag.name).strong().size(look.text_title).color(theme.text_strong));
                            ui.label(RichText::new(on_things(tag.on.len())).size(look.text_small).color(theme.muted));
                            if !tag.note.is_empty() {
                                let by = if tag.noted_by.is_empty() {
                                    String::new()
                                } else {
                                    format!(" ({})", crate::conversation::who(&tag.noted_by))
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{}{by}", first_line(&tag.note))).size(look.text_body).color(theme.text),
                                    )
                                    .truncate(),
                                );
                            }
                        })
                        .response
                        .rect
                    }
                    Row::On(id, subject) => {
                        let Some(on) = tag.on.get(self.ons[i]).filter(|on| on.subject == *subject) else { continue };
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
                                let when = crate::attention::age(on.at_ms, now);
                                ui.label(
                                    RichText::new(if on.by.is_empty() {
                                        format!("tagged {when}")
                                    } else {
                                        format!("tagged by {} {when}", crate::conversation::who(&on.by))
                                    })
                                    .size(look.text_small)
                                    .color(theme.faint),
                                );
                            });
                        })
                        .response
                        .rect
                    }
                };
                if i == self.cursor {
                    ui.painter().rect_stroke(
                        rect.expand(2.0),
                        look.radius_m,
                        egui::Stroke::new(look.line_thin, theme.focus),
                        egui::StrokeKind::Outside,
                    );
                }
                drawn.push((i, rect.height() + look.space_s));
            }
        });
        // Rows drawn taller than laid out keep their height (none overlap
        // next frame); every row's place is told to the scrolling.
        let mut grew = false;
        for (i, h) in &drawn {
            if *h > self.heights[*i] + 0.5 {
                self.heights[*i] = *h;
                grew = true;
            }
        }
        let origin = output.inner_rect.min - output.state.offset;
        for (i, top) in tops.iter().enumerate() {
            self.scrolling
                .item(egui::Rect::from_min_size(origin + egui::vec2(0.0, *top), egui::vec2(output.inner_rect.width(), self.heights[i])));
        }
        self.scrolling.drawn(&output);
        if grew || self.scrolling.busy() {
            ui.ctx().request_repaint();
        }
        self.spots = drawn
            .into_iter()
            .map(|(i, _)| Spot {
                key: self.keys[i].clone(),
                rect: egui::Rect::from_min_size(origin + egui::vec2(0.0, tops[i]), egui::vec2(output.inner_rect.width(), self.heights[i])),
            })
            .filter(|s| s.rect.intersects(output.inner_rect))
            .collect();
    }
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
        self.clock = shared.agents.get().clock();
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

    /// A tag there is.
    fn can_show(&self, target: &Target) -> bool {
        matches!(target, Target::Tag(id) if self.tags.get().get(id).is_some())
    }

    /// The tag, unfolded (its things under it), the cursor on it.
    fn show_target(&mut self, target: &Target) -> Result<(), String> {
        let Target::Tag(id) = target else { return Err("Not a tag.".into()) };
        if self.tags.get().get(id).is_none() {
            return Err("That tag is gone.".into());
        }
        self.set_open(id, true);
        if let Some(at) = self.rows.iter().position(|r| *r == Row::Tag(id.clone())) {
            self.cursor = at;
            self.scrolling.show(at, crate::viewport::Show::Middle);
        }
        Ok(())
    }

    /// A thing's row: the thing (another tag can be put on it here too).
    fn things(&self, key: &str) -> Things {
        let tags = self.tags.get();
        match self.row(key) {
            Some(Row::On(tag, subject)) => {
                // Named as it was named when tagged (named so since, as it moved).
                let words = tags
                    .get(tag)
                    .and_then(|t| t.on.iter().find(|on| on.subject == *subject))
                    .map(|on| thing_name(&on.what))
                    .unwrap_or_default();
                Things { at: vec![crate::nav::Thing { subject: subject.clone(), words }], unit: 0 }
            }
            _ => Things::default(),
        }
    }

    fn motion(&mut self, motion: Motion, count: usize) {
        if let Some(go) = Go::of(motion, count) {
            self.go(go);
            return;
        }
        match motion {
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
            _ => {}
        }
    }

    /// On a thing: goes to it; on a folded tag: unfolds it; on an
    /// unfolded one: into its first thing.
    fn enter(&mut self) -> Step {
        if let Some((_, subject)) = self.thing_here() {
            return self.go_to(&subject);
        }
        let Some(Row::Tag(id)) = self.rows.get(self.cursor).cloned() else { return Step::Nothing("Nothing here.".into()) };
        if self.tag_here(|t| t.on.is_empty()).unwrap_or(true) {
            return Step::Nothing("It's on nothing.".into());
        }
        if self.open.contains(&id) {
            self.go(Go::Down(1));
        } else {
            self.set_open(&id, true);
        }
        Step::Done
    }

    /// On a thing: up to its tag.
    fn leave(&mut self) -> Step {
        match self.rows.get(self.cursor) {
            Some(Row::On(id, _)) => {
                let id = id.clone();
                self.cursor = self.rows.iter().position(|r| *r == Row::Tag(id.clone())).unwrap_or(self.cursor);
                self.scrolling.show(self.cursor, Show::InView);
                Step::Done
            }
            _ => Step::Nothing("The tags have nothing to go out of.".into()),
        }
    }

    /// The cursor's tag (on a thing: its tag's), or every tag.
    fn fold(&mut self, fold: Fold) {
        let Some(id) = self.rows.get(self.cursor).map(|r| r.tag().clone()) else { return };
        match fold {
            Fold::Open => self.set_open(&id, true),
            Fold::Close => self.set_open(&id, false),
            Fold::Toggle => self.set_open(&id, !self.open.contains(&id)),
            Fold::OpenAll => {
                let at = self.rows.get(self.cursor).cloned();
                self.open = self.tags.get().list().iter().map(|t| t.id.clone()).collect();
                self.synced = None;
                self.sync();
                self.cursor = at.and_then(|r| self.rows.iter().position(|x| *x == r)).unwrap_or(self.cursor);
            }
            Fold::CloseAll => {
                self.cursor = self.rows.iter().position(|r| *r == Row::Tag(id.clone())).unwrap_or(self.cursor);
                self.open.clear();
                self.synced = None;
                self.sync();
            }
        }
    }

    /// A row's tag (a thing's row too: its tag's).
    fn tag_at(&self, key: &str) -> Option<TagId> {
        self.row(key).map(|row| row.tag().clone())
    }

    /// As of which change of the tags.
    fn items_changed(&self) -> Option<u64> {
        Some(self.tags.get().version())
    }

    /// Every tag and every thing it's on, folded or not (search finds a
    /// thing under a folded tag; going to it unfolds the tag).
    fn items(&self) -> Vec<Item> {
        let tags = self.tags.get();
        let mut out = Vec::new();
        for tag in tags.list() {
            out.push(Item::new(Row::Tag(tag.id.clone()).key(), tag.name.clone(), tag.note.clone()));
            for on in &tag.on {
                let said = tags.now(&tag.id).and_then(|n| n.get(&on.subject)).and_then(|(_, t)| t.clone()).unwrap_or_default();
                out.push(Item::new(Row::On(tag.id.clone(), on.subject.clone()).key(), "", format!("{} {said}", on.what)));
            }
        }
        out
    }

    fn spots(&self) -> Vec<Spot> {
        self.spots.clone()
    }

    fn cursor_key(&self) -> Option<String> {
        self.keys.get(self.cursor).cloned()
    }

    /// To its row; a thing of a folded tag: the tag unfolded first (the
    /// one gone to opened).
    fn move_to(&mut self, key: &str) {
        let folded = if self.keys.iter().any(|k| k == key) {
            None
        } else {
            self.tags.get().list().iter().find(|t| key.starts_with(&format!("{} ", t.id))).map(|t| t.id.clone())
        };
        if let Some(id) = folded {
            self.set_open(&id, true);
        }
        if let Some(at) = self.keys.iter().position(|k| k == key) {
            self.cursor = at;
            self.scrolling.show(at, Show::Middle);
        }
    }
}

impl ActionProvider for TagList {
    /// The shared moves, folds and its Enter: what a tag's rows are edited
    /// with is the shell's (`tag_at`).
    fn actions(&self) -> Vec<Action> {
        let mut out = crate::nav::list_moves();
        let on_thing = self.thing_here().is_some();
        let enter = match self.rows.get(self.cursor) {
            Some(Row::Tag(id)) if !self.open.contains(id) => Move::Enter
                .offer_if(self.tag_here(|t| !t.on.is_empty()).unwrap_or(false), "Unfold the tag")
                .detail("Its things show under it"),
            Some(Row::Tag(_)) => Move::Enter
                .offer_if(self.tag_here(|t| !t.on.is_empty()).unwrap_or(false), "To its first thing")
                .detail("The cursor on the first thing it's on"),
            _ => Move::Enter
                .offer_if(on_thing, "Go to it")
                .detail("Where it is, as a go-to goes (the jump list has where you were)")
                .on_item(Style::Button, "Go to it", Some(icon::ARROW_SQUARE_OUT)),
        };
        out.push(enter);
        out.push(Move::Leave.offer_if(on_thing, "Up to its tag").detail("The cursor on the tag the thing is under"));
        out.push(Move::Motion(Motion::NextSibling).offer_if(self.rows.iter().filter(|r| matches!(r, Row::Tag(_))).count() > 1, "Next tag"));
        out.push(
            Move::Motion(Motion::PrevSibling).offer_if(self.rows.iter().filter(|r| matches!(r, Row::Tag(_))).count() > 1, "Previous tag"),
        );
        let here = self.rows.get(self.cursor).map(|r| r.tag().clone());
        let name = here.as_ref().and_then(|id| self.tags.get().get(id).map(|t| t.name.clone()));
        out.extend(crate::nav::fold_here(crate::nav::FoldsHere::one(
            here.as_ref().zip(name.as_deref()).map(|(id, name)| (name, !self.open.contains(id))),
        )));
        let tags = self.tags.get().list().len();
        out.extend(crate::nav::fold_all((self.open.len() < tags, "Unfold every tag"), (!self.open.is_empty(), "Fold every tag")));
        out
    }

    fn run(&mut self, _action: &Action, _value: Option<String>, _cx: &mut RunCx<'_>) {}
}

impl Hosted for TagList {
    fn title(&self) -> Status {
        Status::new(format!("{} Tags", icon::TAG), Tone::Muted)
    }

    /// Its rows' buttons are the shell's tag actions: a tag's edits on its
    /// row, taking the tag off on a thing's.
    fn placed(&self) -> Vec<(String, Control)> {
        let button =
            |id: &str, caption: &str, icon: &'static str| (id.to_owned(), Control::new(At::Item, Style::Button, caption, Some(icon)));
        match self.rows.get(self.cursor) {
            Some(Row::Tag(_)) => vec![
                button("tag.rename", "Rename", icon::PENCIL_SIMPLE),
                button("tag.recolour", "Colour", icon::PALETTE),
                button("tag.note", "Note", icon::NOTE_PENCIL),
                button("tag.delete", "Delete", icon::TRASH),
            ],
            Some(Row::On(..)) => vec![button("tag.remove", "Take off", icon::X)],
            None => Vec::new(),
        }
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
