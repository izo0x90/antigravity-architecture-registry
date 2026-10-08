//! A tree as text, for agents: what it shows and what it cut, then each
//! card indented under the one it hangs from, with what sits on its line,
//! what it hides, and lines back to cards shown elsewhere.

use std::collections::BTreeMap;
use std::fmt::Write;

use super::{Card, LineKind, Mark, PillRole, Tree, TreeConfig, ViewKind, Way};

fn mark(m: Mark) -> &'static str {
    match m {
        Mark::Code => "",
        Mark::New => " NEW",
        Mark::Changed => " CHANGED",
        Mark::Done => " DONE",
        Mark::Mismatch => " DOESN'T MATCH",
    }
}

fn way(view: ViewKind, w: Way) -> &'static str {
    match (view, w) {
        (ViewKind::Contains, Way::Down) => "what's inside",
        (ViewKind::Contains, Way::Up) => "what holds it",
        (ViewKind::Calls, Way::Down) => "what it calls",
        (ViewKind::Calls, Way::Up) => "what calls it",
        (ViewKind::DataFlow, Way::Down) => "where it goes",
        (ViewKind::DataFlow, Way::Up) => "where it came from",
    }
}

fn describe(card: &Card) -> String {
    let mut line = format!("{} [{}{}]", card.label, card.kind, mark(card.mark));
    if !card.detail.is_empty() {
        let _ = write!(line, " {}", card.detail);
    }
    if card.more > 0 {
        let _ = write!(line, " (+{} more)", card.more);
    }
    if card.links_left_out > 0 {
        let _ = write!(line, " (+{} lines to cards shown elsewhere, not drawn)", card.links_left_out);
    }
    if card.outside_plan {
        line.push_str(" (code the plan plugs into)");
    }
    if !card.other_plans.is_empty() {
        let names: Vec<&str> = card.other_plans.iter().map(|p| p.0.as_str()).collect();
        let _ = write!(line, " (also changed by {})", names.join(", "));
    }
    let _ = write!(line, "  key={}", card.key);
    line
}

pub fn render(tree: &Tree, config: &TreeConfig) -> String {
    let mut out = String::new();
    let view = tree.request.view;
    let _ = writeln!(out, "{} ({}), {}: {}", view.name(), tree.request.source, way(view, tree.request.way), config.view(view).description);
    let depth = tree.request.depth.unwrap_or(0);
    let _ = writeln!(out, "{} cards, {} levels deep.", tree.cards.len(), depth);
    if let Some(cut) = &tree.cut {
        if cut.shown < cut.asked {
            let _ = writeln!(out, "Cut: showing {} of {} levels; {} more not shown (go into a card, or ask for less depth).", cut.shown, cut.asked, cut.hidden);
        }
        if cut.starts_left_out > 0 {
            let _ = writeln!(out, "Showing the first {} starting points of {} (most reached first); pick a root to see another.", tree.roots.len(), tree.roots.len() as u32 + cut.starts_left_out);
        }
    }
    if !tree.path.is_empty() {
        let names: Vec<&str> = tree.path.iter().map(|c| c.label.as_str()).collect();
        let _ = writeln!(out, "Inside: {}", names.join(" > "));
    }
    if tree.cards.is_empty() {
        out.push_str("(nothing to show)\n");
        return out;
    }
    let mut kids: BTreeMap<&str, Vec<&Card>> = BTreeMap::new();
    for c in &tree.cards {
        if let Some(p) = &c.parent {
            kids.entry(p.as_str()).or_default().push(c);
        }
    }
    let tree_line = |from: &str, to: &str| tree.lines.iter().find(|l| l.from == from && l.to == to && l.kind == LineKind::Tree);
    let mut stack: Vec<(&Card, usize)> = tree.cards.iter().filter(|c| c.parent.is_none()).rev().map(|c| (c, 0)).collect();
    while let Some((card, indent)) = stack.pop() {
        let pad = "  ".repeat(indent);
        let mut line = format!("{pad}{}", describe(card));
        if let Some(l) = card.parent.as_deref().and_then(|p| tree_line(p, &card.key)) {
            if l.mark != Mark::Code {
                let _ = write!(line, " (line{})", mark(l.mark));
            }
            if !l.pills.is_empty() {
                let pills: Vec<String> = l
                    .pills
                    .iter()
                    .map(|p| match p.role {
                        PillRole::In => format!("in {}", p.text),
                        PillRole::Out => format!("out {}", p.text),
                        PillRole::Step => format!("via {}", p.text),
                    })
                    .collect();
                let _ = write!(line, " <{}>", pills.join("; "));
            }
        }
        let _ = writeln!(out, "{line}");
        for back in tree.lines.iter().filter(|l| l.from == card.key && l.kind != LineKind::Tree) {
            let what = if back.kind == LineKind::Loop { "loops back to" } else { "also reaches (shown elsewhere)" };
            let label = tree.card(&back.to).map_or(back.to.as_str(), |c| c.label.as_str());
            let _ = writeln!(out, "{pad}  {what} {label}");
        }
        for kid in kids.get(card.key.as_str()).into_iter().flatten().rev() {
            stack.push((kid, indent + 1));
        }
    }
    if !tree.dropped.is_empty() {
        let _ = writeln!(out, "Didn't fit (closed again): {}", tree.dropped.join(", "));
    }
    out
}
