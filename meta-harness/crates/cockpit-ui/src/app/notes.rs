//! Notes on the thing at the cursor, one way for every kind of note (tags
//! now): what's at the cursor (`NotesHere`: the things there, and each
//! note on them), the gutter down a view that draws things (a mark per
//! note beside each thing it's on), and the names the panel's top line
//! says. A view says only which things are at an item (`Navigable::things`)
//! and where it draws them (`Navigable::draws_things`); it draws no notes.

use harness_contracts::{Subject, TagId};

use super::*;
use crate::nav::Things;
use crate::tags::first_line;

/// The gutter: a bar per tag this wide, this far apart, side by side (as
/// many as fit; the panel's top line names every tag on the cursor's).
const BAR_W: f32 = 2.5;
const BAR_GAP: f32 = 1.5;
const GUTTER_BARS: usize = 4;
pub(super) const GUTTER_W: f32 = BAR_GAP + GUTTER_BARS as f32 * (BAR_W + BAR_GAP);

/// A tag on what's at the cursor: what's said of it there (never the
/// whole tag: what it's on is read from the tags when it's needed).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TagHere {
    pub id: TagId,
    pub name: String,
    pub colour: u8,
    pub note: String,
    /// How many things it's on.
    pub count: usize,
    /// Which of the things there it's on (in a view of tags, the row's;
    /// none: the row is the tag's own).
    pub on: Option<Subject>,
}

impl TagHere {
    fn of(tag: &harness_contracts::Tag, on: Option<&Subject>) -> Self {
        Self { id: tag.id.clone(), name: tag.name.clone(), colour: tag.colour, note: tag.note.clone(), count: tag.on.len(), on: on.cloned() }
    }
}

/// What's at the cursor where you are: the things, the tags on them (the
/// row's first, in a view of tags), and in a view of tags, the row's tag.
pub(super) struct NotesHere {
    pub things: Things,
    pub tags: Vec<TagHere>,
    pub row: Option<TagId>,
}

impl CockpitApp {
    /// The notes on what's at the cursor where `here` is.
    pub(super) fn notes_here(&self, here: Here) -> NotesHere {
        let view = self.desk.view(here.kind);
        let key = view.cursor_key();
        let things = key.as_ref().map(|k| view.things(k)).unwrap_or_default();
        let shared = self.desk.shared.tags.get();
        let row = key.as_ref().and_then(|k| view.tag_at(k)).filter(|id| shared.get(id).is_some());
        let mut tags: Vec<TagHere> = Vec::new();
        for thing in &things.at {
            for (tag, subject) in shared.on(&thing.subject) {
                if !tags.iter().any(|t| t.id == tag.id) {
                    tags.push(TagHere::of(tag, Some(subject)));
                }
            }
        }
        // A view of tags: the row's tag first, on the row's thing (exactly).
        if let Some(tag) = row.as_ref().and_then(|id| shared.get(id)) {
            let on = things.at.first().filter(|thing| tag.on.iter().any(|o| o.subject.same_thing(&thing.subject))).map(|thing| &thing.subject);
            tags.retain(|t| t.id != tag.id);
            tags.insert(0, TagHere::of(tag, on));
        }
        NotesHere { things, tags, row }
    }

    /// The notes on what the cursor's on where `here` is: each name in its
    /// colour (the panel's top line says them).
    pub(super) fn notes_named(&self, here: Here) -> Vec<(String, egui::Color32)> {
        self.notes_here(here).tags.into_iter().filter(|t| t.on.is_some()).map(|t| (t.name, colors().tag(t.colour))).collect()
    }

    /// The gutter down `gutter` (beside the content): each noted thing the
    /// view draws (it says where; it draws no notes itself), a bar in its
    /// tag's colour beside its rows, a column per tag; more tags than
    /// columns, the rest share the last, muted. A bar's notes are named on
    /// hover, or (a finger) said on a long press (`on_gesture`).
    pub(super) fn draw_notes_gutter(&mut self, here: Here, ui: &egui::Ui, gutter: egui::Rect, content: egui::Rect) {
        let bars: Vec<(egui::Rect, String)> = {
            let tags = self.desk.shared.tags.get();
            let view = self.desk.view(here.kind);
            let Some(drawn) = view.draws_things() else { return };
            let theme = colors();
            let painter = ui.painter().with_clip_rect(gutter);
            let shown: Vec<(&harness_contracts::Tag, Vec<egui::Rect>)> = tags
                .list()
                .into_iter()
                .map(|tag| (tag, tag.on.iter().filter_map(|on| drawn.drawn_at(&on.subject)).filter(|r| content.intersects(*r)).collect::<Vec<_>>()))
                .filter(|(_, rows)| !rows.is_empty())
                .collect();
            let crowded = shown.len() > GUTTER_BARS;
            let mut bars = Vec::new();
            for (n, (tag, rows)) in shown.iter().enumerate() {
                let column = n.min(GUTTER_BARS - 1);
                let left = gutter.left() + BAR_GAP + column as f32 * (BAR_W + BAR_GAP);
                let colour = if crowded && column == GUTTER_BARS - 1 { theme.muted } else { theme.tag(tag.colour) };
                let said = if tag.note.is_empty() { tag.name.clone() } else { format!("{}: {}", tag.name, first_line(&tag.note)) };
                for row in rows {
                    let bar = egui::Rect::from_x_y_ranges(left..=left + BAR_W, row.top() + 1.0..=row.bottom() - 1.0);
                    painter.rect_filled(bar, BAR_W / 2.0, colour);
                    let hit = bar.expand2(egui::vec2(BAR_GAP / 2.0, 0.0));
                    match bars.iter_mut().find(|(r, _): &&mut (egui::Rect, String)| *r == hit) {
                        Some((_, names)) => *names = format!("{names}\n{said}"),
                        None => bars.push((hit, said.clone())),
                    }
                }
            }
            bars
        };
        self.note_marks.retain(|(at, _, _)| *at != here.at);
        for (n, (bar, said)) in bars.into_iter().enumerate() {
            let response = ui.interact(bar, ui.id().with(("note_mark", here.kind, n)), egui::Sense::hover());
            if response.hovered() {
                response.on_hover_text(said.clone());
            }
            self.note_marks.push((here.at, bar, said));
        }
    }
}
