//! The observed graph, made from every file's rows:
//! components and relations as linked, plus what belongs to no one file and
//! is made from rows in memory: folders and the module hierarchy, boundary
//! data flows, and which macro uses are placeholders.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arch_graph::{
    CodeAnchor, Component, ComponentId, Graph, Provenance, Relation, RelationId, RelationKind, RepoPath, Span, Stage, Status,
};

use crate::link::{AmbiguousCall, ExternalCall, UnverifiedFlow};
use crate::rows::{Evidence, FileRows, RefKind};
use crate::{FlowKind, PackSet};

/// The graph, and what couldn't be linked.
#[derive(Debug, Default)]
pub struct Observed {
    pub graph: Graph,
    /// Calls proven to mean one of several components.
    pub ambiguous: Vec<AmbiguousCall>,
    /// Calls whose target can't be proven (a receiver of unknown type, a
    /// name that isn't local or imported), with the components sharing the
    /// name. Never turned into relations.
    pub unproven: Vec<AmbiguousCall>,
    pub external: Vec<ExternalCall>,
    pub unverified_flows: Vec<UnverifiedFlow>,
}

/// The names of macros that are placeholders, per language: the pack's,
/// names aliasing one, and macros anywhere expanding to one, to a fixpoint.
pub fn placeholder_macros<'a>(files: impl Iterator<Item = &'a FileRows> + Clone, packs: &PackSet) -> BTreeMap<String, BTreeSet<String>> {
    let mut names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in files.clone() {
        if let Some(m) = packs.by_name(&f.file.language).and_then(|p| p.spec.macros.as_ref()) {
            names.entry(f.file.language.clone()).or_insert_with(|| m.placeholders.iter().cloned().collect());
        }
    }
    loop {
        let mut changed = false;
        for f in files.clone() {
            let (Some(pack), Some(set)) = (packs.by_name(&f.file.language), names.get_mut(&f.file.language)) else { continue };
            let lang = &pack.spec.language;
            let Some(reading) = f.module().and_then(|m| m.detail.module.as_ref()) else { continue };
            for (alias, original) in &reading.aliases {
                let last = lang.words(original).last().copied().unwrap_or(original.as_str());
                if set.contains(last) && !set.contains(alias) {
                    set.insert(alias.clone());
                    changed = true;
                }
            }
            for d in &reading.macro_defs {
                if !set.contains(&d.name) && (d.placeholder || d.uses.iter().any(|u| set.contains(u))) {
                    set.insert(d.name.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            return names;
        }
    }
}

/// A file's placeholder sites: its own, and uses of macros that are placeholders.
pub fn placeholder_sites(file: &FileRows, macros: &BTreeMap<String, BTreeSet<String>>) -> Vec<Span> {
    let set = macros.get(&file.file.language);
    let mut out: Vec<Span> = file
        .placeholders
        .iter()
        .filter(|p| p.macro_name.as_ref().is_none_or(|m| set.is_some_and(|s| s.contains(m))))
        .map(|p| p.span)
        .collect();
    out.sort_by_key(|s| s.start_byte);
    out.dedup();
    out
}

/// One file's components, as the graph has them.
pub fn components_of(file: &FileRows, sites: &[Span]) -> Vec<Component> {
    file.components
        .iter()
        .map(|c| {
            // A placeholder counts for every definition containing it: code
            // with a nested unfinished part isn't finished either.
            let placeholders: Vec<Span> = match c.detail.def {
                Some(_) => sites.iter().filter(|p| c.span.contains(p)).copied().collect(),
                None => Vec::new(),
            };
            Component {
                id: c.id.clone(),
                name: c.name.clone(),
                type_name: c.type_name.clone(),
                description: String::new(),
                status: Status::Existing,
                // Code with placeholder bodies is structure, not an implementation.
                stage: if placeholders.is_empty() { Stage::Implemented } else { Stage::Skeleton },
                anchor: Some(CodeAnchor {
                    language: file.file.language.clone(),
                    file: file.file.path.clone(),
                    symbol: c.detail.symbol.clone(),
                    syntax_kind: c.detail.syntax_kind.clone(),
                    span: c.span,
                    content_hash: file.file.content_hash.clone(),
                }),
                proposed: None,
                signature: c.signature.clone(),
                schemas: Default::default(),
                spec: None,
                placeholders,
            }
        })
        .collect()
}

/// One file's relations, as the graph has them.
pub fn relations_of(file: &FileRows) -> Vec<Relation> {
    file.relations
        .iter()
        .map(|r| {
            let mut relation = Relation::new(r.kind, r.source.clone(), r.target.clone(), Provenance::OBSERVED);
            relation.order = r.detail.order;
            relation.sites = r.detail.sites.clone();
            relation.deferred = r.detail.deferred;
            relation.ports = r.detail.ports.clone();
            relation.data = r.detail.data.clone();
            relation.proven_by = r.prover.clone();
            relation
        })
        .collect()
}

/// Where a component or relation came from: a file's rows, or what's made
/// from everyone's (folders, the module hierarchy, boundaries).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Source {
    File(RepoPath),
    Derived,
}

/// The observed graph, kept up to date by per-file diffs: a changed file's components and relations come out and go back
/// in; only what changed is put into the graph. What belongs to no one file
/// is made again and put in by difference too.
#[derive(Default)]
pub struct Keeper {
    graph: Graph,
    /// Each component as each source has it (two files can make one module).
    components: HashMap<ComponentId, BTreeMap<Source, Component>>,
    relations: HashMap<RelationId, BTreeMap<Source, Relation>>,
    /// What each source put in.
    put: HashMap<Source, (Vec<ComponentId>, Vec<RelationId>)>,
    macros: BTreeMap<String, BTreeSet<String>>,
    unlinked: BTreeMap<RepoPath, Unlinked>,
    /// What each file gives the parts no one file owns.
    shared: HashMap<RepoPath, SharedInputs>,
}

/// What a file gives the parts no one file owns (the module tree, the
/// folders, flows across boundaries, which macros are placeholders): those
/// are made again only when one of these changed.
#[derive(Debug, PartialEq)]
struct SharedInputs {
    module: Option<(String, ComponentId)>,
    aliases: Vec<(String, String)>,
    macro_defs: Vec<crate::extract::MacroDef>,
    boundaries: Vec<crate::rows::FlowRow>,
}

impl SharedInputs {
    fn of(rows: &FileRows) -> Self {
        let module = rows.module();
        let reading = module.and_then(|m| m.detail.module.as_ref());
        Self {
            module: module.map(|m| (m.detail.symbol.clone(), m.id.clone())),
            aliases: reading.map(|r| r.aliases.clone()).unwrap_or_default(),
            macro_defs: reading.map(|r| r.macro_defs.clone()).unwrap_or_default(),
            boundaries: rows.flows.iter().filter(|f| f.flow.boundary.is_some()).cloned().collect(),
        }
    }
}

/// What one file's calls couldn't link.
#[derive(Default)]
struct Unlinked {
    ambiguous: Vec<AmbiguousCall>,
    unproven: Vec<AmbiguousCall>,
    external: BTreeSet<ExternalCall>,
    unverified: Vec<UnverifiedFlow>,
}

impl Keeper {
    /// The graph as it is now, shared (nothing is copied).
    pub fn graph(&self) -> Graph {
        self.graph.share()
    }

    /// Brings the graph up to date with `changed` files' rows in `files`
    /// (gone from `files`: removed).
    pub fn update<'a>(
        &mut self,
        files: impl Iterator<Item = &'a FileRows> + Clone,
        get: impl Fn(&RepoPath) -> Option<&'a FileRows>,
        changed: &BTreeSet<RepoPath>,
        packs: &PackSet,
    ) {
        let mut changed = changed.clone();
        // The parts no one file owns are made again only when what the
        // files give them changed.
        let mut redo = false;
        for path in &changed {
            let now = get(path).map(SharedInputs::of);
            if self.shared.get(path) != now.as_ref() {
                redo = true;
                match now {
                    Some(inputs) => self.shared.insert(path.clone(), inputs),
                    None => self.shared.remove(path),
                };
            }
        }
        // Which macros are placeholders can change with any file: then every
        // file using one is read again for its components.
        if redo {
            let macros = placeholder_macros(files.clone(), packs);
            if macros != self.macros {
                changed
                    .extend(files.clone().filter(|f| f.placeholders.iter().any(|p| p.macro_name.is_some())).map(|f| f.file.path.clone()));
                self.macros = macros;
            }
        }
        let mut dirty_components: BTreeSet<ComponentId> = BTreeSet::new();
        let mut dirty_relations: BTreeSet<RelationId> = BTreeSet::new();
        for path in &changed {
            let source = Source::File(path.clone());
            let (components, relations) = match get(path) {
                Some(rows) => {
                    let sites = placeholder_sites(rows, &self.macros);
                    let mut unlinked = Unlinked::default();
                    unlinked_of(rows, &mut unlinked.ambiguous, &mut unlinked.unproven, &mut unlinked.external, &mut unlinked.unverified);
                    self.unlinked.insert(path.clone(), unlinked);
                    (components_of(rows, &sites), relations_of(rows))
                }
                None => {
                    self.unlinked.remove(path);
                    (Vec::new(), Vec::new())
                }
            };
            self.replace(source, components, relations, &mut dirty_components, &mut dirty_relations);
        }
        if redo {
            let (relations, components): (Vec<Relation>, Vec<Component>) = {
                let made = derived(files, packs);
                (made.iter().filter_map(|(r, _)| r.clone()).collect(), made.into_iter().filter_map(|(_, c)| c).collect())
            };
            self.replace(Source::Derived, components, relations, &mut dirty_components, &mut dirty_relations);
        }
        for id in dirty_components {
            let merged = self.components.get(&id).and_then(|by| fold_components(by.values()));
            match merged {
                None => self.graph.remove_component(&id),
                Some(c) if !self.graph.component(&id).is_some_and(|kept| kept.identical(&c)) => {
                    self.graph.put_component(c, Provenance::OBSERVED)
                }
                Some(_) => {}
            }
        }
        for id in dirty_relations {
            let merged = self.relations.get(&id).and_then(|by| fold_relations(by.values()));
            match merged {
                None => self.graph.remove_relation(&id),
                Some(r) if self.graph.relation(&id) != Some(&r) => self.graph.put_relation(r),
                Some(_) => {}
            }
        }
    }

    /// Replaces what `source` put in; what's affected is marked dirty.
    fn replace(
        &mut self,
        source: Source,
        components: Vec<Component>,
        relations: Vec<Relation>,
        dirty_components: &mut BTreeSet<ComponentId>,
        dirty_relations: &mut BTreeSet<RelationId>,
    ) {
        if let Some((old_components, old_relations)) = self.put.remove(&source) {
            for id in old_components {
                if let Some(by) = self.components.get_mut(&id) {
                    by.remove(&source);
                    if by.is_empty() {
                        self.components.remove(&id);
                    }
                }
                dirty_components.insert(id);
            }
            for id in old_relations {
                if let Some(by) = self.relations.get_mut(&id) {
                    by.remove(&source);
                    if by.is_empty() {
                        self.relations.remove(&id);
                    }
                }
                dirty_relations.insert(id);
            }
        }
        let mut put = (Vec::with_capacity(components.len()), Vec::with_capacity(relations.len()));
        for c in components {
            put.0.push(c.id.clone());
            dirty_components.insert(c.id.clone());
            self.components.entry(c.id.clone()).or_default().insert(source.clone(), c);
        }
        for r in relations {
            let id = r.id();
            put.1.push(id.clone());
            dirty_relations.insert(id.clone());
            let by = self.relations.entry(id).or_default();
            match by.get_mut(&source) {
                Some(existing) => existing.absorb(&r),
                None => {
                    by.insert(source.clone(), r);
                }
            }
        }
        if !put.0.is_empty() || !put.1.is_empty() {
            self.put.insert(source, put);
        }
    }

    /// Every file's calls proven to mean one of several components.
    pub fn ambiguous(&self) -> Vec<AmbiguousCall> {
        self.unlinked.values().flat_map(|u| u.ambiguous.iter().cloned()).collect()
    }

    pub fn unproven(&self) -> Vec<AmbiguousCall> {
        self.unlinked.values().flat_map(|u| u.unproven.iter().cloned()).collect()
    }

    pub fn external(&self) -> Vec<ExternalCall> {
        self.unlinked.values().flat_map(|u| u.external.iter().cloned()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    pub fn unverified(&self) -> Vec<UnverifiedFlow> {
        self.unlinked.values().flat_map(|u| u.unverified.iter().cloned()).collect()
    }
}

/// One component from what each source says, as the graph merges them: the
/// first's intent, the last's place in the code.
fn fold_components<'a>(mut versions: impl Iterator<Item = &'a Component>) -> Option<Component> {
    let mut merged = versions.next()?.clone();
    for next in versions {
        if next.anchor.is_some() {
            merged.anchor = next.anchor.clone();
            merged.placeholders = next.placeholders.clone();
        }
        if merged.signature.is_none() {
            merged.signature = next.signature.clone();
        }
    }
    Some(merged)
}

fn fold_relations<'a>(mut versions: impl Iterator<Item = &'a Relation>) -> Option<Relation> {
    let mut merged = versions.next()?.clone();
    for next in versions {
        merged.absorb(next);
    }
    Some(merged)
}

/// The graph made from every file's rows at once (the tests' check; the
/// server keeps one up to date with `Keeper`).
#[cfg(any(test, feature = "testing"))]
pub fn observed<'a>(files: impl Iterator<Item = &'a FileRows> + Clone, packs: &PackSet) -> Observed {
    let mut keeper = Keeper::default();
    let all: BTreeMap<RepoPath, &FileRows> = files.clone().map(|f| (f.file.path.clone(), f)).collect();
    let changed: BTreeSet<RepoPath> = all.keys().cloned().collect();
    keeper.update(files, |p| all.get(p).copied(), &changed, packs);
    Observed {
        graph: keeper.graph(),
        ambiguous: keeper.ambiguous(),
        unproven: keeper.unproven(),
        external: keeper.external(),
        unverified_flows: keeper.unverified(),
    }
}

/// What a file's calls couldn't link, and why.
fn unlinked_of(
    file: &FileRows,
    ambiguous: &mut Vec<AmbiguousCall>,
    unproven: &mut Vec<AmbiguousCall>,
    external: &mut BTreeSet<ExternalCall>,
    unverified: &mut Vec<UnverifiedFlow>,
) {
    for r in file.refs.iter().filter(|r| r.kind == RefKind::Call) {
        let (Some(within), Some((_, call))) = (&r.within, &r.detail.call) else { continue };
        let callee = call.callee.render();
        match r.evidence {
            Evidence::Ambiguous | Evidence::Unproven => {
                let entry = AmbiguousCall {
                    caller: within.clone(),
                    callee,
                    file: file.file.path.clone(),
                    span: call.span,
                    candidates: r.detail.candidates.clone(),
                };
                if r.evidence == Evidence::Ambiguous { ambiguous.push(entry) } else { unproven.push(entry) }
            }
            Evidence::External => {
                external.insert(ExternalCall { caller: within.clone(), callee });
            }
            _ => {}
        }
        if let Some(target) = &r.target {
            for (source, position, reason) in &r.detail.unverified {
                unverified.push(UnverifiedFlow {
                    source: source.clone(),
                    target: target.clone(),
                    position: *position,
                    file: file.file.path.clone(),
                    span: call.span,
                    reason: reason.clone(),
                });
            }
        }
    }
}

/// What belongs to no one file: the module hierarchy, folders, and data
/// flowing across boundaries. `graph` holds every file's components.
fn derived<'a>(files: impl Iterator<Item = &'a FileRows> + Clone, packs: &PackSet) -> Vec<(Option<Relation>, Option<Component>)> {
    let mut out = Vec::new();
    let contains = |a: &ComponentId, b: &ComponentId| {
        (a != b).then(|| Relation::new(RelationKind::Contains, a.clone(), b.clone(), Provenance::OBSERVED))
    };
    // Module hierarchy: a module contains the modules one segment below it.
    let modules: BTreeMap<(String, String), (ComponentId, RepoPath)> = files
        .clone()
        .flat_map(|f| f.module().map(|m| ((f.file.language.clone(), m.detail.symbol.clone()), (m.id.clone(), f.file.path.clone()))))
        .collect();
    let mut has_parent: BTreeSet<ComponentId> = BTreeSet::new();
    for ((lang, symbol), (id, _)) in &modules {
        let Some(pack) = packs.by_name(lang) else { continue };
        if let Some((parent, _)) = symbol.rsplit_once(pack.spec.language.separator.as_str())
            && let Some((parent_id, _)) = modules.get(&(lang.clone(), parent.to_owned()))
            && let Some(r) = contains(parent_id, id)
        {
            has_parent.insert(id.clone());
            out.push((Some(r), None));
        }
    }
    // Folders: modules nothing else contains sit in the folders their files
    // are in, so the repo reads as folders, crates, modules and members.
    // Each module once, in the first of its files (by path).
    let mut tops: BTreeMap<ComponentId, RepoPath> = BTreeMap::new();
    for f in files.clone() {
        if let Some(m) = f.module() {
            let id = m.id.clone();
            if !has_parent.contains(&id) {
                tops.entry(id).and_modify(|p| *p = p.clone().min(f.file.path.clone())).or_insert_with(|| f.file.path.clone());
            }
        }
    }
    let mut tops: Vec<(ComponentId, RepoPath)> = tops.into_iter().collect();
    tops.sort_by(|a, b| a.1.cmp(&b.1));
    for (folder, parts) in folders(&tops) {
        let id = folder.id.clone();
        out.push((None, Some(folder.component)));
        for part in parts {
            out.push((contains(&id, &part), None));
        }
    }
    // Boundaries: what's sent with a key reaches what receives it.
    let mut senders: BTreeMap<(String, String), BTreeSet<ComponentId>> = BTreeMap::new();
    let mut receivers: BTreeMap<(String, String), BTreeSet<ComponentId>> = BTreeMap::new();
    for f in files {
        for flow in &f.flows {
            let Some(boundary) = &flow.flow.boundary else { continue };
            let key = (boundary.clone(), flow.flow.what.clone());
            match flow.flow.kind {
                FlowKind::Sends => senders.entry(key).or_default().extend(flow.within.clone()),
                FlowKind::Receives => receivers.entry(key).or_default().extend(flow.receivers.iter().cloned()),
                _ => {}
            }
        }
    }
    for (key, from) in &senders {
        for to in receivers.get(key).into_iter().flatten() {
            for source in from {
                if source != to {
                    let mut relation = Relation::new(RelationKind::DataFlow, source.clone(), to.clone(), Provenance::OBSERVED);
                    relation.ports = Some((key.0.clone(), key.1.clone()));
                    relation.data = Some(key.1.clone());
                    out.push((Some(relation), None));
                }
            }
        }
    }
    out
}

/// Where the code is entered: functions named as a language starts a
/// program (`main`), then the receivers of the boundaries its pack marks as
/// entries (a route's handlers; a server's tools the code declares), each
/// once. An event handled inside the program isn't one.
pub fn entry_points<'a>(files: impl Iterator<Item = &'a FileRows>, packs: &PackSet) -> Vec<arch_graph::tree::EntryPoint> {
    let files: Vec<&FileRows> = files.collect();
    use crate::pack::BoundaryEntry;
    let entry_of = |language: &str, boundary: &str| {
        packs.by_name(language).and_then(|p| p.spec.boundaries.iter().find(|b| b.name == boundary)).and_then(|b| b.entry)
    };
    // The keys the code declares, by boundary.
    let declared: BTreeSet<(String, String)> = files
        .iter()
        .flat_map(|f| {
            f.flows
                .iter()
                .filter(|r| r.flow.kind == FlowKind::Sends)
                .filter_map(|r| r.flow.boundary.clone().map(|b| (b, r.flow.what.clone())))
        })
        .collect();
    // The keys each receiver takes, by boundary: one that takes a key no
    // one declares reads keys of its own (a query's fields), it doesn't
    // take the declared ones.
    let mut takes: BTreeMap<(String, ComponentId), BTreeSet<String>> = BTreeMap::new();
    for f in &files {
        for flow in f.flows.iter().filter(|r| r.flow.kind == FlowKind::Receives) {
            let Some(boundary) = &flow.flow.boundary else { continue };
            for id in &flow.receivers {
                takes.entry((boundary.clone(), id.clone())).or_default().insert(flow.flow.what.clone());
            }
        }
    }
    let mut mains = Vec::new();
    let mut receivers: BTreeMap<ComponentId, String> = BTreeMap::new();
    for f in files {
        if let Some(pack) = packs.by_name(&f.file.language) {
            let names = &pack.spec.language.entry_names;
            mains.extend(
                f.components
                    .iter()
                    .filter(|c| c.signature.is_some() && names.contains(&c.name))
                    .map(|c| arch_graph::tree::EntryPoint { id: c.id.clone(), why: c.name.clone() }),
            );
        }
        for flow in f.flows.iter().filter(|r| r.flow.kind == FlowKind::Receives) {
            let Some(boundary) = &flow.flow.boundary else { continue };
            for id in &flow.receivers {
                let counts = match entry_of(&f.file.language, boundary) {
                    Some(BoundaryEntry::Received) => true,
                    Some(BoundaryEntry::Declared) => takes
                        .get(&(boundary.clone(), id.clone()))
                        .is_some_and(|keys| keys.iter().all(|k| declared.contains(&(boundary.clone(), k.clone())))),
                    None => false,
                };
                if counts {
                    receivers.entry(id.clone()).or_insert_with(|| format!("{boundary} {}", flow.flow.what));
                }
            }
        }
    }
    mains.sort_by(|a, b| a.id.cmp(&b.id));
    let mut received: Vec<arch_graph::tree::EntryPoint> = receivers
        .into_iter()
        .filter(|(id, _)| !mains.iter().any(|m| &m.id == id))
        .map(|(id, why)| arch_graph::tree::EntryPoint { id, why })
        .collect();
    received.sort_by(|a, b| a.why.cmp(&b.why).then_with(|| a.id.cmp(&b.id)));
    mains.extend(received);
    mains
}

/// What each named place in a file refers to (for the code viewer): its
/// definitions' names, and the calls linked to something.
pub fn occurrences(file: &FileRows) -> Vec<arch_graph::code::Occurrence> {
    let mut out: Vec<arch_graph::code::Occurrence> = file
        .components
        .iter()
        .filter(|c| c.detail.def.is_some())
        .filter_map(|c| {
            c.name_span.map(|s| arch_graph::code::Occurrence {
                start: s.start_byte,
                end: s.end_byte,
                target: Some(c.id.clone()),
                definition: true,
            })
        })
        .collect();
    out.extend(file.refs.iter().filter(|r| r.kind == RefKind::Call || r.kind == RefKind::Value).filter_map(|r| {
        let (_, call) = r.detail.call.as_ref()?;
        let s = call.name_span?;
        Some(arch_graph::code::Occurrence { start: s.start_byte, end: s.end_byte, target: Some(r.target.clone()?), definition: false })
    }));
    out.sort_by_key(|o| (o.start, o.end));
    out
}

/// A folder holding code: its component and id.
struct Folder {
    id: ComponentId,
    component: Component,
}

/// The folders that hold `modules` (top-level modules and their files),
/// each with what it holds. A folder holding only one thing is that thing
/// (`crates/a/src` with just `lib.rs` is the crate's module); a kept folder
/// is named by its path below the folder above it.
fn folders(modules: &[(ComponentId, RepoPath)]) -> Vec<(Folder, Vec<ComponentId>)> {
    #[derive(Default)]
    struct Dir {
        dirs: BTreeSet<String>,
        modules: Vec<ComponentId>,
    }
    let mut tree: BTreeMap<String, Dir> = BTreeMap::new();
    tree.entry(String::new()).or_default();
    for (id, file) in modules {
        let path = file.as_str();
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
        // Two files can be one module (a crate's lib.rs and main.rs): once.
        let here = &mut tree.entry(dir.clone()).or_default().modules;
        if !here.contains(id) {
            here.push(id.clone());
        }
        let mut child = dir;
        while !child.is_empty() {
            let parent = child.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
            tree.entry(parent.clone()).or_default().dirs.insert(child.clone());
            child = parent;
        }
    }
    enum Item {
        Module(ComponentId),
        Folder(String, Vec<Item>),
    }
    fn fold(path: &str, tree: &BTreeMap<String, Dir>) -> Vec<Item> {
        let Some(dir) = tree.get(path) else { return Vec::new() };
        let mut items: Vec<Item> = dir.modules.iter().cloned().map(Item::Module).collect();
        for child in &dir.dirs {
            let mut inside = fold(child, tree);
            if inside.len() == 1 {
                items.append(&mut inside);
            } else if !inside.is_empty() {
                items.push(Item::Folder(child.clone(), inside));
            }
        }
        items
    }
    fn emit(items: Vec<Item>, above: &str, out: &mut Vec<(Folder, Vec<ComponentId>)>) -> Vec<ComponentId> {
        items
            .into_iter()
            .map(|item| match item {
                Item::Module(id) => id,
                Item::Folder(path, inside) => {
                    let parts = emit(inside, &path, out);
                    let name = path.strip_prefix(above).map_or(path.as_str(), |rest| rest.trim_start_matches('/')).to_owned();
                    let id = ComponentId::for_symbol("folder", &path);
                    let component = Component {
                        id: id.clone(),
                        name,
                        type_name: "folder".into(),
                        description: String::new(),
                        status: Status::Existing,
                        stage: Stage::Implemented,
                        anchor: RepoPath::new(&path).ok().map(|file| CodeAnchor {
                            language: "folder".into(),
                            file,
                            symbol: path.clone(),
                            syntax_kind: "folder".into(),
                            span: Span { start_line: 0, end_line: 0, start_byte: 0, end_byte: 0 },
                            content_hash: String::new(),
                        }),
                        proposed: None,
                        signature: None,
                        schemas: Default::default(),
                        spec: None,
                        placeholders: Vec::new(),
                    };
                    out.push((Folder { id: id.clone(), component }, parts));
                    id
                }
            })
            .collect()
    }
    let mut out = Vec::new();
    emit(fold("", &tree), "", &mut out);
    out
}
