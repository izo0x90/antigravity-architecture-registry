//! Drawing a file's changes: a change's lines, one column (interleaved)
//! or side by side (removed lines beside the added ones), and what they
//! add and remove. The Changes view, a card's changes section and the
//! chat draw them the same way.

use egui::RichText;
use harness_contracts::changes::{DiffLine, Hunk, LineKind};

use crate::theme::colors;

/// Whether a change's lines show their numbers (a file's change: yes; a
/// call's, compared from texts that may be a part of the file: no).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Numbers {
    Shown,
    Left,
}

/// One line of a change: its numbers, its mark and its text, tinted by
/// what happened to it.
pub(crate) fn draw_line(ui: &mut egui::Ui, line: &DiffLine, numbers: Numbers) {
    let look = crate::theme::look();
    let c = colors();
    let (mark, color, fill) = match line.kind {
        LineKind::Added => ("+", c.ok, c.ok_bg),
        LineKind::Removed => ("−", c.error, c.error_bg),
        LineKind::Context => (" ", c.text, egui::Color32::TRANSPARENT),
    };
    egui::Frame::NONE.fill(fill).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = look.space_xs;
            if numbers == Numbers::Shown {
                let n = |n: Option<u32>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
                ui.label(RichText::new(format!("{} {}", n(line.old), n(line.new))).monospace().size(look.text_small).color(c.faint));
            }
            ui.label(RichText::new(mark).monospace().size(look.text_body).color(color));
            ui.add(
                egui::Label::new(RichText::new(&line.text).monospace().size(look.text_body).color(if line.kind == LineKind::Context {
                    c.text
                } else {
                    color
                }))
                .truncate(),
            );
        });
    });
}

/// A blank half of a side-by-side row (the other side has a line there).
pub(crate) fn draw_gap(ui: &mut egui::Ui) {
    ui.label(RichText::new(" ").monospace().size(crate::theme::look().text_body));
}

/// A change in one column: its header (where it is, numbered), then its
/// lines, interleaved: those out of sight take a line's room undrawn
/// (each is one line high, cut short where it's too long).
pub(crate) fn draw_hunk(ui: &mut egui::Ui, hunk: &Hunk, numbers: Numbers) {
    if numbers == Numbers::Shown {
        ui.label(
            RichText::new(format!("@@ −{},{} +{},{} @@", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines))
                .monospace()
                .size(crate::theme::look().text_small)
                .color(colors().faint),
        );
    }
    let mut tall = None;
    for line in &hunk.lines {
        let width = ui.available_width();
        if let Some(height) = tall
            && !ui.is_rect_visible(egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, height)))
        {
            ui.allocate_space(egui::vec2(width, height));
            continue;
        }
        let top = ui.cursor().top();
        draw_line(ui, line, numbers);
        tall = Some(ui.cursor().top() - top - ui.spacing().item_spacing.y);
    }
}

/// How many of `lines` are added and removed.
pub(crate) fn counts<'a>(lines: impl IntoIterator<Item = &'a DiffLine>) -> (usize, usize) {
    lines.into_iter().fold((0, 0), |(added, removed), l| match l.kind {
        LineKind::Added => (added + 1, removed),
        LineKind::Removed => (added, removed + 1),
        LineKind::Context => (added, removed),
    })
}

/// What a change adds and removes, as the Changes view says it: "+N" and
/// "−N", each in its colour.
pub(crate) fn draw_counts(ui: &mut egui::Ui, added: usize, removed: usize) {
    let look = crate::theme::look();
    ui.label(RichText::new(format!("+{added}")).monospace().size(look.text_small).color(colors().ok));
    ui.label(RichText::new(format!("−{removed}")).monospace().size(look.text_small).color(colors().error));
}

/// Lines as side-by-side rows: unchanged ones on both sides; a run of
/// removed lines beside the run of added ones that follows it.
pub(crate) fn paired(lines: &[DiffLine]) -> Vec<(Option<&DiffLine>, Option<&DiffLine>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind == LineKind::Context {
            rows.push((Some(&lines[i]), Some(&lines[i])));
            i += 1;
            continue;
        }
        let removed: Vec<&DiffLine> = lines[i..].iter().take_while(|l| l.kind == LineKind::Removed).collect();
        i += removed.len();
        let added: Vec<&DiffLine> = lines[i..].iter().take_while(|l| l.kind == LineKind::Added).collect();
        i += added.len();
        for n in 0..removed.len().max(added.len()) {
            rows.push((removed.get(n).copied(), added.get(n).copied()));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What lines add and remove, counted.
    #[test]
    fn a_changes_lines_are_counted() {
        let line = |kind| DiffLine { kind, old: None, new: None, text: String::new() };
        assert_eq!(counts(&[line(LineKind::Added), line(LineKind::Context), line(LineKind::Removed), line(LineKind::Added)]), (2, 1));
    }
}
