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

/// The changes from `old` to `new`, line by line, as the Changes view's
/// hunks: each with `context` unchanged lines around it.
pub(crate) fn hunks(old: &str, new: &str, context: usize) -> Vec<Hunk> {
    let diff = similar::TextDiff::from_lines(old, new);
    let number = |at: Option<usize>| at.map(|n| n as u32 + 1);
    diff.grouped_ops(context)
        .iter()
        .map(|group| {
            let lines: Vec<DiffLine> = group
                .iter()
                .flat_map(|op| diff.iter_changes(op))
                .map(|change| DiffLine {
                    kind: match change.tag() {
                        similar::ChangeTag::Equal => LineKind::Context,
                        similar::ChangeTag::Delete => LineKind::Removed,
                        similar::ChangeTag::Insert => LineKind::Added,
                    },
                    old: number(change.old_index()),
                    new: number(change.new_index()),
                    text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
                })
                .collect();
            let (first, last) = (group.first(), group.last());
            let span = |range: fn(&similar::DiffOp) -> std::ops::Range<usize>| first.zip(last).map_or(0..0, |(f, l)| range(f).start..range(l).end);
            // (Numbered from 1; a side with no lines at the one before, as git says.)
            let start = |span: &std::ops::Range<usize>| span.start as u32 + u32::from(!span.is_empty());
            let (old_span, new_span) = (span(similar::DiffOp::old_range), span(similar::DiffOp::new_range));
            Hunk { old_start: start(&old_span), old_lines: old_span.len() as u32, new_start: start(&new_span), new_lines: new_span.len() as u32, lines }
        })
        .collect()
}

/// How many lines `hunks` add and remove.
pub(crate) fn counted(hunks: &[Hunk]) -> (usize, usize) {
    let lines = || hunks.iter().flat_map(|h| &h.lines);
    (lines().filter(|l| l.kind == LineKind::Added).count(), lines().filter(|l| l.kind == LineKind::Removed).count())
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

    /// A file before and after, as hunks: each change with its context
    /// lines around it, numbered on each side; what they add and remove.
    #[test]
    fn a_files_text_before_and_after_is_its_hunks() {
        let old = "a\nb\nc\nd\ne\nf\ng\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\n";
        let hunks = hunks(old, new, 1);
        type Said<'a> = (LineKind, Option<u32>, Option<u32>, &'a str);
        let said: Vec<Vec<Said>> = hunks.iter().map(|h| h.lines.iter().map(|l| (l.kind, l.old, l.new, l.text.as_str())).collect()).collect();
        assert_eq!(said, [
            vec![(LineKind::Context, Some(1), Some(1), "a"), (LineKind::Removed, Some(2), None, "b"), (LineKind::Added, None, Some(2), "B"), (LineKind::Context, Some(3), Some(3), "c")],
            vec![(LineKind::Context, Some(7), Some(7), "g"), (LineKind::Added, None, Some(8), "h")],
        ]);
        assert_eq!((hunks[0].old_start, hunks[0].old_lines, hunks[0].new_start, hunks[0].new_lines), (1, 3, 1, 3));
        assert_eq!(counted(&hunks), (2, 1));
        let new_file = super::hunks("", "new\nfile\n", 3);
        assert_eq!((counted(&new_file), new_file[0].old_start, new_file[0].new_start), ((2, 0), 0, 1), "a new file: all added");
    }
}
