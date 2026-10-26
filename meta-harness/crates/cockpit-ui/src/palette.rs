//! The action list: what can be done where you are (`:`), or everything
//! (`?`), found by typing; then (if the action takes one) its parameter: a
//! filterable choice list, a text field, or a search whose results the
//! action's view looks up as you type. Opens with the input focused, so it
//! works for typing, dictation and touch alike. Matching is the same
//! everywhere (`arch_search::rank`).

use egui::{Frame, RichText, Stroke};

use crate::actions::{Bound, Choice, Invocation, Offered, Origin, Param};
use crate::keymap::KeymapConfig;
use crate::pick_list::{Listing, every_row_picks};
use crate::sheet::{Item, Listed, Reach, Sheet};
use crate::theme::colors;

/// Where it is: the action list (as far as it reaches), or an action's
/// value, picked from the list (`from`: the list as it was, the way back)
/// or asked for straight away.
#[derive(Clone, Debug)]
enum Stage {
    Actions {
        reach: Reach,
    },
    Choose {
        action: Box<Bound>,
        from: Option<Reach>,
    },
    Text {
        action: Box<Bound>,
        from: Option<Reach>,
    },
    /// Written in the typing box (many lines).
    Writing {
        action: Box<Bound>,
        from: Option<Reach>,
    },
    /// Looked up as it's typed: the action is read afresh each frame (its
    /// results come in after the query goes out).
    Search {
        action: Box<Bound>,
        from: Option<Reach>,
    },
}

/// What a search stage is asking its view for.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchAsk {
    pub origin: Origin,
    pub action: String,
    pub query: String,
}

pub struct Palette {
    open: bool,
    stage: Stage,
    filter: String,
    /// The rows of whatever it lists, and the one the keys are on.
    list: crate::pick_list::PickList,
    /// A search query to hand to its view (taken by the app each frame).
    ask: Option<SearchAsk>,
    /// Enter came while a search's rows were of what was typed before (in
    /// the frame it changed, or before its answers came): it's taken once
    /// the rows are of what's typed.
    enter_waiting: bool,
    /// What it says (why a row picked can't run here), until the next key.
    said: Option<String>,
    /// The query a search's rows were of last frame (they changed: the
    /// highlight starts again).
    rows_of: Option<String>,
    /// The tallest it's been while open: it never gets shorter (going
    /// from a list to a text field, say), so nothing under it jumps.
    tallest: f32,
    /// How many times it's been opened: what's picked while it's open
    /// (what a tag goes on) is for this opening only.
    opened: u64,
    /// Its field was given text to edit: the caret goes after it.
    caret_to_end: bool,
    /// What's written, for an action that takes writing.
    writing: crate::conversation::TypingBox,
    /// A trigger character was just typed where a word starts in what's
    /// written (taken by the app: it opens the trigger's picker).
    trigger: Option<char>,
    /// What's written, set aside while another action picks what goes in
    /// after it (it comes back with that put in).
    held: Option<(Stage, crate::conversation::TypingBox)>,
    /// The list ranked for what's typed, kept until the text or the list
    /// changes (not ranked again every frame it's open).
    ranked: Ranked,
    /// Opened for what's being written (a trigger's picker): left without
    /// a pick, what was typed in its field goes back after the trigger.
    gives_back: bool,
    given_back: Option<String>,
    /// Where it was drawn this frame (none: it wasn't).
    drawn_at: Option<egui::Rect>,
}

/// Rows ranked for a text, remembered with what they were ranked from.
#[derive(Default)]
struct Ranked {
    key: Option<u64>,
    rows: Vec<usize>,
}

impl Ranked {
    /// The rows (indexes into the list) for `filter`, ranked only when it
    /// or the list (`fingerprint`: what of it the ranking reads) changed.
    fn of(&mut self, filter: &str, fingerprint: impl std::hash::Hash, haystacks: impl FnOnce() -> Vec<String>) -> &[usize] {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        filter.hash(&mut hasher);
        fingerprint.hash(&mut hasher);
        let key = hasher.finish();
        if self.key != Some(key) {
            self.rows = arch_search::rank(filter, &haystacks(), arch_search::Matching::shared()).into_iter().map(|r| r.index).collect();
            self.key = Some(key);
        }
        &self.rows
    }
}

impl Palette {
    /// The area it's drawn in.
    fn area() -> egui::Id {
        egui::Id::new("command_palette")
    }

    /// Where it was drawn this frame, while it's open.
    pub fn drawn_at(&self) -> Option<egui::Rect> {
        self.drawn_at.filter(|_| self.open)
    }

    /// Where it was last drawn (none: it hasn't been).
    #[cfg(test)]
    pub fn drawn(ctx: &egui::Context) -> Option<egui::Rect> {
        ctx.memory(|m| m.area_rect(Self::area()))
    }

    /// The row the keys are on.
    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.list.highlight()
    }

    /// What's typed in its field.
    #[cfg(test)]
    pub fn filter(&self) -> &str {
        &self.filter
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            stage: Stage::Actions { reach: Reach::Here },
            filter: String::new(),
            list: Default::default(),
            ask: None,
            enter_waiting: false,
            said: None,
            rows_of: None,
            tallest: 0.0,
            opened: 0,
            caret_to_end: false,
            writing: Default::default(),
            trigger: None,
            held: None,
            ranked: Ranked::default(),
            gives_back: false,
            given_back: None,
            drawn_at: None,
        }
    }
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Opens at the action list, as far as `reach`.
    pub fn open(&mut self, reach: Reach) {
        *self = Self { open: true, opened: self.opened + 1, stage: Stage::Actions { reach }, ..Self::default() };
    }

    /// Which opening this is, or (closed) was last: what's picked in it
    /// is run just after it closes.
    pub fn opening(&self) -> u64 {
        self.opened
    }

    /// The search open now: its action's origin and id.
    pub fn searching(&self) -> Option<(Origin, &str)> {
        match &self.stage {
            Stage::Search { action, .. } if self.open => Some((action.origin, action.action.id.as_str())),
            _ => None,
        }
    }

    /// The action whose value it's picking now (none: it's closed, or
    /// listing actions).
    pub fn picking(&self) -> Option<&str> {
        match &self.stage {
            Stage::Choose { action, .. } | Stage::Text { action, .. } | Stage::Writing { action, .. } | Stage::Search { action, .. }
                if self.open =>
            {
                Some(&action.action.id)
            }
            _ => None,
        }
    }

    /// Opens straight at an action's parameter (e.g. from a leader key).
    pub fn open_for(&mut self, action: &Bound) {
        self.open(Reach::Here);
        self.enter(action.clone(), None);
    }

    /// Its keys where it is, as its line under it says them (and the key
    /// bar, while it has the keys): `find` gives an action by its origin
    /// and id (what the cycle key changes a value to act on).
    pub fn keys_said<'a>(&self, keymap: &KeymapConfig, find: impl Fn(Origin, &str) -> Option<&'a Bound>) -> Vec<String> {
        let cycle = match &self.stage {
            Stage::Choose { action, .. } | Stage::Text { action, .. } | Stage::Writing { action, .. } | Stage::Search { action, .. } => {
                action.action.cycle.as_deref().and_then(|id| find(action.origin, id))
            }
            Stage::Actions { .. } => None,
        };
        keys_said(&self.stage, cycle, self.enter_waiting, keymap)
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    fn enter(&mut self, action: Bound, from: Option<Reach>) {
        self.filter.clear();
        self.list.reset();
        (self.enter_waiting, self.rows_of) = (false, None);
        let action = Box::new(action);
        self.stage = match &action.action.param {
            Param::Text { start, .. } => {
                self.filter = start.clone();
                self.caret_to_end = true;
                Stage::Text { action, from }
            }
            Param::Writing { start, .. } => {
                self.writing = Default::default();
                self.writing.restore(start);
                Stage::Writing { action, from }
            }
            Param::Search { .. } => {
                self.ask = Some(SearchAsk { origin: action.origin, action: action.action.id.clone(), query: String::new() });
                Stage::Search { action, from }
            }
            _ => {
                // Starting on what's chosen now, as a setting's list does.
                self.list = crate::pick_list::PickList::at(action.action.param.choices().iter().position(|c| c.current).unwrap_or(0));
                Stage::Choose { action, from }
            }
        };
    }

    /// A trigger typed in what's written (once).
    pub fn take_trigger(&mut self) -> Option<char> {
        self.trigger.take()
    }

    /// What's written is set aside while `pick` asks for what goes in
    /// after it.
    pub fn hold_for(&mut self, pick: &Bound) {
        let stage = std::mem::replace(&mut self.stage, Stage::Actions { reach: Reach::Here });
        self.held = Some((stage, std::mem::take(&mut self.writing)));
        self.open = true;
        self.enter(pick.clone(), None);
    }

    /// It's open at an action's writing that takes links (`#`, `@`).
    pub fn writes_links(&self) -> bool {
        self.open
            && matches!(&self.stage, Stage::Writing { action, .. } if matches!(action.action.param, Param::Writing { links: true, .. }))
    }

    /// Its typing box as it is, while what's written takes links.
    pub fn writing(&self) -> Option<&crate::conversation::TypingBox> {
        self.writes_links().then_some(&self.writing)
    }

    /// Its typing box, while what's written is typed there (its links are
    /// the shell's to colour and check).
    pub fn writing_box(&mut self) -> Option<&mut crate::conversation::TypingBox> {
        (self.open && matches!(self.stage, Stage::Writing { .. })).then_some(&mut self.writing)
    }

    /// Something is being picked for what's written here (set aside).
    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// What was set aside comes back (with `text` put in after it).
    pub fn put_back(&mut self) -> Option<&mut crate::conversation::TypingBox> {
        // (What it's back to was picked for: nothing to give back.)
        self.gives_back = false;
        let (stage, writing) = self.held.take()?;
        (self.stage, self.writing, self.open) = (stage, writing, true);
        Some(&mut self.writing)
    }

    /// Opened for what's being written: left without a pick, what's typed
    /// in its field goes back after the trigger.
    pub fn give_back_typing(&mut self) {
        self.gives_back = true;
    }

    /// What was typed in a trigger's picker left without a pick, for the
    /// box it was opened from (once).
    pub fn take_given_back(&mut self) -> Option<String> {
        self.given_back.take()
    }

    /// Left without a pick: what was typed goes back to what's written (set
    /// aside here, or the box it was opened from); then back.
    fn leave_giving_back(&mut self) {
        let typed = std::mem::take(&mut self.filter);
        self.gives_back = false;
        match self.put_back() {
            Some(writing) => writing.put(&typed),
            None => {
                self.given_back = (!typed.is_empty()).then_some(typed);
                self.close();
            }
        }
    }

    /// The query a search stage wants looked up, once per change.
    pub fn take_search(&mut self) -> Option<SearchAsk> {
        self.ask.take()
    }

    fn back(&mut self) {
        if self.gives_back {
            self.leave_giving_back();
            return;
        }
        if self.held.is_some() {
            self.put_back();
            return;
        }
        match &self.stage {
            Stage::Choose { from: Some(reach), .. }
            | Stage::Text { from: Some(reach), .. }
            | Stage::Writing { from: Some(reach), .. }
            | Stage::Search { from: Some(reach), .. } => {
                self.stage = Stage::Actions { reach: *reach };
                self.filter.clear();
                self.list.reset();
                self.enter_waiting = false;
            }
            _ => self.close(),
        }
    }

    /// Renders it; returns an invocation when the user picks something.
    pub fn render(&mut self, ctx: &egui::Context, offered: &[Offered], sheet: &Sheet, keymap: &KeymapConfig) -> Option<Invocation> {
        self.drawn_at = None;
        if !self.open {
            return None;
        }
        let screen = ctx.content_rect();
        let narrow = screen.width() < 600.0;
        let width = (screen.width() - 32.0).min(620.0);
        let pos =
            if narrow { egui::pos2(16.0, (screen.max.y - 420.0).max(60.0)) } else { egui::pos2((screen.width() - width) / 2.0, 90.0) };
        let list_height = (screen.max.y - pos.y - 110.0).clamp(120.0, 360.0);

        // Its keys are its own: egui moves the keyboard on Tab and the
        // arrows unless the field has had it a pass already; not here.
        ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        let moved = ctx.input_mut(|i| keymap.list.moved(i));
        let cycled = ctx.input_mut(|i| keymap.list.cycled(i));
        let (enter, escape) = ctx.input(|i| (keymap.list.picked(i), keymap.back.pressed(i)));
        if escape {
            self.back();
            return None;
        }
        if moved.is_some() || cycled || enter {
            self.said = None;
            // What it says changes with it; the key bar, drawn before it,
            // says it next frame, at once.
            ctx.request_repaint();
        }
        let find = |origin: Origin, id: &str| offered.iter().map(|o| &o.bound).find(|b| b.origin == origin && b.action.id == id);

        let mut picked: Option<Invocation> = None;
        let mut leave = false;
        let mut next_stage: Option<(Bound, Option<Reach>)> = None;

        // The action picked for, read afresh each frame where it's meant to
        // change: a search's results come after it's asked; one with a cycle
        // key acts on what that picks. (Any other stays what it was asked
        // for: an edit of a tag deleted meanwhile is of that tag still.)
        let search = matches!(self.stage, Stage::Search { .. });
        if let Stage::Search { action, .. } | Stage::Choose { action, .. } | Stage::Text { action, .. } | Stage::Writing { action, .. } =
            &mut self.stage
            && (search || action.action.cycle.is_some())
            && let Some(fresh) = find(action.origin, &action.action.id)
        {
            **action = fresh.clone();
        }
        // Enter on rows of what was typed before waits for those of what's
        // typed, unless a row was chosen on them (a key, the pointer): then
        // that row.
        let stale = matches!(&self.stage, Stage::Search { action, .. } if matches!(&action.action.param, Param::Search { of, .. } if of.as_deref() != Some(self.filter.as_str())));
        let waiting = stale && !self.list.chosen();
        let enter = if waiting {
            self.enter_waiting |= enter;
            false
        } else {
            enter || std::mem::take(&mut self.enter_waiting)
        };
        // The action list's cycle key: everything, or what applies here.
        if cycled && let Stage::Actions { reach } = self.stage {
            self.stage = Stage::Actions { reach: reach.other() };
            self.list.reset();
        }
        let stage = self.stage.clone();
        // While a value is picked: what the cycle key changes it to act on.
        let cycle = match &stage {
            Stage::Choose { action, .. } | Stage::Text { action, .. } | Stage::Writing { action, .. } | Stage::Search { action, .. } => {
                action.action.cycle.as_ref().and_then(|id| find(action.origin, id))
            }
            Stage::Actions { .. } => None,
        };
        // Cycled: it runs, and the picker stays (its top line says the new).
        let cycling = cycled && cycle.is_some();
        if let Some(bound) = cycle.filter(|_| cycled) {
            picked = Some(invocation(bound, None));
        }

        let shown = egui::Area::new(Self::area())
            .fixed_pos(pos)
            // Whole from its first frame: no fading in.
            .fade_in(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(colors().overlay)
                    .stroke(Stroke::new(crate::theme::look().line_thin, colors().focus))
                    .corner_radius(crate::theme::look().radius_xl)
                    .inner_margin(crate::theme::margin(crate::theme::look().space_ml))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        let title = match &stage {
                            Stage::Actions { reach: Reach::Everything } => "Every action and key".to_owned(),
                            Stage::Actions { reach: Reach::Here } => "What can be done here".to_owned(),
                            Stage::Choose { action, .. }
                            | Stage::Text { action, .. }
                            | Stage::Writing { action, .. }
                            | Stage::Search { action, .. } => action.action.about.clone().unwrap_or_else(|| action.action.label.clone()),
                        };
                        ui.add(
                            egui::Label::new(RichText::new(title).strong().size(crate::theme::look().text_label).color(colors().info))
                                .truncate(),
                        );
                        let hint = match &stage {
                            Stage::Text { action, .. } | Stage::Writing { action, .. } | Stage::Search { action, .. } => {
                                match &action.action.param {
                                    Param::Text { hint, .. } | Param::Writing { hint, .. } | Param::Search { hint, .. } => hint.clone(),
                                    _ => String::new(),
                                }
                            }
                            _ => "Type to filter…".to_owned(),
                        };
                        // Writing: the typing box every conversation has; Enter sends it.
                        if let Stage::Writing { action, .. } = &stage {
                            let typed = self.writing.draw(ui, egui::Id::new("palette_writing"), 4, &hint, true);
                            // Sent by its key, or (a phone's keyboard may have none) its button.
                            let send = ui.button(format!("{} Send", egui_phosphor::regular::PAPER_PLANE_RIGHT)).clicked();
                            let links = matches!(action.action.param, Param::Writing { links: true, .. });
                            if typed.sent.is_some() || (send && !self.writing.text.trim().is_empty()) {
                                let written = self.writing.take();
                                // One that takes links: its message whole; else its text.
                                let value = if links { serde_json::to_string(&written).unwrap_or_default() } else { written.text };
                                picked = Some(invocation(action, Some(value)));
                            }
                            // A trigger only where links are taken.
                            if links {
                                self.trigger = typed.trigger.or(self.trigger);
                            }
                        }
                        let edit = if matches!(stage, Stage::Writing { .. }) {
                            None
                        } else {
                            // The keyboard's, before it's drawn (the key of the pass
                            // is typed in it), and only when it hasn't it.
                            let id = egui::Id::new("palette_field");
                            if ui.memory(|m| m.focused() != Some(id)) {
                                ui.memory_mut(|m| m.request_focus(id));
                            }
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut self.filter)
                                    .id(id)
                                    .lock_focus(true)
                                    .hint_text(hint)
                                    .desired_width(f32::INFINITY),
                            );
                            if std::mem::take(&mut self.caret_to_end) {
                                crate::conversation::caret_to_end(ui.ctx(), edit.id, &self.filter);
                            }
                            Some(edit)
                        };
                        let mut enter = enter;
                        if edit.as_ref().is_some_and(egui::Response::changed) {
                            self.list.reset();
                            self.said = None;
                            if let Stage::Search { action, .. } = &stage {
                                self.ask =
                                    Some(SearchAsk { origin: action.origin, action: action.action.id.clone(), query: self.filter.clone() });
                                // Results of what was typed before: Enter waits for these.
                                // (An Enter before this typing is dropped: it
                                // was for what's typed no more.)
                                self.enter_waiting = enter;
                                enter = false;
                                ctx.request_repaint();
                            }
                        }
                        ui.add_space(crate::theme::look().space_s);

                        match &stage {
                            Stage::Actions { reach } => {
                                let listed = Listed::of(offered, sheet, *reach, &self.filter);
                                let fingerprint = listed.fingerprint();
                                let haystacks = || (0..listed.len()).map(|e| listed.haystack(e)).collect();
                                let lines =
                                    listed.lines(self.ranked.of(&self.filter, (&fingerprint, *reach == Reach::Everything), haystacks));
                                let listing = Listing {
                                    id: "palette_actions",
                                    height: list_height,
                                    len: lines.len(),
                                    details: true,
                                    empty: "No matching actions",
                                };
                                if let Some(i) =
                                    self.list.show(ui, listing, moved, enter, |i| listed.kind(lines[i]), |i| listed.row(lines[i]))
                                    && let crate::sheet::Line::Entry(e) = lines[i]
                                {
                                    match listed.item(e) {
                                        Item::Line { number, bound, .. } => picked = Some(invocation(bound, Some(number.to_string()))),
                                        // Refused where it doesn't apply: why not is said (as its key would).
                                        Item::Action(o) => match (o.refused(), &o.bound.action.param) {
                                            (Some(why), _) => self.said = Some(why),
                                            (None, Param::None) => picked = Some(invocation(&o.bound, None)),
                                            (None, _) => next_stage = Some((o.bound.clone(), Some(*reach))),
                                        },
                                        Item::Inform(_) => {}
                                    }
                                }
                            }
                            Stage::Choose { action, .. } => {
                                let choices: Vec<Choice> = action.action.param.choices();
                                let fingerprint: Vec<(&str, &str)> = choices.iter().map(|c| (c.label.as_str(), c.value.as_str())).collect();
                                let haystacks = || fingerprint.iter().map(|(label, value)| format!("{label} {value}")).collect();
                                let ranked: Vec<&Choice> =
                                    self.ranked.of(&self.filter, &fingerprint, haystacks).iter().map(|i| &choices[*i]).collect();
                                let details = ranked.iter().any(|c| c.detail.is_some());
                                let row = |i: usize| crate::pick_list::Row::of_choice(ranked[i]);
                                let listing = Listing {
                                    id: "palette_choices",
                                    height: list_height,
                                    len: ranked.len(),
                                    details,
                                    empty: "Nothing to choose",
                                };
                                if let Some(i) = self.list.show(ui, listing, moved, enter, every_row_picks, row) {
                                    picked = Some(invocation(action, Some(ranked[i].value.clone())));
                                }
                            }
                            Stage::Search { action, .. } => {
                                let (found, of) = match &action.action.param {
                                    Param::Search { found, of, .. } => (found.clone(), of.clone()),
                                    _ => (Vec::new(), None),
                                };
                                // A space typed (no name a trigger picks has one): what's
                                // typed is text, back where it's written.
                                if self.gives_back && self.filter.ends_with(' ') {
                                    leave = true;
                                }
                                if of != self.rows_of {
                                    self.rows_of.clone_from(&of);
                                    self.list.reset();
                                }
                                // No rows: said of the query they're of (rows of an
                                // earlier one stay, said as they were, until the
                                // new ones come; a search opened has none: each is
                                // cleared when its picker closes).
                                let empty = match of.as_deref().map(str::trim) {
                                    Some(query) if !query.is_empty() => "Nothing found",
                                    _ if self.filter.trim().is_empty() => "Type to search",
                                    _ => "Looking…",
                                };
                                let details = found.iter().any(|c| c.detail.is_some());
                                let row = |i: usize| crate::pick_list::Row::of_choice(&found[i]);
                                let listing = Listing { id: "palette_found", height: list_height, len: found.len(), details, empty };
                                if let Some(i) = self.list.show(ui, listing, moved, enter, every_row_picks, row) {
                                    picked = Some(invocation(action, Some(found[i].value.clone())));
                                }
                            }
                            // Sent from its box (above).
                            Stage::Writing { .. } => {}
                            Stage::Text { action, .. } => {
                                if enter && !self.filter.trim().is_empty() {
                                    picked = Some(invocation(action, Some(self.filter.trim().to_owned())));
                                }
                            }
                        }
                        // Its keys, on one line under it; in their place, until
                        // the next key, why a row picked can't run (the box
                        // keeps its size).
                        let line = match &self.said {
                            Some(said) => RichText::new(said).size(crate::theme::look().text_small).color(colors().warn),
                            None => RichText::new(keys_said(&stage, cycle, self.enter_waiting, keymap).join(" · "))
                                .size(crate::theme::look().text_small)
                                .color(colors().muted),
                        };
                        ui.add(egui::Label::new(line).wrap());
                        let used = ui.min_rect().height();
                        self.tallest = self.tallest.max(used);
                        if self.tallest > used {
                            ui.allocate_space(egui::vec2(0.0, self.tallest - used - ui.spacing().item_spacing.y));
                        }
                    });
            });
        self.drawn_at = Some(shown.response.rect);

        if let Some((action, from_list)) = next_stage {
            self.enter(action, from_list);
        }
        if leave && picked.is_none() {
            self.leave_giving_back();
        }
        if picked.is_some() && !cycling {
            self.gives_back = false;
            self.close();
        }
        picked
    }
}

/// Its keys at `stage`: what picks, what moves, what the cycle key does now
/// (`cycle`: what it changes a value to act on), what goes back; first,
/// that an Enter waits for the rows of what's typed.
fn keys_said(stage: &Stage, cycle: Option<&Bound>, waiting: bool, keymap: &KeymapConfig) -> Vec<String> {
    let mut keys: Vec<String> = match stage {
        Stage::Writing { .. } => keymap.input.keys(&keymap.back, "goes back"),
        Stage::Text { .. } => vec![(keymap.list.pick_label(), "submits"), (keymap.back.label(), "goes back")],
        _ => keymap.list.keys(&keymap.back, "goes back"),
    }
    .into_iter()
    .map(|(key, does)| format!("{key} {does}"))
    .collect();
    if waiting {
        keys.insert(0, "Looking…".to_owned());
    }
    let switch = match (stage, cycle) {
        (Stage::Actions { reach: Reach::Here }, _) => Some("everything"),
        (Stage::Actions { reach: Reach::Everything }, _) => Some("what applies here"),
        (_, Some(bound)) => Some(bound.action.label.as_str()),
        _ => None,
    };
    if let Some(does) = switch {
        keys.insert(keys.len() - 1, format!("{}: {does}", keymap.list.cycle_label()));
    }
    keys
}

fn invocation(bound: &Bound, value: Option<String>) -> Invocation {
    Invocation { action: bound.action.clone(), origin: bound.origin, value }
}
