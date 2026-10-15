//! Hints, search and marks: typed keys a view reads itself.

use super::*;

/// Where a hint label goes so it hides as little as it can, and never on
/// another label: just left of its item, else just right of it (inside the
/// slot), else at the right end of a wide item, else over its corner.
pub(super) fn hint_chip(item: egui::Rect, size: egui::Vec2, slot: egui::Rect, taken: &[egui::Rect]) -> egui::Rect {
    let top = item.top() + ((item.height().min(18.0) - size.y) / 2.0).min(0.0);
    let at = |x: f32| egui::Rect::from_min_size(egui::pos2(x, top), size);
    let inside = |r: &egui::Rect| !slot.is_positive() || (r.left() >= slot.left() && r.right() <= slot.right());
    let mut places = vec![at(item.left() - size.x - 2.0), at(item.right() + 2.0)];
    if item.width() > size.x * 3.0 {
        places.push(at(item.right() - size.x - 2.0));
    }
    let corner = egui::Rect::from_min_size(item.left_top() + egui::vec2(-4.0, -4.0), size);
    places.push(corner);
    places.iter().copied().find(|r| inside(r) && !taken.iter().any(|t| t.intersects(*r))).or_else(|| places.iter().copied().find(inside)).unwrap_or(corner)
}

impl CockpitApp {
    pub(super) fn start_hint(&mut self, open: bool) {
        let kind = self.focused_kind();
        let spots = self.desk.view(kind).spots();
        let chars: Vec<char> = self.config.keymap.hint_chars.chars().collect();
        let labels = hint_labels(spots.len(), &chars);
        if labels.is_empty() {
            self.note = Some("Nothing on screen to label.".into());
            return;
        }
        self.start_capture(Capture::Hint(Hinting { slot: self.desk.columns.focused(), kind, open, typed: String::new(), labels: labels.into_iter().zip(spots).collect() }));
    }

    /// A hint label was typed (or tapped): move there, or open it.
    pub(super) fn hint_hit(&mut self, kind: BufferKind, key: &str, open: bool, ctx: &egui::Context) {
        let before = self.jump_here();
        if open {
            let step = self.desk.view_mut(kind).open_item(key);
            self.take_step(step, before, ctx);
        } else {
            self.desk.view_mut(kind).move_to(key);
            self.after_jump(before);
        }
    }

    /// Typed keys while the shell captures them; returns whether it still does.
    pub(super) fn on_capture(&mut self, typed: &[char], back: bool, ctx: &egui::Context) -> bool {
        let Some(capture) = self.capture.take() else { return false };
        match capture {
            Capture::Hint(mut hint) => {
                if back {
                    return false;
                }
                for c in typed {
                    hint.typed.push(*c);
                    if let Some((_, spot)) = hint.labels.iter().find(|(label, _)| *label == hint.typed) {
                        let key = spot.key.clone();
                        self.hint_hit(hint.kind, &key, hint.open, ctx);
                        return false;
                    }
                    if !hint.labels.iter().any(|(label, _)| label.starts_with(&hint.typed)) {
                        return false;
                    }
                }
                self.capture = Some(Capture::Hint(hint));
                true
            }
            Capture::Search(mut search) => {
                let kind = search.kind;
                if back {
                    self.desk.view_mut(kind).on_search(SearchEvent::Cancelled);
                    self.desk.view_mut(kind).restore(&search.before.place);
                    self.desk.view_mut(kind).restore_view();
                    return false;
                }
                for c in typed {
                    match c {
                        '\n' | '\r' => {
                            self.desk.view_mut(kind).on_search(SearchEvent::Confirmed);
                            // Only a search that finds something stays for n/N.
                            let found = !rank_matches(&self.desk.view(kind).items(), &search.query).is_empty();
                            if found {
                                self.desk.searches.insert(kind, search.query.clone());
                                self.desk.missed.remove(&kind);
                            } else {
                                self.desk.searches.remove(&kind);
                                if search.query.is_empty() {
                                    self.desk.missed.remove(&kind);
                                } else {
                                    self.desk.missed.insert(kind, search.query.clone());
                                }
                            }
                            self.after_jump(search.before);
                            return false;
                        }
                        '\u{8}' => {
                            search.query.pop();
                        }
                        c if !c.is_control() => search.query.push(*c),
                        _ => {}
                    }
                }
                // Incremental: the first match shows while typing.
                self.desk.view_mut(kind).on_search(SearchEvent::Typing(&search.query));
                let items = self.desk.view(kind).items();
                match rank_matches(&items, &search.query).first() {
                    Some(&first) => self.desk.view_mut(kind).move_to(&items[first].key),
                    None => {
                        // No match (yet): back where it was, looking as it did.
                        self.desk.view_mut(kind).restore(&search.before.place);
                        self.desk.view_mut(kind).restore_view();
                        self.desk.view_mut(kind).save_view();
                    }
                }
                self.capture = Some(Capture::Search(search));
                true
            }
            Capture::MarkSet | Capture::MarkJump => {
                // Waits for a letter; anything else (or Escape) gives up.
                if !back && typed.is_empty() {
                    self.capture = Some(capture);
                    return true;
                }
                let Some(letter) = typed.iter().copied().find(|c| c.is_alphanumeric()).filter(|_| !back) else { return false };
                if capture == Capture::MarkSet {
                    self.desk.marks.insert(letter, self.jump_here());
                    self.note = Some(format!("Mark {letter} set."));
                } else {
                    match self.desk.marks.get(&letter).cloned() {
                        Some(mark) => {
                            let before = self.jump_here();
                            self.restore_jump(mark, ctx);
                            self.after_jump(before);
                        }
                        None => self.note = Some(format!("No mark {letter}.")),
                    }
                }
                false
            }
        }
    }

    /// The next (or previous) match of the view's last search.
    pub(super) fn search_step(&mut self, forward: bool, count: usize) {
        let kind = self.focused_kind();
        let Some(query) = self.desk.searches.get(&kind).cloned() else {
            self.note = Some(match self.desk.missed.get(&kind) {
                Some(missed) => format!("No match for {missed} here."),
                None => "Nothing searched here yet (/).".into(),
            });
            return;
        };
        let items = self.desk.view(kind).items();
        let found = rank_matches(&items, &query);
        if found.is_empty() {
            self.note = Some(format!("No match for {query}."));
            return;
        }
        let mut at = self.desk.view(kind).cursor_key().and_then(|k| items.iter().position(|i| i.key == k));
        for _ in 0..count {
            at = step_match(&found, at, forward);
        }
        if let Some(i) = at {
            let before = self.jump_here();
            self.desk.view_mut(kind).move_to(&items[i].key);
            self.after_jump(before);
        }
    }

    /// The search line for the key bar: the prompt while typing, else where
    /// the cursor is among the view's last search's matches.
    pub(super) fn search_status(&self) -> Option<(String, bool)> {
        if let Some(Capture::Search(search)) = &self.capture {
            let found = rank_matches(&self.desk.view(search.kind).items(), &search.query).len();
            return Some((format!("/{}| {found} {}", search.query, if found == 1 { "match" } else { "matches" }), true));
        }
        let kind = self.focused_kind();
        let query = self.desk.searches.get(&kind)?;
        let items = self.desk.view(kind).items();
        let found = rank_matches(&items, query);
        let cursor = self.desk.view(kind).cursor_key()?;
        let at = found.iter().position(|i| items[*i].key == cursor)?;
        Some((format!("/{query} {}/{}", at + 1, found.len()), false))
    }

    /// Hint labels over what the focused view draws; tapping one picks it.
    pub(super) fn render_hints(&mut self, ctx: &egui::Context) {
        let Some(Capture::Hint(hint)) = &self.capture else { return };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("hint_labels")));
        let tap = ctx.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
        let mut hit: Option<String> = None;
        let mut taken: Vec<egui::Rect> = Vec::new();
        for (label, spot) in &hint.labels {
            if !label.starts_with(&hint.typed) {
                continue;
            }
            let galley = painter.layout_no_wrap(label.to_uppercase(), egui::FontId::monospace(crate::theme::look().text_title), colors().on_bright);
            let chip = hint_chip(spot.rect, galley.size() + egui::vec2(8.0, 4.0), self.slot_rects.get(hint.slot).copied().unwrap_or(egui::Rect::EVERYTHING), &taken);
            taken.push(chip);
            painter.rect_filled(chip, 3.0, colors().key);
            painter.galley(chip.min + egui::vec2(4.0, 2.0), galley, colors().on_bright);
            if tap.is_some_and(|p| chip.contains(p)) {
                hit = Some(spot.key.clone());
            }
        }
        if let Some(key) = hit
            && let Some(Capture::Hint(hint)) = self.capture.take()
        {
            self.input_mode = InputMode::Normal;
            self.hint_hit(hint.kind, &key, hint.open, ctx);
        }
    }
}
