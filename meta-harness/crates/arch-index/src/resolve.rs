//! Cross-file resolution: turn raw extracts into the observed graph. Gives
//! every definition its symbol, links containment and implements, and
//! resolves calls by evidence (self, qualified path, same module, imports,
//! unique name). Ambiguous calls are recorded with their candidates; calls to
//! nothing in the repo are recorded as external. Nothing is guessed.

use std::collections::{BTreeMap, BTreeSet};

use arch_graph::{CodeAnchor, Component, ComponentId, Graph, Provenance, Relation, RelationKind, RepoPath, Span, Stage, Status, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::pack::LanguageSpec;
use crate::{FileExtract, PackSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbiguousCall {
    pub caller: ComponentId,
    pub callee: String,
    pub file: RepoPath,
    pub span: Span,
    pub candidates: Vec<ComponentId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCall {
    pub caller: ComponentId,
    pub callee: String,
}

#[derive(Debug, Default)]
pub struct Resolved {
    pub graph: Graph,
    pub ambiguous: Vec<AmbiguousCall>,
    pub external: Vec<ExternalCall>,
}

struct Entry {
    id: ComponentId,
    symbol: String,
    name: String,
    module: String,
    file: usize,
    /// (file, definition) it came from; None for a file's module.
    def: Option<(usize, usize)>,
    /// The component it's declared in (owner type for scoped definitions).
    owner: Option<usize>,
    /// Can own members that `self` refers to (a non-executable, non-module type).
    is_type: bool,
}

impl Entry {
    fn is_module(&self) -> bool {
        self.def.is_none()
    }
}

/// Strips generics/arguments and reference markers from a type or callee text.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0i32;
    for ch in text.chars() {
        match ch {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().trim_start_matches(['&', '*']).trim_start_matches("mut ").trim_start_matches("dyn ").trim().to_owned()
}

fn split(text: &str, separators: &[String]) -> Vec<String> {
    let mut parts = vec![clean(text)];
    for sep in separators {
        parts = parts.iter().flat_map(|p| p.split(sep.as_str()).map(str::to_owned).collect::<Vec<_>>()).collect();
    }
    parts.into_iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()
}

fn symbol_words(symbol: &str) -> Vec<&str> {
    symbol.split(|ch: char| !(ch.is_alphanumeric() || ch == '_')).filter(|s| !s.is_empty()).collect()
}

pub fn resolve(extracts: &[FileExtract], packs: &PackSet, types: &TypeSystem) -> Resolved {
    let mut out = Resolved::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut def_entry: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut module_entry: BTreeMap<usize, usize> = BTreeMap::new();
    let is_type = |type_name: &str| types.resolve(type_name).map(|t| !t.executable && !t.is_a("module")).unwrap_or(false);

    for (fi, ex) in extracts.iter().enumerate() {
        let sep = packs.by_name(&ex.language).map_or("::", |p| p.spec.language.separator.as_str());
        let name = ex.module.rsplit(sep).next().unwrap_or(&ex.module).to_owned();
        module_entry.insert(fi, entries.len());
        entries.push(Entry {
            id: ComponentId::for_symbol(&ex.language, &ex.module),
            symbol: ex.module.clone(),
            name,
            module: ex.module.clone(),
            file: fi,
            def: None,
            owner: None,
            is_type: false,
        });
    }

    // Symbols: definitions outside scopes first (types), then scoped ones and
    // everything nested in them, since a scope needs its owner type known.
    let scoped_chain = |fi: usize, di: usize| {
        let defs = &extracts[fi].definitions;
        let mut current = Some(di);
        while let Some(i) = current {
            if defs[i].scope.is_some() {
                return true;
            }
            current = defs[i].parent;
        }
        false
    };
    let mut pending: Vec<(usize, usize)> = extracts.iter().enumerate().flat_map(|(fi, ex)| (0..ex.definitions.len()).map(move |di| (fi, di))).collect();
    for allow_scoped in [false, true] {
        loop {
            let before = pending.len();
            let mut still = Vec::new();
            for (fi, di) in std::mem::take(&mut pending) {
                let ex = &extracts[fi];
                let def = &ex.definitions[di];
                let Some(pack) = packs.by_name(&ex.language) else { continue };
                let lang = &pack.spec.language;
                let parent_entry = def.parent.and_then(|p| def_entry.get(&(fi, p)).copied());
                if (!allow_scoped && scoped_chain(fi, di)) || (def.parent.is_some() && parent_entry.is_none()) {
                    still.push((fi, di));
                    continue;
                }
                let (prefix, owner) = match (&def.scope, parent_entry) {
                    (Some(scope), _) => match find_type(&entries, &scope.target, &ex.module, lang) {
                        Some(owner) => (entries[owner].symbol.clone(), Some(owner)),
                        None => (format!("{}{}{}", ex.module, lang.separator, clean(&scope.target)), None),
                    },
                    (None, Some(parent)) => (entries[parent].symbol.clone(), Some(parent)),
                    (None, None) => (ex.module.clone(), None),
                };
                let symbol = format!("{prefix}{}{}", lang.separator, def.name);
                def_entry.insert((fi, di), entries.len());
                entries.push(Entry {
                    id: ComponentId::for_symbol(&ex.language, &symbol),
                    symbol,
                    name: def.name.clone(),
                    module: ex.module.clone(),
                    file: fi,
                    def: Some((fi, di)),
                    owner,
                    is_type: is_type(&def.type_name),
                });
            }
            pending = still;
            if pending.len() == before || pending.is_empty() {
                break;
            }
        }
    }

    // Components.
    for entry in &entries {
        let ex = &extracts[entry.file];
        let def = entry.def.map(|(fi, di)| &extracts[fi].definitions[di]);
        let (type_name, syntax_kind, span, signature) = match def {
            Some(d) => (d.type_name.clone(), d.syntax_kind.clone(), d.span, d.signature.clone()),
            None => ("module".to_owned(), "source_file".to_owned(), Span { start_line: 1, end_line: ex.lines, start_byte: 0, end_byte: ex.bytes }, None),
        };
        // A placeholder belongs to the innermost definition containing it.
        let placeholders = match def {
            Some(_) => ex
                .placeholders
                .iter()
                .filter(|p| span.contains(p) && !ex.definitions.iter().any(|o| o.span != span && span.contains(&o.span) && o.span.contains(p)))
                .copied()
                .collect(),
            None => Vec::new(),
        };
        out.graph.upsert_component(
            Component {
                id: entry.id.clone(),
                name: entry.name.clone(),
                type_name,
                description: String::new(),
                status: Status::Existing,
                // Code with placeholder bodies is structure, not an implementation.
                stage: if placeholders.is_empty() { Stage::Implemented } else { Stage::Skeleton },
                anchor: Some(CodeAnchor {
                    language: ex.language.clone(),
                    file: ex.file.clone(),
                    symbol: entry.symbol.clone(),
                    syntax_kind,
                    span,
                    content_hash: ex.content_hash.clone(),
                }),
                proposed: None,
                signature,
                spec: None,
                placeholders,
            },
            Provenance::OBSERVED,
        );
    }

    let link = |graph: &mut Graph, kind: RelationKind, a: &ComponentId, b: &ComponentId| {
        if a != b {
            graph.upsert_relation(Relation::new(kind, a.clone(), b.clone(), Provenance::OBSERVED));
        }
    };
    // Module hierarchy: a module contains the modules one segment below it.
    let modules: BTreeMap<(&str, &str), &ComponentId> =
        entries.iter().filter(|e| e.is_module()).map(|e| ((extracts[e.file].language.as_str(), e.symbol.as_str()), &e.id)).collect();
    for entry in entries.iter().filter(|e| e.is_module()) {
        let lang = extracts[entry.file].language.as_str();
        let Some(pack) = packs.by_name(lang) else { continue };
        if let Some((parent, _)) = entry.symbol.rsplit_once(pack.spec.language.separator.as_str())
            && let Some(parent_id) = modules.get(&(lang, parent))
        {
            link(&mut out.graph, RelationKind::Contains, parent_id, &entry.id);
        }
    }
    // Definitions are contained by their owner, else by their file's module.
    for (&(fi, _), &ei) in &def_entry {
        let container = match entries[ei].owner {
            Some(owner) => entries[owner].id.clone(),
            None => entries[module_entry[&fi]].id.clone(),
        };
        link(&mut out.graph, RelationKind::Contains, &container, &entries[ei].id);
    }
    // Implements: declared bases and scope interfaces.
    for (&(fi, di), &ei) in &def_entry {
        let ex = &extracts[fi];
        let Some(pack) = packs.by_name(&ex.language) else { continue };
        let def = &ex.definitions[di];
        let mut pairs: Vec<(usize, &str)> = def.bases.iter().map(|b| (ei, b.as_str())).collect();
        if let (Some(scope), Some(owner)) = (&def.scope, entries[ei].owner)
            && let Some(iface) = &scope.implements
        {
            pairs.push((owner, iface.as_str()));
        }
        for (source, text) in pairs {
            if let Some(target) = find_type(&entries, text, &ex.module, &pack.spec.language)
                && target != source
            {
                link(&mut out.graph, RelationKind::Implements, &entries[source].id, &entries[target].id);
            }
        }
    }

    // Calls.
    let mut seen_external: BTreeSet<(ComponentId, String)> = BTreeSet::new();
    for (fi, ex) in extracts.iter().enumerate() {
        let Some(pack) = packs.by_name(&ex.language) else { continue };
        let mut order: BTreeMap<ComponentId, u32> = BTreeMap::new();
        for call in &ex.calls {
            let caller = match call.caller {
                Some(di) => def_entry.get(&(fi, di)).copied(),
                None => Some(module_entry[&fi]),
            };
            let Some(caller) = caller else { continue };
            let caller_id = entries[caller].id.clone();
            match resolve_call(&entries, caller, &call.callee, &ex.imports, &pack.spec.language) {
                CallTarget::One(target) => {
                    let mut relation = Relation::new(RelationKind::Calls, caller_id.clone(), entries[target].id.clone(), Provenance::OBSERVED);
                    let next = order.entry(caller_id).or_insert(0);
                    relation.order = Some(*next);
                    *next += 1;
                    relation.sites.push(call.span);
                    out.graph.upsert_relation(relation);
                }
                CallTarget::Many(candidates) => out.ambiguous.push(AmbiguousCall {
                    caller: caller_id,
                    callee: call.callee.clone(),
                    file: ex.file.clone(),
                    span: call.span,
                    candidates: candidates.iter().map(|&c| entries[c].id.clone()).collect(),
                }),
                CallTarget::External => {
                    if seen_external.insert((caller_id.clone(), call.callee.clone())) {
                        out.external.push(ExternalCall { caller: caller_id, callee: call.callee.clone() });
                    }
                }
            }
        }
    }
    out
}

/// Finds a type component (one that can own members) by its written name.
fn find_type(entries: &[Entry], text: &str, module: &str, lang: &LanguageSpec) -> Option<usize> {
    let parts: Vec<String> = split(text, &lang.call_separators).into_iter().filter(|p| !lang.relative_prefixes.contains(p)).collect();
    let name = parts.last()?;
    let candidates: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| e.is_type && &e.name == name).map(|(i, _)| i).collect();
    if candidates.len() == 1 {
        return candidates.first().copied();
    }
    let by_path = path_matches(entries, &candidates, &parts);
    if by_path.len() == 1 {
        return by_path.first().copied();
    }
    let local: Vec<usize> = candidates.into_iter().filter(|&c| entries[c].module == module).collect();
    if local.len() == 1 { local.first().copied() } else { None }
}

/// Candidates whose symbol ends with the written qualified path.
fn path_matches(entries: &[Entry], candidates: &[usize], parts: &[String]) -> Vec<usize> {
    if parts.len() < 2 {
        return Vec::new();
    }
    let wanted: Vec<&str> = parts.iter().map(String::as_str).collect();
    candidates.iter().copied().filter(|&c| symbol_words(&entries[c].symbol).ends_with(&wanted)).collect()
}

enum CallTarget {
    One(usize),
    Many(Vec<usize>),
    External,
}

fn narrow(list: Vec<usize>) -> Option<CallTarget> {
    match list.len() {
        0 => None,
        1 => Some(CallTarget::One(list[0])),
        _ => Some(CallTarget::Many(list)),
    }
}

/// The type `self` means inside `entry`: the nearest owner that is a type.
fn self_type(entries: &[Entry], entry: usize) -> Option<usize> {
    let mut current = entries[entry].owner;
    while let Some(o) = current {
        if entries[o].is_type {
            return Some(o);
        }
        current = entries[o].owner;
    }
    None
}

fn resolve_call(entries: &[Entry], caller: usize, callee: &str, imports: &[String], lang: &LanguageSpec) -> CallTarget {
    let parts = split(callee, &lang.call_separators);
    let Some(name) = parts.last().cloned() else { return CallTarget::External };
    let qualifier = &parts[..parts.len() - 1];
    let candidates: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| !e.is_module() && e.name == name).map(|(i, _)| i).collect();
    if candidates.is_empty() {
        return CallTarget::External;
    }
    let is_member = |c: usize| entries[c].owner.is_some_and(|o| entries[o].is_type);

    if let Some(first) = qualifier.first() {
        // self.method() / Self::method(): members of the caller's own type.
        if qualifier.len() == 1 && lang.self_names.contains(first) {
            let own = self_type(entries, caller);
            let members: Vec<usize> = candidates.iter().copied().filter(|&c| own.is_some() && entries[c].owner == own).collect();
            return narrow(members).unwrap_or(CallTarget::External);
        }
        let path: Vec<String> = parts.iter().filter(|p| !lang.relative_prefixes.contains(p)).cloned().collect();
        if let Some(target) = narrow(path_matches(entries, &candidates, &path)) {
            return target;
        }
        // A type or path qualifier that matched nothing is outside the repo
        // (e.g. Vec::new). A lowercase receiver (store.save()) has an unknown
        // type: only members can be meant.
        let via_path_syntax = lang.separator != "." && callee.contains(lang.separator.as_str());
        let names_module = entries.iter().any(|e| e.is_module() && &e.name == first);
        let receiver = qualifier.len() == 1
            && first.chars().next().is_some_and(char::is_lowercase)
            && !via_path_syntax
            && !(lang.separator == "." && names_module);
        if !receiver {
            return CallTarget::External;
        }
        return narrow(candidates.into_iter().filter(|&c| is_member(c)).collect()).unwrap_or(CallTarget::External);
    }

    // Bare calls reach free functions or types (constructors), not members.
    let free: Vec<usize> = candidates.into_iter().filter(|&c| !is_member(c)).collect();
    let caller_module = entries[caller].module.as_str();
    if let Some(target) = narrow(free.iter().copied().filter(|&c| entries[c].module == caller_module).collect()) {
        return target;
    }
    let imported: Vec<usize> = free
        .iter()
        .copied()
        .filter(|&c| {
            let module_tail = symbol_words(&entries[c].module).last().copied().unwrap_or("");
            imports.iter().any(|imp| symbol_words(imp).contains(&name.as_str()) && symbol_words(imp).contains(&module_tail))
        })
        .collect();
    if let Some(target) = narrow(imported) {
        return target;
    }
    narrow(free).unwrap_or(CallTarget::External)
}
