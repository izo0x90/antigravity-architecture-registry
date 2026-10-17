//! Command palette: fuzzy search over every available action, then (if the action
//! takes one) its parameter: a filterable choice list, a text field, or a search
//! whose results the action's view looks up as you type. Opens with the input
//! focused, so it works for typing, dictation and touch alike. Matching is the
//! same everywhere (`arch_search::rank`).

use egui::{Color32, Frame, RichText, Stroke};

use crate::actions::{Bound, Choice, Invocation, Origin, Param};
use crate::keymap::KeymapConfig;
use crate::theme::colors;

#[derive(Clone, Debug)]
enum Stage {
    Actions,
    Choose { action: Box<Bound>, from_list: bool },
    Text { action: Box<Bound>, from_list: bool },
    /// Looked up as it's typed: the action is read afresh each frame (its
    /// results come in after the query goes out).
    Search { action: Box<Bound>, from_list: bool },
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
    selected: usize,
    /// Scroll the selected row into view (only after keyboard moves).
    scroll: bool,
    /// A search query to hand to its view (taken by the app each frame).
    ask: Option<SearchAsk>,
    /// Enter came in the frame the search changed: its results are of what
    /// was typed before, so it's taken the next frame, on the new ones.
    enter_waiting: bool,
}

impl Palette {
    /// The row the keys are on.
    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.selected
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self { open: false, stage: Stage::Actions, filter: String::new(), selected: 0, scroll: false, ask: None, enter_waiting: false }
    }
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        *self = Self { open: true, ..Self::default() };
    }

    /// Opens with the list already filtered (e.g. "index." for the index actions).
    pub fn open_with(&mut self, filter: &str) {
        self.open();
        self.filter = filter.to_owned();
    }

    /// Opens straight at an action's parameter (e.g. from a leader key).
    pub fn open_for(&mut self, action: &Bound) {
        self.open();
        self.enter(action.clone(), false);
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    fn enter(&mut self, action: Bound, from_list: bool) {
        self.filter.clear();
        self.selected = 0;
        let action = Box::new(action);
        self.stage = match &action.action.param {
            Param::Text { .. } => Stage::Text { action, from_list },
            Param::Search { .. } => {
                self.ask = Some(SearchAsk { origin: action.origin, action: action.action.id.clone(), query: String::new() });
                Stage::Search { action, from_list }
            }
            _ => Stage::Choose { action, from_list },
        };
    }

    /// The query a search stage wants looked up, once per change.
    pub fn take_search(&mut self) -> Option<SearchAsk> {
        self.ask.take()
    }

    fn back(&mut self) {
        match &self.stage {
            Stage::Choose { from_list: true, .. } | Stage::Text { from_list: true, .. } | Stage::Search { from_list: true, .. } => {
                self.stage = Stage::Actions;
                self.filter.clear();
                self.selected = 0;
            }
            _ => self.close(),
        }
    }

    /// Renders the palette; returns an invocation when the user picks something.
    pub fn render(&mut self, ctx: &egui::Context, actions: &[Bound], keymap: &KeymapConfig) -> Option<Invocation> {
        if !self.open {
            return None;
        }
        let screen = ctx.content_rect();
        let narrow = screen.width() < 600.0;
        let width = (screen.width() - 32.0).min(620.0);
        let pos = if narrow {
            egui::pos2(16.0, (screen.max.y - 420.0).max(60.0))
        } else {
            egui::pos2((screen.width() - width) / 2.0, 90.0)
        };
        let list_height = (screen.max.y - pos.y - 110.0).clamp(120.0, 360.0);

        let step = ctx.input_mut(|i| keymap.list.step(i));
        let (enter, escape) = ctx.input(|i| (keymap.list.picked(i), keymap.back.pressed(i)));
        let enter = enter || std::mem::take(&mut self.enter_waiting);
        let (up, down) = (step < 0, step > 0);
        if escape {
            self.back();
            return None;
        }

        let mut picked: Option<Invocation> = None;
        let mut next_stage: Option<(Bound, bool)> = None;
        // A search's results arrive after it's asked: read the action afresh.
        if let Stage::Search { action, .. } = &mut self.stage
            && let Some(fresh) = actions.iter().find(|b| b.origin == action.origin && b.action.id == action.action.id)
        {
            **action = fresh.clone();
        }
        let stage = self.stage.clone();

        egui::Area::new(egui::Id::new("command_palette"))
            .fixed_pos(pos)
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
                            Stage::Actions => "Run an action".to_owned(),
                            Stage::Choose { action, .. } | Stage::Text { action, .. } | Stage::Search { action, .. } => action.action.label.clone(),
                        };
                        ui.label(RichText::new(title).strong().size(crate::theme::look().text_label).color(colors().info));
                        let hint = match &stage {
                            Stage::Text { action, .. } | Stage::Search { action, .. } => match &action.action.param {
                                Param::Text { hint } | Param::Search { hint, .. } => hint.clone(),
                                _ => String::new(),
                            },
                            _ => "Type to filter…".to_owned(),
                        };
                        let edit = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(hint).desired_width(f32::INFINITY));
                        edit.request_focus();
                        let mut enter = enter;
                        if edit.changed() {
                            self.selected = 0;
                            if let Stage::Search { action, .. } = &stage {
                                self.ask = Some(SearchAsk { origin: action.origin, action: action.action.id.clone(), query: self.filter.clone() });
                                // Results of what was typed before: Enter waits for these.
                                (self.enter_waiting, enter) = (enter, false);
                                ctx.request_repaint();
                            }
                        }
                        ui.add_space(crate::theme::look().space_s);

                        match &stage {
                            Stage::Actions => {
                                // `:42` goes to line (or row) 42 of the focused view.
                                let line = self.filter.trim().parse::<usize>().ok().filter(|n| *n > 0).and_then(|n| actions.iter().find(|b| b.action.id == "nav.line").map(|b| (n, b)));
                                let mut listed: Vec<&Bound> = actions.iter().collect();
                                // The closest to where you are first, then by name.
                                listed.sort_by(|a, b| (a.layer, &a.action.label).cmp(&(b.layer, &b.action.label)));
                                let haystacks: Vec<String> = listed
                                    .iter()
                                    .map(|b| {
                                        let group = b.leader.as_ref().filter(|l| l.len() > 1).and_then(|l| keymap.group_label(&l[..1])).unwrap_or("");
                                        format!("{} {} {group}", b.action.label, b.action.id)
                                    })
                                    .collect();
                                let ranked: Vec<&Bound> = arch_search::rank(&self.filter, &haystacks, arch_search::Matching::shared()).into_iter().map(|r| listed[r.index]).collect();
                                let first = usize::from(line.is_some());
                                self.move_selection(up, down, ranked.len() + first);
                                egui::ScrollArea::vertical().max_height(list_height).min_scrolled_height(list_height).show(ui, |ui| {
                                    if let Some((n, bound)) = line {
                                        let selected = self.selected == 0;
                                        if palette_row(ui, selected, self.scroll, &format!("Go to line {n}"), None, ":N") || (selected && enter) {
                                            picked = Some(invocation(bound, Some(n.to_string())));
                                        }
                                    }
                                    for (row, bound) in ranked.iter().enumerate() {
                                        let row = row + first;
                                        let selected = row == self.selected;
                                        let clicked = palette_row(ui, selected, self.scroll, &bound.action.label, bound.action.detail.as_deref(), &bound.key_hint());
                                        if clicked || (selected && enter) {
                                            match bound.action.param {
                                                Param::None => picked = Some(invocation(bound, None)),
                                                _ => next_stage = Some(((*bound).clone(), true)),
                                            }
                                        }
                                    }
                                    if ranked.is_empty() && line.is_none() {
                                        ui.label(RichText::new("No matching actions").size(crate::theme::look().text_body).color(colors().muted));
                                    }
                                });
                            }
                            Stage::Choose { action, .. } => {
                                let choices: Vec<Choice> = action.action.param.choices();
                                let haystacks: Vec<String> = choices.iter().map(|c| format!("{} {}", c.label, c.value)).collect();
                                let ranked: Vec<&Choice> = arch_search::rank(&self.filter, &haystacks, arch_search::Matching::shared()).into_iter().map(|r| &choices[r.index]).collect();
                                self.move_selection(up, down, ranked.len());
                                egui::ScrollArea::vertical().max_height(list_height).min_scrolled_height(list_height).show(ui, |ui| {
                                    for (row, choice) in ranked.iter().enumerate() {
                                        let selected = row == self.selected;
                                        let label = if choice.current { format!("{} {}", egui_phosphor::regular::CHECK, choice.label) } else { choice.label.clone() };
                                        let clicked = palette_row(ui, selected, self.scroll, &label, choice.detail.as_deref(), "");
                                        if clicked || (selected && enter) {
                                            picked = Some(invocation(action, Some(choice.value.clone())));
                                        }
                                    }
                                    if ranked.is_empty() {
                                        ui.label(RichText::new("Nothing to choose").size(crate::theme::look().text_body).color(colors().muted));
                                    }
                                });
                            }
                            Stage::Search { action, .. } => {
                                let (found, waiting) = match &action.action.param {
                                    Param::Search { found, waiting, .. } => (found.clone(), *waiting),
                                    _ => (Vec::new(), false),
                                };
                                self.move_selection(up, down, found.len());
                                egui::ScrollArea::vertical().max_height(list_height).min_scrolled_height(list_height).show(ui, |ui| {
                                    for (row, choice) in found.iter().enumerate() {
                                        let selected = row == self.selected;
                                        let clicked = palette_row(ui, selected, self.scroll, &choice.label, choice.detail.as_deref(), "");
                                        if clicked || (selected && enter) {
                                            picked = Some(invocation(action, Some(choice.value.clone())));
                                        }
                                    }
                                    if found.is_empty() {
                                        let note = match (waiting, self.filter.trim().is_empty()) {
                                            (true, _) => "Looking…",
                                            (false, true) => "Type to search",
                                            (false, false) => "Nothing found",
                                        };
                                        ui.label(RichText::new(note).size(crate::theme::look().text_body).color(colors().muted));
                                    }
                                });
                            }
                            Stage::Text { action, .. } => {
                                if enter && !self.filter.trim().is_empty() {
                                    picked = Some(invocation(action, Some(self.filter.trim().to_owned())));
                                }
                                ui.label(RichText::new(format!("{} submits · {} goes back", keymap.list.pick_label(), keymap.back.label())).size(crate::theme::look().text_small).color(colors().muted));
                            }
                        }
                    });
            });

        if let Some((action, from_list)) = next_stage {
            self.enter(action, from_list);
        }
        if picked.is_some() {
            self.close();
        }
        picked
    }

    fn move_selection(&mut self, up: bool, down: bool, len: usize) {
        self.scroll = up || down;
        if len == 0 {
            self.selected = 0;
            return;
        }
        if down {
            self.selected = (self.selected + 1) % len;
        }
        if up {
            self.selected = (self.selected + len - 1) % len;
        }
        self.selected = self.selected.min(len - 1);
    }
}

fn invocation(bound: &Bound, value: Option<String>) -> Invocation {
    Invocation { action: bound.action.clone(), origin: bound.origin, value }
}

/// One tappable row; returns true when clicked.
fn palette_row(ui: &mut egui::Ui, selected: bool, scroll: bool, label: &str, detail: Option<&str>, keys: &str) -> bool {
    let fill = if selected { colors().selection } else { Color32::TRANSPARENT };
    let response = Frame::NONE
        .fill(fill)
        .corner_radius(crate::theme::look().radius_m)
        .inner_margin(crate::theme::margin2(crate::theme::look().space_m, crate::theme::look().space_sm))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(crate::theme::look().text_label).color(colors().text_strong));
                if let Some(detail) = detail {
                    ui.label(RichText::new(detail).size(crate::theme::look().text_small).color(colors().muted));
                }
                if !keys.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(keys).monospace().size(crate::theme::look().text_small).color(colors().key));
                    });
                }
            });
        })
        .response;
    if selected && scroll {
        response.scroll_to_me(None);
    }
    response.interact(egui::Sense::click()).clicked()
}
