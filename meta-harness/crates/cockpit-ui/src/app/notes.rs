//! Notes on the thing at the cursor, one way for every kind of note (tags,
//! discussions): what's at the cursor (`NotesHere`: the things there, and
//! each note on them), the gutter down a view that draws things (a bar per
//! tag beside each thing it's on, a speech-bubble mark per thing with
//! discussions, coloured by whose move it is), and what the panel's top
//! line says. A view says only which things are at an item
//! (`Navigable::things`) and where it draws them (`Navigable::draws_things`);
//! it draws no notes.

use std::collections::HashMap;

use harness_contracts::{Discussion, DiscussionStatus, Move, Subject, TagId};

use super::*;
use crate::kind::BufferKind;
use crate::nav::Things;
use crate::conversation::first_line;

/// The gutter's columns of tag bars (as many tags as fit; the panel's top
/// line names every tag on the cursor's thing); the rest share the last.
const GUTTER_BARS: usize = 4;

/// How wide the gutter is: its tag bars, then the discussions' marks
/// (sizes from the theme).
pub(super) fn gutter_width() -> f32 {
    let look = crate::theme::look();
    look.gutter_gap + GUTTER_BARS as f32 * (look.gutter_bar + look.gutter_gap) + look.gutter_mark + look.gutter_gap
}

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
/// row's first, in a view of tags), in a view of tags the row's tag, and
/// the discussions on them.
pub(super) struct NotesHere {
    pub things: Things,
    pub tags: Vec<TagHere>,
    pub row: Option<TagId>,
    pub discussions: Vec<Standing>,
}

/// Where a discussion at the cursor stands (what's said of it there; it
/// isn't copied whole).
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Standing {
    pub id: harness_contracts::DiscussionId,
    pub whose: Move,
    pub settled: bool,
}

impl Standing {
    fn of(d: &Discussion) -> Self {
        Self { id: d.id.clone(), whose: d.whose_move(), settled: d.status == DiscussionStatus::Settled }
    }

    /// Which goes first of those on one thing: yours, an agent's, settled.
    fn urgency(&self) -> u8 {
        match self.whose {
            Move::User => 0,
            Move::Agent(_) => 1,
            Move::Nobody => 2,
        }
    }

    /// Whose move it is, as a mark's colour.
    fn colour(&self) -> egui::Color32 {
        match self.whose {
            Move::User => colors().warn,
            Move::Agent(_) => colors().info,
            Move::Nobody => colors().faint,
        }
    }
}

/// Discussions, in a few words (`2 discussions, 1 waiting on you`).
pub(super) fn discussions_said(on: &[Standing]) -> String {
    let n = on.len();
    let yours = on.iter().filter(|d| d.whose == Move::User).count();
    let open = on.iter().filter(|d| !d.settled).count();
    let mut said = if n == 1 { "1 discussion".to_owned() } else { format!("{n} discussions") };
    if yours > 0 {
        said.push_str(&format!(", {yours} waiting on you"));
    } else if open == 0 {
        said.push_str(if n == 1 { ", settled" } else { ", all settled" });
    }
    said
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
        // The discussions on the things there (on one, in one, or one in
        // it), each once; not one that's itself there (a discussion's own
        // view, its row in the plan).
        let all = self.desk.shared.discussions.get();
        let mut discussions: Vec<Standing> = Vec::new();
        for thing in &things.at {
            for d in all.on(&thing.subject) {
                let own = things.at.iter().any(|t| matches!(&t.subject, Subject::Discussion { id } if *id == d.id));
                if !own && !discussions.iter().any(|x| x.id == d.id) {
                    discussions.push(Standing::of(d));
                }
            }
        }
        NotesHere { things, tags, row, discussions }
    }

    /// The notes on what the cursor's on where `here` is, as the panel's
    /// top line says them: each tag's name in its colour, then its
    /// discussions in a few words, in the colour of whose move it is.
    pub(super) fn notes_named(&self, here: Here) -> Vec<(crate::hosted::NoteMark, String, egui::Color32)> {
        let at = self.notes_here(here);
        let mut out: Vec<_> = at.tags.into_iter().filter(|t| t.on.is_some()).map(|t| (crate::hosted::NoteMark::Tag, t.name, colors().tag(t.colour))).collect();
        let mut on = at.discussions;
        on.sort_by_key(Standing::urgency);
        if let Some(first) = on.first() {
            out.push((crate::hosted::NoteMark::Discussions, discussions_said(&on), first.colour()));
        }
        out
    }
}

/// What's noted, as the gutter draws it: each tag's things, each
/// discussion; made again only when the tags or discussions change (not
/// every frame).
#[derive(Default)]
pub(super) struct Noted {
    /// The tags' and discussions' versions it was made at.
    of: Option<(u64, u64)>,
    /// Each tag (in the tags' order): its colour, what's said of it, its things.
    tags: Vec<(TagId, u8, String, Vec<Subject>)>,
    discussions: Vec<Discussion>,
    /// Each gutter's columns: the tag in each while it's shown there (a
    /// tag keeps its column as things scroll; a column is free again once
    /// its tag is off screen).
    columns: HashMap<(crate::keymap::Host, BufferKind), Vec<Option<TagId>>>,
}

impl Noted {
    fn refresh(&mut self, tags: &crate::tags::Tags, discussions: &crate::discussions::Discussions) {
        let now = (tags.version(), discussions.version());
        if self.of == Some(now) {
            return;
        }
        self.tags = tags
            .list()
            .into_iter()
            .map(|tag| {
                let said = if tag.note.is_empty() { tag.name.clone() } else { format!("{}: {}", tag.name, first_line(&tag.note)) };
                (tag.id.clone(), tag.colour, said, tag.on.iter().map(|on| on.subject.clone()).collect())
            })
            .collect();
        self.discussions = discussions.all().cloned().collect();
        self.of = Some(now);
    }

    /// The column of each tag in `shown`, in the gutter at `at`: one it
    /// had keeps it, a new one takes the first free.
    fn columns(&mut self, at: (crate::keymap::Host, BufferKind), shown: &[TagId]) -> Vec<usize> {
        let columns = self.columns.entry(at).or_default();
        for column in columns.iter_mut() {
            if column.as_ref().is_some_and(|id| !shown.contains(id)) {
                *column = None;
            }
        }
        shown
            .iter()
            .map(|id| match columns.iter().position(|c| c.as_ref() == Some(id)) {
                Some(n) => n,
                None => match columns.iter().position(Option::is_none) {
                    Some(n) => {
                        columns[n] = Some(id.clone());
                        n
                    }
                    None => {
                        columns.push(Some(id.clone()));
                        columns.len() - 1
                    }
                },
            })
            .collect()
    }
}

impl CockpitApp {
    /// The notes on the things the view draws (it says where; it draws no
    /// notes itself), down `gutter` beside the content, or (a view whose
    /// things aren't rows: no gutter) on each thing's top right corner: a
    /// bar in its tag's colour beside its rows (on a card, a dot), a column
    /// per tag (kept while the tag's on screen; more tags than columns
    /// share the last, muted), a tag on a thing and on one inside it drawn
    /// as two (the outer faint, the inner solid on it); and a speech-bubble
    /// mark where a thing has discussions, in the colour of whose move it
    /// is. A mark's notes are named on hover, or (a finger) said on a long
    /// press (`on_gesture`); a discussion's mark clicked opens it.
    pub(super) fn draw_notes(&mut self, here: Here, ui: &egui::Ui, gutter: Option<egui::Rect>, content: egui::Rect) {
        let look = crate::theme::look();
        let theme = colors();
        type Bubble = (egui::Rect, String, Vec<harness_contracts::DiscussionId>);
        let (bars, bubbles): (Vec<(egui::Rect, String)>, Vec<Bubble>) = {
            let view = self.desk.view(here.kind);
            let Some(drawn) = view.draws_things() else { return };
            let painter = ui.painter().with_clip_rect(gutter.unwrap_or(content));
            // Where a column's bar and a thing's mark go: in the gutter
            // (beside the rows), or on the thing (its top right corner, on
            // its edge: over none of what it says).
            let step = look.gutter_bar + look.gutter_gap;
            let bar_at = |column: usize, row: egui::Rect| match gutter {
                Some(gutter) => {
                    let left = gutter.left() + look.gutter_gap + column as f32 * step;
                    let top = row.top() + 1.0;
                    egui::Rect::from_x_y_ranges(left..=left + look.gutter_bar, top..=(row.bottom() - 1.0).max(top + 1.0))
                }
                None => {
                    let size = look.gutter_bar * 2.0;
                    let right = row.right() - look.gutter_mark - look.gutter_gap * 2.0 - column as f32 * (size + look.gutter_gap);
                    egui::Rect::from_center_size(egui::pos2(right - size / 2.0, row.top()), egui::vec2(size, size))
                }
            };
            let mark_at = |row: egui::Rect| match gutter {
                Some(gutter) => egui::Rect::from_min_size(egui::pos2(gutter.left() + look.gutter_gap + GUTTER_BARS as f32 * step, row.top() + 1.0), egui::vec2(look.gutter_mark, look.gutter_mark)),
                None => egui::Rect::from_center_size(egui::pos2(row.right() - look.gutter_gap - look.gutter_mark / 2.0, row.top()), egui::vec2(look.gutter_mark, look.gutter_mark)),
            };
            let noted = &mut self.noted;
            noted.refresh(&self.desk.shared.tags.get(), &self.desk.shared.discussions.get());
            let shown: Vec<(TagId, u8, String, Vec<egui::Rect>)> = noted
                .tags
                .iter()
                .map(|(id, colour, said, on)| (id.clone(), *colour, said.clone(), on.iter().filter_map(|s| drawn.drawn_at(s)).filter(|r| content.intersects(*r)).collect::<Vec<_>>()))
                .filter(|(.., rows)| !rows.is_empty())
                .collect();
            let ids: Vec<TagId> = shown.iter().map(|(id, ..)| id.clone()).collect();
            let columns = noted.columns((here.at, here.kind), &ids);
            let crowded = columns.iter().any(|c| *c >= GUTTER_BARS);
            let mut bars = Vec::new();
            for ((_, colour, said, rows), at) in shown.iter().zip(columns) {
                let column = at.min(GUTTER_BARS - 1);
                let colour = if crowded && column == GUTTER_BARS - 1 { theme.muted } else { theme.tag(*colour) };
                // A row with another of the tag's inside it (a command, a
                // line of it): faint, drawn first, so the inner one's bar
                // shows on it.
                let mut rows = rows.clone();
                rows.sort_by(|a, b| b.height().total_cmp(&a.height()));
                for row in &rows {
                    let outer = rows.iter().any(|inner| inner != row && row.contains_rect(*inner));
                    let bar = bar_at(column, *row);
                    painter.rect_filled(bar, bar.width() / 2.0, if outer { colour.gamma_multiply(0.4) } else { colour });
                    let hit = bar.expand2(egui::vec2(look.gutter_gap / 2.0, 0.0));
                    match bars.iter_mut().find(|(r, _): &&mut (egui::Rect, String)| *r == hit) {
                        Some((_, names)) => *names = format!("{names}\n{said}"),
                        None => bars.push((hit, said.clone())),
                    }
                }
            }
            // Discussions: a mark at the top of each thing with some.
            let mut marks: Vec<(egui::Rect, Vec<&Discussion>)> = Vec::new();
            for d in &noted.discussions {
                let Some(row) = drawn.drawn_at(&d.on.subject).filter(|r| content.intersects(*r)) else { continue };
                let mark = mark_at(row);
                match marks.iter_mut().find(|(r, _)| r.intersects(mark)) {
                    Some((_, on)) => on.push(d),
                    None => marks.push((mark, vec![d])),
                }
            }
            let mut bubbles = Vec::new();
            for (mark, mut on) in marks {
                on.sort_by_key(|d| Standing::of(d).urgency());
                let standing: Vec<Standing> = on.iter().map(|d| Standing::of(d)).collect();
                painter.text(mark.center(), egui::Align2::CENTER_CENTER, egui_phosphor::fill::CHAT_CIRCLE, crate::theme::solid_font(ui, look.gutter_mark), standing[0].colour());
                let each = on.iter().map(|d| format!("{} {}: {}", d.id, d.kind.words(), first_line(d.asking()))).collect::<Vec<_>>().join("\n");
                bubbles.push((mark, format!("{}\n{each}", discussions_said(&standing)), standing.into_iter().map(|d| d.id).collect()));
            }
            (bars, bubbles)
        };
        self.note_marks.retain(|(at, _, _)| *at != here.at);
        // For a finger: a long press anywhere along a row of the gutter
        // says every note there; a mark is tapped within a finger's reach
        // of it (in the gutter, never over the content).
        let reach = look.gutter_gap * 4.0;
        let within = |r: egui::Rect| match gutter {
            Some(g) => r.intersect(egui::Rect::from_x_y_ranges(g.x_range(), egui::Rangef::EVERYTHING)),
            None => r,
        };
        for (n, (bar, said)) in bars.into_iter().enumerate() {
            let response = ui.interact(bar, ui.id().with(("note_mark", here.kind, n)), egui::Sense::hover());
            if response.hovered() {
                response.on_hover_text(said.clone());
            }
            let row = gutter.map_or(bar.expand(reach), |g| egui::Rect::from_x_y_ranges(g.x_range(), bar.y_range()));
            self.note_marks.push((here.at, row, said));
        }
        for (n, (mark, said, ids)) in bubbles.into_iter().enumerate() {
            let response = ui.interact(within(mark.expand(reach)), ui.id().with(("discussion_mark", here.kind, n)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if response.clicked() {
                // As its action (one: it opens; several: which, first).
                // (The actions now offered are made again: they're of the mark.)
                let one = (ids.len() == 1).then(|| ids[0].to_string());
                self.desk.discussions_here = ids;
                self.forget_frame();
                self.run_by_id("discuss.open", one, ui.ctx());
            } else if response.hovered() {
                response.on_hover_text(said.clone());
            }
            self.note_marks.push((here.at, within(mark.expand(reach)), said));
        }
    }
}
