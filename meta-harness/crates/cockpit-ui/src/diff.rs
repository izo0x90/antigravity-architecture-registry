//! Drawing a file's changes: a change's lines, one column (interleaved)
//! or side by side (removed lines beside the added ones). The changes view
//! and a card's changes section draw them the same way.

use egui::RichText;
use harness_contracts::changes::{DiffLine, Hunk, LineKind};

use crate::theme::colors;

/// One line of a change: its numbers, its mark and its text, tinted by
/// what happened to it.
pub(crate) fn draw_line(ui: &mut egui::Ui, line: &DiffLine, numbers: bool) {
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
            if numbers {
                let n = |n: Option<u32>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
                ui.label(RichText::new(format!("{} {}", n(line.old), n(line.new))).monospace().size(look.text_small).color(c.faint));
            }
            ui.label(RichText::new(mark).monospace().size(look.text_body).color(color));
            ui.add(egui::Label::new(RichText::new(&line.text).monospace().size(look.text_body).color(if line.kind == LineKind::Context { c.text } else { color })).truncate());
        });
    });
}

/// A blank half of a side-by-side row (the other side has a line there).
pub(crate) fn draw_gap(ui: &mut egui::Ui) {
    ui.label(RichText::new(" ").monospace().size(crate::theme::look().text_body));
}

/// A change: its header, then its lines (interleaved, or removed lines on
/// the left and added on the right, paired in runs).
pub(crate) fn draw_hunk(ui: &mut egui::Ui, hunk: &Hunk, side_by_side: bool) {
    let look = crate::theme::look();
    ui.label(RichText::new(format!("@@ −{},{} +{},{} @@", hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines)).monospace().size(look.text_small).color(colors().faint));
    if !side_by_side {
        for line in &hunk.lines {
            draw_line(ui, line, true);
        }
        return;
    }
    for (left, right) in paired(&hunk.lines) {
        ui.columns(2, |cols| {
            match left {
                Some(l) => draw_line(&mut cols[0], l, true),
                None => draw_gap(&mut cols[0]),
            }
            match right {
                Some(r) => draw_line(&mut cols[1], r, true),
                None => draw_gap(&mut cols[1]),
            }
        });
    }
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
