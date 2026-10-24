//! What the action list lists: what applies where you are (`:`), or
//! everything (`?`): every action under where it acts (what you're on,
//! this view, this panel, everywhere), in the order of the menus that hold
//! them, faded where it doesn't apply (and what it needs, when that's
//! known); then keys that run nothing themselves (a mode's, the
//! pointer's), each under its section's title. `:42` lists going to line
//! 42 first. Matched by the shared fuzzy matcher; each section keeps its
//! rows in the order they matched.

use std::borrow::Cow;

use crate::actions::{Bound, Group, Layer, Param};
use crate::pick_list::{Row, RowKind};

/// How far the action list reaches: what applies where you are, or
/// everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    #[default]
    Here,
    Everything,
}

impl Reach {
    /// The other (what the list's cycle key switches to).
    pub fn other(self) -> Self {
        match self {
            Self::Here => Self::Everything,
            Self::Everything => Self::Here,
        }
    }
}

/// An action offered here, and whether it applies where you are (judged
/// once, by the app, for every surface).
pub struct Offered {
    pub bound: Bound,
    pub applies: bool,
}

impl Offered {
    /// Asked to run: what's said instead, when it isn't. One that asks for
    /// a value isn't asked for it where it doesn't apply; one that doesn't
    /// runs (its view says why it can't, in its own words), as a switch
    /// flips. (Its key, its control and the action list all ask this.)
    pub fn refused(&self) -> Option<String> {
        let action = &self.bound.action;
        let asks = !matches!(action.param, Param::None | Param::Toggle { .. });
        (asks && !self.applies).then(|| format!("{}: {}.", action.label, action.when.why_not()))
    }
}

/// A key that runs nothing itself, and what it does.
pub struct Inform {
    pub keys: String,
    pub does: String,
}

/// A section of keys that run nothing themselves (a mode's, the pointer's).
pub struct Informs {
    pub title: String,
    pub keys: Vec<Inform>,
}

/// What the list needs to know of where you are, beyond the actions: the
/// titles of what holds you (the panel's view, its host), the keys that
/// run nothing themselves, the leader's groups' names in the menus' order
/// (what its actions are found by too), and going to a line.
pub struct Sheet {
    pub view: String,
    pub host: String,
    pub informs: Vec<Informs>,
    pub groups: Vec<(char, String)>,
    pub line: Option<Bound>,
}

impl Sheet {
    /// A layer's section title.
    pub fn title(&self, layer: Layer) -> &str {
        match layer {
            Layer::On => "On what you're on",
            Layer::Panel => &self.view,
            Layer::Host => &self.host,
            Layer::Everywhere => "Everywhere",
        }
    }

    fn group(&self, key: char) -> &str {
        self.groups.iter().find(|(k, _)| *k == key).map_or("", |(_, name)| name.as_str())
    }
}

/// One thing listed.
pub enum Item<'a> {
    Action(&'a Offered),
    /// Going to the line typed (its number, said as a label).
    Line { number: usize, label: String, bound: &'a Bound },
    Inform(&'a Inform),
}

/// One line of the list: a section's title, or an entry (by its place).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Title(usize),
    Entry(usize),
}

/// A section: its title (none: none drawn), and whether its actions say
/// every way to run them (or only their key).
struct Section {
    title: Option<String>,
    every_way: bool,
}

/// The entries listed, each in its section.
pub struct Listed<'a> {
    sections: Vec<Section>,
    entries: Vec<(usize, Item<'a>)>,
    /// The entry going to the line typed (always first).
    line: Option<usize>,
    sheet: &'a Sheet,
}

impl<'a> Listed<'a> {
    /// What applies, closest to where you are first, then by name, in one
    /// untitled section; or everything, by layer and group (most important
    /// first), then the keys that only inform. Here, a number typed lists
    /// going to that line first.
    pub fn of(offered: &'a [Offered], sheet: &'a Sheet, reach: Reach, typed: &str) -> Self {
        let mut listed = Self { sections: Vec::new(), entries: Vec::new(), line: None, sheet };
        match reach {
            Reach::Here => {
                listed.sections.push(Section { title: None, every_way: false });
                if let Some(number) = typed.trim().parse::<usize>().ok().filter(|n| *n > 0)
                    && let Some(bound) = &sheet.line
                {
                    listed.line = Some(0);
                    listed.entries.push((0, Item::Line { number, label: format!("Go to line {number}"), bound }));
                }
                let mut here: Vec<&Offered> = offered.iter().filter(|o| o.applies).collect();
                here.sort_by(|a, b| (a.bound.layer, &a.bound.action.label).cmp(&(b.bound.layer, &b.bound.action.label)));
                listed.entries.extend(here.into_iter().map(|o| (0, Item::Action(o))));
            }
            Reach::Everything => {
                // In a section, as the menus hold them (a menu's in its
                // order, then those in none), then by what they do.
                let menu = |o: &Offered| o.bound.leader.as_ref().and_then(|l| sheet.groups.iter().position(|(k, _)| Some(k) == l.first())).unwrap_or(usize::MAX);
                for layer in Layer::ALL {
                    let mut these: Vec<&Offered> = offered.iter().filter(|o| o.bound.layer == layer).collect();
                    if these.is_empty() {
                        continue;
                    }
                    these.sort_by_key(|o| (menu(o), Group::ALL.iter().position(|g| *g == o.bound.action.group), o.bound.action.importance));
                    listed.sections.push(Section { title: Some(sheet.title(layer).to_owned()), every_way: true });
                    let section = listed.sections.len() - 1;
                    listed.entries.extend(these.into_iter().map(|o| (section, Item::Action(o))));
                }
                for informs in sheet.informs.iter().filter(|i| !i.keys.is_empty()) {
                    listed.sections.push(Section { title: Some(informs.title.clone()), every_way: false });
                    let section = listed.sections.len() - 1;
                    listed.entries.extend(informs.keys.iter().map(|inform| (section, Item::Inform(inform))));
                }
            }
        }
        listed
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
                Item::Action(o) => (*section, o.bound.action.id.as_str(), o.bound.action.label.as_str(), o.bound.action.detail.as_str(), o.applies),
                Item::Line { label, bound, .. } => (*section, bound.action.id.as_str(), label.as_str(), "", true),
                Item::Inform(inform) => (*section, inform.keys.as_str(), inform.does.as_str(), "", true),
            })
            .collect()
    }

    /// The text an entry is matched by: its label, id, the leader group
    /// it's in, its description (an informing key: what it does, the key).
    pub fn haystack(&self, entry: usize) -> String {
        match &self.entries[entry].1 {
            Item::Action(o) => {
                let action = &o.bound.action;
                let group = o.bound.leader.as_ref().filter(|l| l.len() > 1).map_or("", |l| self.sheet.group(l[0]));
                format!("{} {} {group} {}", action.label, action.id, action.detail)
            }
            Item::Line { label, .. } => label.clone(),
            Item::Inform(inform) => format!("{} {}", inform.does, inform.keys),
        }
    }

    /// The lines for the entries matched (`order`, best first): going to
    /// the line typed first, then by section, each under its title (if it
    /// has one), best first within each, the sections in the order of
    /// their best (nothing typed: their own order).
    pub fn lines(&self, order: &[usize]) -> Vec<Line> {
        let order: Vec<usize> = order.iter().copied().filter(|e| Some(*e) != self.line).collect();
        let mut best = vec![usize::MAX; self.sections.len()];
        for (rank, e) in order.iter().enumerate() {
            let section = self.entries[*e].0;
            best[section] = best[section].min(rank);
        }
        let mut order = order;
        order.sort_by_key(|e| best[self.entries[*e].0]);
        let mut lines: Vec<Line> = self.line.map(Line::Entry).into_iter().collect();
        lines.reserve(order.len() + self.sections.len());
        let mut last = None;
        for e in order {
            let section = self.entries[e].0;
            if last != Some(section) && self.sections[section].title.is_some() {
                lines.push(Line::Title(section));
            }
            last = Some(section);
            lines.push(Line::Entry(e));
        }
        lines
    }

    /// What a line is (without drawing it): a title, an action or the
    /// line to go to (picked), a key that only informs.
    pub fn kind(&self, line: Line) -> RowKind {
        match line {
            Line::Title(_) => RowKind::Heading,
            Line::Entry(e) => match &self.entries[e].1 {
                Item::Action(_) | Item::Line { .. } => RowKind::Pick,
                Item::Inform(_) => RowKind::Inform,
            },
        }
    }

    /// A line as it's drawn: a title; an action (its key, or in a section
    /// that says them, every way to run it; faded where it doesn't apply,
    /// saying what it needs); the line to go to; a key that only informs.
    pub fn row(&self, line: Line) -> Row<'_> {
        match line {
            Line::Title(section) => Row { label: self.sections[section].title.as_deref().unwrap_or_default(), ..Row::default() },
            Line::Entry(e) => {
                let (section, item) = &self.entries[e];
                match item {
                    Item::Action(o) => {
                        let (action, applies) = (&o.bound.action, o.applies);
                        let keys = if self.sections[*section].every_way { o.bound.ways().join(" · ") } else { o.bound.key_hint() };
                        let detail = match action.when.needs().filter(|_| !applies) {
                            Some(needs) => Cow::Owned(format!("Needs {needs} · {}", action.detail)),
                            None => Cow::Borrowed(action.detail.as_str()),
                        };
                        Row { label: &action.label, detail: Some(detail), keys: keys.into(), faint: !applies, ..Row::default() }
                    }
                    Item::Line { label, .. } => Row { label, keys: ":N".into(), ..Row::default() },
                    Item::Inform(inform) => Row { label: &inform.does, keys: Cow::Borrowed(&inform.keys), ..Row::default() },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Action, Origin, PRIMARY, MINOR, When};

    /// An action offered: its id (its label too), layer, leader path,
    /// whether it applies, and what it takes.
    fn offered(id: &str, layer: Layer, leader: &str, applies: bool, param: Param) -> Offered {
        let action = Action::new(id, id, format!("does {id}"), ()).param(param).when(When::Target).importance(if id.ends_with('!') { PRIMARY } else { MINOR });
        let leader = (!leader.is_empty()).then(|| leader.chars().collect());
        Offered { bound: Bound { action, origin: Origin::Shell, leader, keys: Vec::new(), gestures: Vec::new(), layer }, applies }
    }

    fn sheet(line: Option<Bound>) -> Sheet {
        let informs = vec![Informs { title: "Pointer".into(), keys: vec![Inform { keys: "tap".into(), does: "put the cursor there".into() }] }];
        Sheet { view: "This view (Graph)".into(), host: "This panel (1)".into(), informs, groups: vec![(',', "Graph actions".into()), ('b', "Buffer".into())], line }
    }

    /// The labels of `lines`, a title as `# title`.
    fn said(listed: &Listed<'_>, lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| match l {
            Line::Title(_) => format!("# {}", listed.row(*l).label),
            Line::Entry(_) => listed.row(*l).label.to_owned(),
        }).collect()
    }

    fn all(listed: &Listed<'_>) -> Vec<usize> {
        (0..listed.len()).collect()
    }

    /// Here: what applies, closest first then by name, with no titles; a
    /// number typed lists going to that line first.
    #[test]
    fn here_lists_what_applies_closest_first_and_a_number_its_line_first() {
        let actions = [offered("zoom", Layer::On, "", true, Param::None), offered("away", Layer::Everywhere, "", true, Param::None), offered("idle", Layer::On, "", false, Param::None), offered("apply", Layer::On, "", true, Param::None)];
        let line = offered("nav.line", Layer::Everywhere, "", true, Param::Text { hint: String::new(), start: String::new() }).bound;
        let sheet = sheet(Some(line));
        let listed = Listed::of(&actions, &sheet, Reach::Here, "");
        assert_eq!(said(&listed, &listed.lines(&all(&listed))), ["apply", "zoom", "away"]);
        let listed = Listed::of(&actions, &sheet, Reach::Here, " 7 ");
        let lines = listed.lines(&[1, 0, 2, 3]);
        assert_eq!(said(&listed, &lines)[0], "Go to line 7", "first, whatever matched best");
        assert!(matches!(listed.item(0), Item::Line { number: 7, .. }));
        assert_eq!(listed.row(lines[0]).keys, ":N");
    }

    /// Everything: under the four places it acts (titled), each in the
    /// menus' order then by what it does, then the keys that only inform;
    /// the sections in the order of their best match.
    #[test]
    fn everything_is_under_where_it_acts_in_the_menus_order() {
        let actions = [
            offered("in buffer", Layer::Panel, "bx", true, Param::None),
            offered("in graph", Layer::Panel, ",x", true, Param::None),
            offered("in none", Layer::Panel, "", true, Param::None),
            offered("on it", Layer::On, "", true, Param::None),
            offered("everywhere", Layer::Everywhere, "", true, Param::None),
        ];
        let sheet = sheet(None);
        let listed = Listed::of(&actions, &sheet, Reach::Everything, "");
        let order = all(&listed);
        assert_eq!(said(&listed, &listed.lines(&order)), ["# On what you're on", "on it", "# This view (Graph)", "in graph", "in buffer", "in none", "# Everywhere", "everywhere", "# Pointer", "put the cursor there"]);
        // Matched: the sections ordered by their best, each under its title.
        let everywhere = (0..listed.len()).find(|e| listed.haystack(*e).starts_with("everywhere")).unwrap();
        let lines = listed.lines(&[everywhere, 0]);
        assert_eq!(said(&listed, &lines), ["# Everywhere", "everywhere", "# On what you're on", "on it"]);
        assert_eq!((listed.kind(lines[0]), listed.kind(lines[1])), (RowKind::Heading, RowKind::Pick));
        let pointer = listed.lines(&[listed.len() - 1]);
        assert_eq!(listed.kind(pointer[1]), RowKind::Inform, "a key that runs nothing is read, not picked");
        // An action is found by its menu's name.
        let in_graph = (0..listed.len()).find(|e| listed.haystack(*e).starts_with("in graph")).unwrap();
        assert!(listed.haystack(in_graph).contains("Graph actions"), "{}", listed.haystack(in_graph));
    }

    /// One that doesn't apply: listed faded, saying what it needs, picked
    /// like any (what then happens is the one rule's): one that asks for a
    /// value is refused, saying why; one that doesn't isn't.
    #[test]
    fn one_that_doesnt_apply_is_faded_and_refused_only_when_it_asks_for_a_value() {
        let choice = Param::Choice(vec![crate::actions::Choice::new("a", "A")]);
        let actions = [offered("asks", Layer::On, "", false, choice), offered("runs", Layer::On, "", false, Param::None), offered("fine", Layer::On, "", true, Param::None)];
        let sheet = sheet(None);
        let listed = Listed::of(&actions, &sheet, Reach::Everything, "");
        let lines = listed.lines(&all(&listed));
        let row = listed.row(lines[1]);
        assert!(row.faint && row.detail.as_deref().is_some_and(|d| d.starts_with("Needs something under the cursor")), "{row:?}");
        assert_eq!(listed.kind(lines[1]), RowKind::Pick);
        assert_eq!(actions[0].refused().as_deref(), Some("asks: it needs something under the cursor."));
        assert_eq!((actions[1].refused(), actions[2].refused()), (None, None));
        assert!(!listed.row(lines[3]).faint);
    }
}
