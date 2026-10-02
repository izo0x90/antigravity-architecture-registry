//! Command palette: fuzzy search over every available action, then (if the action
//! takes one) its parameter: a filterable choice list or a text field. Opens with
//! the input focused, so it works for typing, dictation and touch alike.

use egui::{Color32, Frame, Margin, RichText, Stroke};

use crate::actions::{Action, Choice, Invocation, Param, fuzzy_score, group_label};

#[derive(Clone, Debug)]
enum Stage {
    Actions,
    Choose { action: Action, from_list: bool },
    Text { action: Action, from_list: bool },
}

pub struct Palette {
    open: bool,
    stage: Stage,
    filter: String,
    selected: usize,
    /// Scroll the selected row into view (only after keyboard moves).
    scroll: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self { open: false, stage: Stage::Actions, filter: String::new(), selected: 0, scroll: false }
    }
}

fn leader_hint(action: &Action) -> String {
    if action.leader.is_empty() {
        return String::new();
    }
    let keys: Vec<String> = action.leader.iter().map(char::to_string).collect();
    format!("SPC {}", keys.join(" "))
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self) {
        *self = Self { open: true, ..Self::default() };
    }

    /// Opens straight at an action's parameter (e.g. from a leader key).
    pub fn open_for(&mut self, action: &Action) {
        self.open();
        self.enter(action.clone(), false);
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    fn enter(&mut self, action: Action, from_list: bool) {
        self.filter.clear();
        self.selected = 0;
        self.stage = match &action.param {
            Param::Text { .. } => Stage::Text { action, from_list },
            _ => Stage::Choose { action, from_list },
        };
    }

    fn back(&mut self) {
        match &self.stage {
            Stage::Choose { from_list: true, .. } | Stage::Text { from_list: true, .. } => {
                self.stage = Stage::Actions;
                self.filter.clear();
                self.selected = 0;
            }
            _ => self.close(),
        }
    }

    /// Renders the palette; returns an invocation when the user picks something.
    pub fn render(&mut self, ctx: &egui::Context, actions: &[Action]) -> Option<Invocation> {
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

        let (up, down, enter, escape) = ctx.input(|i| {
            let ctrl = i.modifiers.ctrl;
            (
                i.key_pressed(egui::Key::ArrowUp) || (ctrl && i.key_pressed(egui::Key::P)),
                i.key_pressed(egui::Key::ArrowDown) || (ctrl && i.key_pressed(egui::Key::N)),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if escape {
            self.back();
            return None;
        }

        let mut picked: Option<Invocation> = None;
        let mut next_stage: Option<(Action, bool)> = None;
        let stage = self.stage.clone();

        egui::Area::new(egui::Id::new("command_palette"))
            .fixed_pos(pos)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(Color32::from_rgba_premultiplied(16, 21, 32, 250))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(56, 120, 210)))
                    .corner_radius(8)
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        let title = match &stage {
                            Stage::Actions => "Run an action".to_owned(),
                            Stage::Choose { action, .. } | Stage::Text { action, .. } => action.label.clone(),
                        };
                        ui.label(RichText::new(title).strong().size(12.0).color(Color32::from_rgb(90, 200, 255)));
                        let hint = match &stage {
                            Stage::Text { action, .. } => match &action.param {
                                Param::Text { hint } => hint.clone(),
                                _ => String::new(),
                            },
                            _ => "Type to filter…".to_owned(),
                        };
                        let edit = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text(hint).desired_width(f32::INFINITY));
                        edit.request_focus();
                        if edit.changed() {
                            self.selected = 0;
                        }
                        ui.add_space(4.0);

                        match &stage {
                            Stage::Actions => {
                                let mut ranked: Vec<(i32, &Action)> = actions
                                    .iter()
                                    .filter_map(|a| {
                                        let group = if a.leader.len() > 1 { group_label(&a.leader[..1]).unwrap_or("") } else { "" };
                                        let haystack = format!("{} {} {group}", a.label, a.id);
                                        fuzzy_score(&self.filter, &haystack).map(|s| (s, a))
                                    })
                                    .collect();
                                ranked.sort_by_key(|(s, a)| (-s, a.label.clone()));
                                self.move_selection(up, down, ranked.len());
                                egui::ScrollArea::vertical().max_height(list_height).min_scrolled_height(list_height).show(ui, |ui| {
                                    for (row, (_, action)) in ranked.iter().enumerate() {
                                        let selected = row == self.selected;
                                        let clicked = palette_row(ui, selected, self.scroll, &action.label, action.detail.as_deref(), &leader_hint(action));
                                        if clicked || (selected && enter) {
                                            match action.param {
                                                Param::None => picked = Some(Invocation::of(action, None)),
                                                _ => next_stage = Some(((*action).clone(), true)),
                                            }
                                        }
                                    }
                                    if ranked.is_empty() {
                                        ui.label(RichText::new("No matching actions").size(11.0).color(Color32::GRAY));
                                    }
                                });
                            }
                            Stage::Choose { action, .. } => {
                                let choices: &[Choice] = match &action.param {
                                    Param::Choice(c) => c,
                                    _ => &[],
                                };
                                let mut ranked: Vec<(i32, &Choice)> = choices
                                    .iter()
                                    .filter_map(|c| fuzzy_score(&self.filter, &format!("{} {}", c.label, c.value)).map(|s| (s, c)))
                                    .collect();
                                ranked.sort_by_key(|(s, _)| -s);
                                self.move_selection(up, down, ranked.len());
                                egui::ScrollArea::vertical().max_height(list_height).min_scrolled_height(list_height).show(ui, |ui| {
                                    for (row, (_, choice)) in ranked.iter().enumerate() {
                                        let selected = row == self.selected;
                                        let label = if choice.current { format!("{} {}", egui_phosphor::regular::CHECK, choice.label) } else { choice.label.clone() };
                                        let clicked = palette_row(ui, selected, self.scroll, &label, choice.detail.as_deref(), "");
                                        if clicked || (selected && enter) {
                                            picked = Some(Invocation::of(action, Some(choice.value.clone())));
                                        }
                                    }
                                    if ranked.is_empty() {
                                        ui.label(RichText::new("Nothing to choose").size(11.0).color(Color32::GRAY));
                                    }
                                });
                            }
                            Stage::Text { action, .. } => {
                                if enter && !self.filter.trim().is_empty() {
                                    picked = Some(Invocation::of(action, Some(self.filter.trim().to_owned())));
                                }
                                ui.label(RichText::new("Enter to submit • Esc to go back").size(10.0).color(Color32::GRAY));
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

/// One tappable row; returns true when clicked.
fn palette_row(ui: &mut egui::Ui, selected: bool, scroll: bool, label: &str, detail: Option<&str>, keys: &str) -> bool {
    let fill = if selected { Color32::from_rgb(35, 55, 85) } else { Color32::TRANSPARENT };
    let response = Frame::NONE
        .fill(fill)
        .corner_radius(4)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(12.0).color(Color32::WHITE));
                if let Some(detail) = detail {
                    ui.label(RichText::new(detail).size(10.0).color(Color32::GRAY));
                }
                if !keys.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(keys).monospace().size(10.0).color(Color32::from_rgb(255, 215, 0)));
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
