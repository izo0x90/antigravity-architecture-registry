//! Seeing a view at a coarser grain: each part is drawn as what holds it at
//! the chosen level (its type, its module, its package, its folder), and the
//! lines between parts are merged onto those holders. A holder can be
//! opened in place to show what it holds, a level finer, down to the parts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{LayoutKind, View, ViewEdge, ViewNode};
use crate::{Alignment, Component, ComponentId, RelationId, RelationKind};

/// One level: which component types stand for it, and which of a part's
/// holders of those types (the nearest, or the outermost).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GrainSpec {
    pub name: String,
    pub types: Vec<String>,
    #[serde(default)]
    pub pick: Pick,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pick {
    #[default]
    Nearest,
    Outermost,
}

impl GrainSpec {
    /// What holds `node` at this level (itself, when it is of the level's types).
    pub fn holder(&self, node: &ViewNode, components: &BTreeMap<ComponentId, Component>) -> Option<ComponentId> {
        let of_level = |id: &ComponentId| components.get(id).is_some_and(|c| self.types.iter().any(|t| *t == c.type_name));
        let mut chain = std::iter::once(&node.id).chain(&node.ancestors).filter(|id| of_level(id));
        match self.pick {
            Pick::Nearest => chain.next().cloned(),
            Pick::Outermost => chain.last().cloned(),
        }
    }
}

/// The levels `view` can be seen at: those that would group some of its
/// parts, each differently from the level below it (a level that changes
/// nothing isn't offered).
pub fn levels<'a>(view: &View, components: &BTreeMap<ComponentId, Component>, grains: &'a [GrainSpec]) -> Vec<&'a GrainSpec> {
    // Each part's group at a level: its holder there, or (with none) its
    // group from the level below: a coarser level never splits a group.
    let parts: Vec<ComponentId> = view.nodes.iter().map(|n| n.id.clone()).collect();
    let mut out: Vec<&GrainSpec> = Vec::new();
    let mut below = parts;
    for g in grains {
        let now: Vec<ComponentId> = view.nodes.iter().zip(&below).map(|(n, b)| g.holder(n, components).unwrap_or_else(|| b.clone())).collect();
        if now != below {
            out.push(g);
            below = now;
        }
    }
    out
}

/// `view` seen at `levels[at]` (with the finer levels below it to open
/// into): every part sits inside its holder at that level, and inside the
/// holders at each finer level that's opened, so a closed holder stands
/// for all of it. The parts and their relations are all still there; what's
/// drawn (and merged) is up to which holders are open.
pub fn regroup(view: &View, components: &BTreeMap<ComponentId, Component>, levels: &[&GrainSpec], at: usize) -> View {
    let mut out = view.clone();
    out.spec.layout = LayoutKind::Nested;
    out.edges.retain(|e| e.kind != RelationKind::Contains);
    let mut nodes: BTreeMap<ComponentId, ViewNode> = view.nodes.iter().map(|n| (n.id.clone(), n.clone())).collect();
    let mut contains: BTreeSet<(ComponentId, ComponentId)> = BTreeSet::new();
    let mut members: BTreeMap<ComponentId, BTreeSet<ComponentId>> = BTreeMap::new();
    for node in &view.nodes {
        // The holders from the chosen level down to the finest, each once,
        // outermost first; the part sits in the innermost.
        let mut path: Vec<ComponentId> = Vec::new();
        for grain in levels[..=at.min(levels.len().saturating_sub(1))].iter().rev() {
            if let Some(h) = grain.holder(node, components).filter(|h| *h != node.id && !path.contains(h)) {
                path.push(h);
            }
        }
        let mut outer: Option<ComponentId> = None;
        for holder in path.into_iter().chain(std::iter::once(node.id.clone())) {
            if let Some(o) = &outer
                && *o != holder
            {
                contains.insert((o.clone(), holder.clone()));
                members.entry(o.clone()).or_default().insert(holder.clone());
            }
            if !nodes.contains_key(&holder) {
                let ancestors = node.ancestors.iter().skip_while(|a| **a != holder).skip(1).cloned().collect();
                nodes.insert(holder.clone(), ViewNode { id: holder.clone(), depth: None, truncated: false, alignment: Alignment::Observed, members: 0, ancestors });
            }
            outer = Some(holder);
        }
    }
    for (id, inside) in &members {
        if let Some(n) = nodes.get_mut(id) {
            n.members = inside.len() as u32;
        }
    }
    out.nodes = nodes.into_values().collect();
    out.edges.extend(contains.into_iter().map(|(a, b)| ViewEdge {
        id: RelationId(format!("contains:{a}->{b}")),
        kind: RelationKind::Contains,
        source: a,
        target: b,
        alignment: Alignment::Observed,
        overlay: false,
        order: None,
        carries: None,
        unconfirmed: None,
    }));
    // Roots stay what they were (the parts the view started from).
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::scene::tests::{metrics, sound};
    use crate::view::scene::{End, Options, build};
    use crate::view::tests::{id, sample};
    use crate::view::{ViewCatalog, query};

    // req: G.13
    #[test]
    fn calls_seen_by_module_merge_onto_modules_and_open_in_place() {
        let mut g = sample();
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::step"), id("lib::util"), crate::Provenance::OBSERVED));
        g.upsert_relation(crate::Relation::new(RelationKind::Calls, id("app::save"), id("lib::util"), crate::Provenance::OBSERVED));
        let catalog = ViewCatalog::defaults().unwrap();
        let view = query(&g, catalog.get("calls").unwrap());
        let components: BTreeMap<ComponentId, Component> = g.components().map(|c| (c.id.clone(), c.clone())).chain(g.components().map(|c| (c.id.clone(), c.clone()))).collect();
        let offered = levels(&view, &components, &catalog.grains);
        let names: Vec<&str> = offered.iter().map(|g| g.name.as_str()).collect();
        assert!(names.contains(&"modules"), "functions sit in modules: {names:?}");
        let at = offered.iter().position(|g| g.name == "modules").unwrap();
        let grouped = regroup(&view, &components, &offered, at);
        // Shut, the modules stand for their functions: one line app -> lib, counting both calls.
        let shut = build(&grouped, &Options { open: &BTreeSet::new(), selection: None, max_cards: 60, show_all: false, shut: &BTreeSet::new() }, &crate::view::scene::tests::Names, &metrics(), None);
        sound(&shut);
        assert!(shut.rects.contains_key(&id("app")) && shut.rects.contains_key(&id("lib")) && !shut.rects.contains_key(&id("app::step")));
        let across: Vec<_> = shut.lines.iter().filter(|l| [&l.from, &l.to].contains(&&End::Node(id("lib")))).collect();
        assert_eq!(across.len(), 1);
        assert_eq!((&across[0].from, &across[0].to, across[0].count), (&End::Node(id("app")), &End::Node(id("lib")), 2));
        assert_eq!(shut.rects.len(), 2, "two modules, nothing else: {:?}", shut.rects.keys().collect::<Vec<_>>());
        // Opened in place: app's functions inside it, the calls inside it drawn,
        // and the line to lib starting from the two that call it.
        let open: BTreeSet<ComponentId> = [id("app")].into();
        let opened = build(&grouped, &Options { open: &open, selection: None, max_cards: 60, show_all: false, shut: &BTreeSet::new() }, &crate::view::scene::tests::Names, &metrics(), Some(&shut));
        sound(&opened);
        for f in ["app::run", "app::step", "app::save", "app::log"] {
            assert!(opened.rects[&id("app")].contains(&opened.rects[&id(f)]), "{f} inside app");
        }
        let into_lib: BTreeSet<End> = opened.lines.iter().filter(|l| l.to == End::Node(id("lib"))).map(|l| l.from.clone()).collect();
        assert_eq!(into_lib, [End::Node(id("app::step")), End::Node(id("app::save"))].into());
        assert!(opened.lines.iter().any(|l| l.from == End::Node(id("app::run")) && l.to == End::Node(id("app::step"))), "calls within app");
    }
}
