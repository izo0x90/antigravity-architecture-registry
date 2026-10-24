//! What the action list lists: what applies where you are (`:`), or
//! everything (`?`): every action, faded where it doesn't apply (and what
//! it needs, when that's known), then keys that run nothing themselves (a
//! mode's, the pointer's), each under its section's title. Matched by the
//! shared fuzzy matcher; with everything, each section keeps its rows in
//! the order they matched.

use std::borrow::Cow;

use crate::actions::{Bound, Group, Layer, Param};
use crate::keymap::KeymapConfig;
use crate::pick_list::{Row, RowKind};

/// What the list needs to know of where you are, beyond the actions:
/// which of them apply now (in their order), the layers' titles (what
/// you're on, the panel's view, what holds it, everywhere), and keys that
/// run nothing themselves, by section (a mode's, the pointer's).
pub struct Sheet {
    pub applies: Vec<bool>,
    pub layers: [String; 4],
    pub informs: Vec<Informs>,
}

/// A section of keys that run nothing themselves: its title, and each key
/// with what it does.
pub type Informs = (String, Vec<(String, String)>);

const LAYERS: [Layer; 4] = [Layer::On, Layer::Panel, Layer::Host, Layer::Everywhere];

/// One thing listed.
pub enum Item<'a> {
    Action { bound: &'a Bound, applies: bool },
    Inform { keys: &'a str, what: &'a str },
}

/// One line of the list: a section's title, or an entry (by its place).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Title(usize),
    Entry(usize),
}

/// The entries listed, each in its section, and the sections' titles.
pub struct Listed<'a> {
    titles: Vec<String>,
    entries: Vec<(usize, Item<'a>)>,
    everything: bool,
}

impl<'a> Listed<'a> {
    /// What applies, closest to where you are first, then by name; or
    /// everything, by layer and group (most important first), then the
    /// keys that only inform.
    pub fn of(actions: &'a [Bound], sheet: &'a Sheet, everything: bool) -> Self {
        let applies = |i: usize| sheet.applies.get(i).copied().unwrap_or(true);
        if !everything {
            let mut listed: Vec<(usize, &Bound)> = actions.iter().enumerate().filter(|(i, _)| applies(*i)).collect();
            listed.sort_by(|(_, a), (_, b)| (a.layer, &a.action.label).cmp(&(b.layer, &b.action.label)));
            let entries = listed.into_iter().map(|(_, bound)| (0, Item::Action { bound, applies: true })).collect();
            return Self { titles: Vec::new(), entries, everything };
        }
        let (mut titles, mut entries) = (Vec::new(), Vec::new());
        for (layer, title) in LAYERS.into_iter().zip(&sheet.layers) {
            for group in Group::ALL {
                let mut these: Vec<(usize, &Bound)> = actions.iter().enumerate().filter(|(_, b)| b.layer == layer && b.action.group == group).collect();
                if these.is_empty() {
                    continue;
                }
                these.sort_by_key(|(_, b)| b.action.importance);
                titles.push(format!("{title} · {}", group.title()));
                entries.extend(these.into_iter().map(|(i, bound)| (titles.len() - 1, Item::Action { bound, applies: applies(i) })));
            }
        }
        for (title, keys) in sheet.informs.iter().filter(|(_, keys)| !keys.is_empty()) {
            titles.push(title.clone());
            entries.extend(keys.iter().map(|(keys, what)| (titles.len() - 1, Item::Inform { keys, what })));
        }
        Self { titles, entries, everything }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn item(&self, entry: usize) -> &Item<'a> {
        &self.entries[entry].1
    }

    /// What the ranking reads, as a fingerprint (ranked again only when
    /// it changes) and as the text matched.
    pub fn fingerprint(&self) -> Vec<(usize, &str, &str, &str, bool)> {
        self.entries
            .iter()
            .map(|(section, item)| match item {
                Item::Action { bound, applies } => (*section, bound.action.id.as_str(), bound.action.label.as_str(), bound.action.detail.as_str(), *applies),
                Item::Inform { keys, what } => (*section, *keys, *what, "", true),
            })
            .collect()
    }

    /// The text an entry is matched by: its label, id, the leader group
    /// it's in, its description (an informing key: what it does, the key).
    pub fn haystack(&self, entry: usize, keymap: &KeymapConfig) -> String {
        match &self.entries[entry].1 {
            Item::Action { bound, .. } => {
                let group = bound.leader.as_ref().filter(|l| l.len() > 1).and_then(|l| keymap.group_label(&l[..1])).unwrap_or("");
                format!("{} {} {group} {}", bound.action.label, bound.action.id, bound.action.detail)
            }
            Item::Inform { keys, what } => format!("{what} {keys}"),
        }
    }

    /// The lines for the entries matched (`order`, best first): as they
    /// are; with everything, by section, each under its title, best first
    /// within each, and the sections in the order of their best (nothing
    /// typed: their own order).
    pub fn lines(&self, order: &[usize]) -> Vec<Line> {
        if !self.everything {
            return order.iter().map(|e| Line::Entry(*e)).collect();
        }
        let mut best = vec![usize::MAX; self.titles.len()];
        for (rank, e) in order.iter().enumerate() {
            let section = self.entries[*e].0;
            best[section] = best[section].min(rank);
        }
        let mut order = order.to_vec();
        order.sort_by_key(|e| best[self.entries[*e].0]);
        let mut lines = Vec::with_capacity(order.len() + self.titles.len());
        for e in order {
            let section = self.entries[e].0;
            if lines.last().is_none_or(|l| !matches!(l, Line::Entry(p) if self.entries[*p].0 == section)) {
                lines.push(Line::Title(section));
            }
            lines.push(Line::Entry(e));
        }
        lines
    }

    /// A line as it's drawn: a title; an action (with everything, every
    /// way to run it; faded where it doesn't apply, saying what it needs,
    /// and then picked only when it can say why itself, run); a key that
    /// only informs.
    pub fn row(&self, line: Line) -> Row<'_> {
        match line {
            Line::Title(section) => Row { label: &self.titles[section], kind: RowKind::Heading, ..Row::default() },
            Line::Entry(e) => match &self.entries[e].1 {
                Item::Action { bound, applies } => {
                    let keys = if self.everything { bound.ways().join(" · ") } else { bound.key_hint() };
                    let detail = match bound.action.when.needs().filter(|_| !applies) {
                        Some(needs) => Cow::Owned(format!("Needs {needs} · {}", bound.action.detail)),
                        None => Cow::Borrowed(bound.action.detail.as_str()),
                    };
                    let kind = if *applies || bound.action.param == Param::None { RowKind::Pick } else { RowKind::Inform };
                    Row { label: &bound.action.label, detail: Some(detail), keys: keys.into(), kind, faint: !applies, ..Row::default() }
                }
                Item::Inform { keys, what } => Row { label: what, keys: Cow::Borrowed(keys), kind: RowKind::Inform, ..Row::default() },
            },
        }
    }
}
