//! Cross-file resolution: turn raw extracts into the observed graph. Gives
//! every definition its symbol, links containment and implements, and
//! resolves calls by evidence (self and typed receivers, qualified paths,
//! same module, imports and aliases), within one language. Calls proven to
//! mean one of several components are ambiguous; calls that can't be proven
//! (unknown receiver type, a name neither local nor imported) are unproven;
//! calls to nothing in the repo are external. Nothing is guessed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use arch_graph::{CodeAnchor, Component, ComponentId, Graph, Provenance, Relation, RelationKind, RepoPath, Span, Stage, Status, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::pack::LanguageSpec;
use crate::{FileExtract, FlowKind, PackSet, RawCall};

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

/// A value passed from one repo call into another whose types can't confirm
/// the flow (missing or mismatched types). Reported, never added as a relation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnverifiedFlow {
    pub source: ComponentId,
    pub target: ComponentId,
    /// The target's argument position.
    pub position: u32,
    pub file: RepoPath,
    pub span: Span,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct Resolved {
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

struct Entry {
    id: ComponentId,
    symbol: String,
    name: String,
    module: String,
    language: String,
    file: usize,
    /// (file, definition) it came from; None for a file's module.
    def: Option<(usize, usize)>,
    /// The component it's declared in (owner type for scoped definitions).
    owner: Option<usize>,
    /// Can own members that `self` refers to (a non-executable, non-module type).
    is_type: bool,
    /// A type whose values carry data (it has properties): made, and holds state.
    holds_data: bool,
    /// What the owner value is called inside it, when the language names it
    /// per definition (a Go receiver).
    self_name: Option<String>,
}

impl Entry {
    fn is_module(&self) -> bool {
        self.def.is_none()
    }
}

/// Lookups over the entries, kept as they're added, so resolution asks a
/// table instead of scanning every entry for every name.
#[derive(Default)]
struct Index {
    /// (language, name) -> the definitions with that name, in entry order.
    named: HashMap<(String, String), Vec<usize>>,
    /// (owner, name) -> the members of `owner` with that name.
    members: HashMap<(usize, String), Vec<usize>>,
    /// (language, symbol) of every module.
    modules: HashSet<(String, String)>,
}

impl Index {
    fn add(&mut self, i: usize, e: &Entry) {
        if e.is_module() {
            self.modules.insert((e.language.clone(), e.symbol.clone()));
            return;
        }
        self.named.entry((e.language.clone(), e.name.clone())).or_default().push(i);
        if let Some(owner) = e.owner {
            self.members.entry((owner, e.name.clone())).or_default().push(i);
        }
    }

    /// Definitions (not modules) of `language` named `name`, in entry order.
    fn named(&self, language: &str, name: &str) -> &[usize] {
        self.named.get(&(language.to_owned(), name.to_owned())).map_or(&[], Vec::as_slice)
    }

    fn members(&self, owner: usize, name: &str) -> &[usize] {
        self.members.get(&(owner, name.to_owned())).map_or(&[], Vec::as_slice)
    }
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
                        anchor: RepoPath::new(&path).ok().map(|file| CodeAnchor { language: "folder".into(), file, symbol: path.clone(), syntax_kind: "folder".into(), span: Span { start_line: 0, end_line: 0, start_byte: 0, end_byte: 0 }, content_hash: String::new() }),
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

/// Strips bracketed groups (generics, arguments) and leading reference
/// markers/noise words from a type or callee text.
fn clean(text: &str, lang: &LanguageSpec) -> String {
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
    lang.strip_references(&out).trim().to_owned()
}

fn split(text: &str, lang: &LanguageSpec) -> Vec<String> {
    let mut parts = vec![clean(text, lang)];
    for sep in &lang.call_separators {
        parts = parts.iter().flat_map(|p| p.split(sep.as_str()).map(str::to_owned).collect::<Vec<_>>()).collect();
    }
    parts.into_iter().map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect()
}

/// `text` without a leading constructor keyword (`new Greeter` -> `Greeter`).
fn strip_constructor<'a>(text: &'a str, lang: &LanguageSpec) -> &'a str {
    let text = text.trim();
    for prefix in &lang.constructor_prefixes {
        if let Some(rest) = text.strip_prefix(prefix.as_str())
            && rest.starts_with(char::is_whitespace)
        {
            return rest.trim_start();
        }
    }
    text
}

/// `text` with a leading alias replaced by what it stands for (its last name).
fn de_alias(text: &str, aliases: &[(String, String)], lang: &LanguageSpec) -> String {
    let text = text.trim();
    let head_end = lang.call_separators.iter().filter_map(|sep| text.find(sep.as_str())).min().unwrap_or(text.len());
    let head = &text[..head_end];
    match aliases.iter().find(|(alias, _)| alias == head).and_then(|(_, original)| lang.words(original).last().map(|w| (*w).to_owned())) {
        Some(original) => format!("{original}{}", &text[head_end..]),
        None => text.to_owned(),
    }
}

/// Every file's placeholder sites: its own, plus uses of macros that are
/// placeholders. A name is one when the pack says so, when it aliases one
/// (`use std::todo as later`), or when a definition anywhere in the repo
/// (same language) expands to one; worked out to a fixpoint.
fn placeholder_sites(extracts: &[FileExtract], packs: &PackSet) -> Vec<Vec<Span>> {
    let mut names: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for ex in extracts {
        if let Some(m) = packs.by_name(&ex.language).and_then(|p| p.spec.macros.as_ref()) {
            names.entry(ex.language.as_str()).or_insert_with(|| m.placeholders.iter().cloned().collect());
        }
    }
    loop {
        let mut changed = false;
        for ex in extracts {
            let (Some(pack), Some(set)) = (packs.by_name(&ex.language), names.get_mut(ex.language.as_str())) else { continue };
            let lang = &pack.spec.language;
            for (alias, original) in &ex.aliases {
                let last = lang.split_last(original).map_or(original.as_str(), |(_, _, n)| n);
                if set.contains(last) && !set.contains(alias) {
                    set.insert(alias.clone());
                    changed = true;
                }
            }
            for d in &ex.macro_defs {
                if !set.contains(&d.name) && (d.placeholder || d.uses.iter().any(|u| set.contains(u))) {
                    set.insert(d.name.clone());
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    extracts
        .iter()
        .map(|ex| {
            let set = names.get(ex.language.as_str());
            let mut out = ex.placeholders.clone();
            out.extend(ex.macro_uses.iter().filter(|(n, _)| set.is_some_and(|s| s.contains(n))).map(|(_, span)| *span));
            out.sort_by_key(|s| s.start_byte);
            out.dedup();
            out
        })
        .collect()
}

pub fn resolve(extracts: &[FileExtract], packs: &PackSet, types: &TypeSystem) -> Resolved {
    let mut out = Resolved::default();
    let mut entries: Vec<Entry> = Vec::new();
    let mut index = Index::default();
    let mut def_entry: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    let mut module_entry: BTreeMap<usize, usize> = BTreeMap::new();
    let module_types: BTreeSet<&str> = packs.packs.iter().map(|p| p.spec.language.module_type.as_str()).collect();
    let is_type = |type_name: &str| types.resolve(type_name).map(|t| !t.executable && !module_types.iter().any(|m| t.is_a(m))).unwrap_or(false);

    for (fi, ex) in extracts.iter().enumerate() {
        let sep = packs.by_name(&ex.language).map_or("::", |p| p.spec.language.separator.as_str());
        let name = ex.module.rsplit(sep).next().unwrap_or(&ex.module).to_owned();
        module_entry.insert(fi, entries.len());
        let entry = Entry {
            id: ComponentId::for_symbol(&ex.language, &ex.module),
            symbol: ex.module.clone(),
            name,
            module: ex.module.clone(),
            language: ex.language.clone(),
            file: fi,
            def: None,
            owner: None,
            is_type: false,
            holds_data: false,
            self_name: None,
        };
        index.add(entries.len(), &entry);
        entries.push(entry);
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
                    (Some(scope), _) => match find_type(&entries, &index, &scope.target, ex, lang).found() {
                        Some(owner) => (entries[owner].symbol.clone(), Some(owner)),
                        None => (format!("{}{}{}", ex.module, lang.separator, clean(&scope.target, lang)), None),
                    },
                    (None, Some(parent)) => (entries[parent].symbol.clone(), Some(parent)),
                    (None, None) => (ex.module.clone(), None),
                };
                let symbol = format!("{prefix}{}{}", lang.separator, def.name);
                def_entry.insert((fi, di), entries.len());
                let entry = Entry {
                    id: ComponentId::for_symbol(&ex.language, &symbol),
                    symbol,
                    name: def.name.clone(),
                    module: ex.module.clone(),
                    language: ex.language.clone(),
                    file: fi,
                    def: Some((fi, di)),
                    owner,
                    is_type: is_type(&def.type_name),
                    holds_data: types.resolve(&def.type_name).is_ok_and(|t| t.allows_properties),
                    self_name: def.self_name.clone(),
                };
                index.add(entries.len(), &entry);
                entries.push(entry);
            }
            pending = still;
            if pending.len() == before || pending.is_empty() {
                break;
            }
        }
    }

    let sites = placeholder_sites(extracts, packs);
    // Components.
    for entry in &entries {
        let ex = &extracts[entry.file];
        let def = entry.def.map(|(fi, di)| &extracts[fi].definitions[di]);
        let (type_name, syntax_kind, span, signature) = match def {
            Some(d) => (d.type_name.clone(), d.syntax_kind.clone(), d.span, d.signature.clone()),
            None => {
                let module_type = packs.by_name(&ex.language).map(|p| p.spec.language.module_type.clone()).unwrap_or_default();
                (module_type, ex.root_kind.clone(), Span { start_line: 1, end_line: ex.lines, start_byte: 0, end_byte: ex.bytes }, None)
            }
        };
        // A placeholder counts for every definition containing it: code with a
        // nested unfinished part isn't finished either.
        let placeholders = match def {
            Some(_) => sites[entry.file].iter().filter(|p| span.contains(p)).copied().collect(),
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
                schemas: Default::default(),
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
    // Folders: modules nothing else contains sit in the folders their files
    // are in, so the repo reads as folders, crates, modules and members.
    let tops: Vec<(ComponentId, RepoPath)> = entries
        .iter()
        .filter(|e| e.is_module() && out.graph.parent(&e.id).is_none())
        .map(|e| (e.id.clone(), extracts[e.file].file.clone()))
        .collect();
    for (folder, parts) in folders(&tops) {
        out.graph.upsert_component(folder.component, Provenance::OBSERVED);
        for part in parts {
            link(&mut out.graph, RelationKind::Contains, &folder.id, &part);
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
    let mut implements_of: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
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
            if let Some(target) = find_type(&entries, &index, text, ex, &pack.spec.language).found()
                && target != source
            {
                link(&mut out.graph, RelationKind::Implements, &entries[source].id, &entries[target].id);
                let list = implements_of.entry(source).or_default();
                if !list.contains(&target) {
                    list.push(target);
                }
            }
        }
    }

    // Calls.
    let cx = Cx { entries: &entries, index: &index, extracts, packs, implements: &implements_of };
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
            if call.reference {
                // A function named as a value may be called later: a deferred
                // edge when the name is a known function, nothing otherwise.
                if let CallTarget::One(target) = cx.resolve_call(caller, &call.callee, Some(call), 0)
                    && entries[target].def.is_some()
                    && !entries[target].is_type
                {
                    let mut relation = Relation::new(RelationKind::Calls, caller_id, entries[target].id.clone(), Provenance::OBSERVED);
                    relation.sites.push(call.span);
                    relation.deferred = true;
                    out.graph.upsert_relation(relation);
                }
                continue;
            }
            match cx.resolve_call(caller, &call.callee, Some(call), 0) {
                CallTarget::One(target) => {
                    let mut relation = Relation::new(RelationKind::Calls, caller_id.clone(), entries[target].id.clone(), Provenance::OBSERVED);
                    let next = order.entry(caller_id).or_insert(0);
                    relation.order = Some(*next);
                    *next += 1;
                    relation.sites.push(call.span);
                    relation.deferred = call.deferred;
                    out.graph.upsert_relation(relation);
                    // Calling a type makes a value of it (`Report(...)`, `new Store()`).
                    if entries[target].holds_data {
                        let mut made = Relation::new(RelationKind::DataFlow, entries[caller].id.clone(), entries[target].id.clone(), Provenance::OBSERVED);
                        made.ports = Some(("makes".to_owned(), "value".to_owned()));
                        made.data = Some(entries[target].name.clone());
                        made.sites.push(call.span);
                        out.graph.upsert_relation(made);
                    }
                    // Outputs of other repo calls passed in as arguments.
                    for arg in &call.args {
                        let CallTarget::One(source) = cx.resolve_call(caller, &arg.producer, None, 0) else { continue };
                        match flow(&entries, extracts, source, target, arg.position, &pack.spec.language) {
                            Ok((ports, carried)) => {
                                let mut relation = Relation::new(RelationKind::DataFlow, entries[source].id.clone(), entries[target].id.clone(), Provenance::OBSERVED);
                                relation.ports = Some(ports);
                                relation.data = Some(carried);
                                relation.sites.push(call.span);
                                out.graph.upsert_relation(relation);
                            }
                            Err(reason) => out.unverified_flows.push(UnverifiedFlow {
                                source: entries[source].id.clone(),
                                target: entries[target].id.clone(),
                                position: arg.position,
                                file: ex.file.clone(),
                                span: call.span,
                                reason,
                            }),
                        }
                    }
                }
                target @ (CallTarget::Many(_) | CallTarget::Unproven(_)) => {
                    let (list, candidates) = match target {
                        CallTarget::Many(c) => (&mut out.ambiguous, c),
                        CallTarget::Unproven(c) => (&mut out.unproven, c),
                        _ => continue,
                    };
                    list.push(AmbiguousCall {
                        caller: caller_id,
                        callee: call.callee.clone(),
                        file: ex.file.clone(),
                        span: call.span,
                        candidates: candidates.iter().map(|&c| entries[c].id.clone()).collect(),
                    });
                }
                CallTarget::External => {
                    if seen_external.insert((caller_id.clone(), call.callee.clone())) {
                        out.external.push(ExternalCall { caller: caller_id, callee: call.callee.clone() });
                    }
                }
            }
        }
    }

    // Data flow beyond arguments: results returned or kept, values made,
    // the state of a definition's own type, and boundaries crossed.
    let mut senders: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    let mut receivers: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (fi, ex) in extracts.iter().enumerate() {
        let Some(pack) = packs.by_name(&ex.language) else { continue };
        let lang = &pack.spec.language;
        for raw in &ex.flows {
            let at = match raw.at {
                Some(di) => def_entry.get(&(fi, di)).copied(),
                None => Some(module_entry[&fi]),
            };
            let Some(at) = at else { continue };
            let mut add = |source: usize, target: usize, ports: (String, String), data: Option<String>| {
                if source == target {
                    return;
                }
                let mut relation = Relation::new(RelationKind::DataFlow, entries[source].id.clone(), entries[target].id.clone(), Provenance::OBSERVED);
                relation.ports = Some(ports);
                relation.data = data;
                relation.sites.push(raw.span);
                out.graph.upsert_relation(relation);
            };
            match raw.kind {
                FlowKind::Returned | FlowKind::Bound => {
                    let CallTarget::One(source) = cx.resolve_call(at, &raw.what, None, 0) else { continue };
                    if entries[source].is_type {
                        continue;
                    }
                    // What comes back, as written (`Result<Report, Error>`, `(u32, u32)`).
                    let carried = signature(&entries, extracts, source).and_then(|s| s.returns.as_deref()).map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
                    let into = if raw.kind == FlowKind::Returned { "returns".to_owned() } else { format!("kept as {}", raw.names.join(", ")) };
                    add(source, at, ("return".to_owned(), into), carried);
                }
                FlowKind::Constructs => {
                    if let Some(ty) = find_type(&entries, &index, &raw.what, ex, lang).found().filter(|t| entries[*t].holds_data) {
                        add(at, ty, ("makes".to_owned(), "value".to_owned()), Some(entries[ty].name.clone()));
                    }
                }
                FlowKind::Writes | FlowKind::Reads => {
                    let Some(owner) = entries[at].owner.filter(|o| entries[*o].holds_data) else { continue };
                    let data = Some(format!("{}.{}", entries[owner].name, raw.what));
                    if raw.kind == FlowKind::Writes {
                        add(at, owner, ("writes".to_owned(), raw.what.clone()), data);
                    } else {
                        add(owner, at, (raw.what.clone(), "reads".to_owned()), data);
                    }
                }
                FlowKind::Sends => {
                    if let Some(boundary) = &raw.boundary {
                        senders.entry((boundary.clone(), raw.what.clone())).or_default().push(at);
                    }
                }
                FlowKind::Receives => {
                    let Some(boundary) = &raw.boundary else { continue };
                    let list = receivers.entry((boundary.clone(), raw.what.clone())).or_default();
                    if raw.names.is_empty() {
                        list.push(at);
                    }
                    // Named handlers (route handlers) receive, wherever they're registered.
                    for handler in &raw.names {
                        if let CallTarget::One(h) = cx.resolve_call(at, handler, None, 0) {
                            list.push(h);
                        }
                    }
                }
            }
        }
    }
    for (key, from) in &senders {
        for &to in receivers.get(key).into_iter().flatten() {
            for &source in from {
                if source != to {
                    let mut relation = Relation::new(RelationKind::DataFlow, entries[source].id.clone(), entries[to].id.clone(), Provenance::OBSERVED);
                    relation.ports = Some((key.0.clone(), key.1.clone()));
                    relation.data = Some(key.1.clone());
                    out.graph.upsert_relation(relation);
                }
            }
        }
    }
    out
}

fn signature<'a>(entries: &[Entry], extracts: &'a [FileExtract], entry: usize) -> Option<&'a arch_graph::Signature> {
    entries[entry].def.and_then(|(fi, di)| extracts[fi].definitions[di].signature.as_ref())
}

/// Ports for `source`'s output flowing into `target`'s argument `position`,
/// when both types are written and carry the same data.
fn flow(entries: &[Entry], extracts: &[FileExtract], source: usize, target: usize, position: u32, lang: &LanguageSpec) -> Result<((String, String), String), String> {
    // A type by its last path segment (`store.Store`, `crate::x::Store` -> Store).
    let base = |t: &str| split(t, lang).pop().unwrap_or_default();
    let produced = signature(entries, extracts, source).and_then(|s| s.returns.as_deref()).map(|t| {
        // `Self` (and the like) means the producer's own type.
        let t = base(t);
        match self_type(entries, source) {
            Some(own) if lang.self_names.contains(&t) => entries[own].name.clone(),
            _ => t,
        }
    });
    let Some(produced) = produced else { return Err(format!("{} declares no return type", entries[source].symbol)) };
    let params: Vec<&arch_graph::Param> = signature(entries, extracts, target)
        .map(|s| s.params.iter().filter(|p| !lang.words(&p.name).iter().any(|w| lang.self_names.iter().any(|n| n == w))).collect())
        .unwrap_or_default();
    let Some(param) = params.get(position as usize) else { return Err(format!("{} has no parameter at position {position}", entries[target].symbol)) };
    let Some(wanted) = param.ty.as_deref().map(base) else { return Err(format!("parameter '{}' of {} has no type", param.name, entries[target].symbol)) };
    let compatible = produced == wanted || lang.compatible_types.iter().any(|group| group.contains(&produced) && group.contains(&wanted));
    if compatible { Ok((("return".to_owned(), param.name.clone()), produced)) } else { Err(format!("{produced} doesn't match parameter '{}': {wanted}", param.name)) }
}

/// What a written type name refers to, seen from a file.
enum TypeRef {
    /// A type in the repo.
    Found(usize),
    /// Not a repo type: no repo type has the name, the qualified path leads
    /// elsewhere, or the file imports the name from somewhere else.
    Outside,
    /// Repo types share the name, but nothing proves which (not local, not
    /// imported).
    Unknown,
}

impl TypeRef {
    fn found(self) -> Option<usize> {
        match self {
            TypeRef::Found(t) => Some(t),
            _ => None,
        }
    }
}

/// Finds a type component (one that can own members) by its written name,
/// among the components of the same language, by the same evidence as calls:
/// a qualified path, the file's own module, or its imports (aliases first).
fn find_type(entries: &[Entry], index: &Index, text: &str, ex: &FileExtract, lang: &LanguageSpec) -> TypeRef {
    let text = de_alias(text, &ex.aliases, lang);
    let raw = split(&text, lang);
    let anchored = raw.first().is_some_and(|w| lang.relative_prefixes.contains(w));
    let parts: Vec<String> = raw.into_iter().filter(|p| !lang.relative_prefixes.contains(p)).collect();
    let Some(name) = parts.last() else { return TypeRef::Outside };
    let candidates: Vec<usize> = index.named(&lang.name, name).iter().copied().filter(|&i| entries[i].is_type).collect();
    if candidates.is_empty() {
        return TypeRef::Outside;
    }
    if parts.len() >= 2 {
        return match path_matches(entries, &candidates, &parts, lang).as_slice() {
            [] => TypeRef::Outside,
            // `a::T` proves nothing unless `a` itself is in scope.
            [one] if anchored || head_in_scope(entries, index, ex, lang, &parts[0]) => TypeRef::Found(*one),
            _ => TypeRef::Unknown,
        };
    }
    let local: Vec<usize> = candidates.iter().copied().filter(|&c| entries[c].module == ex.module).collect();
    if let [one] = local.as_slice() {
        return TypeRef::Found(*one);
    }
    let imported: Vec<usize> = candidates.iter().copied().filter(|&c| imports_from(&ex.imports, lang, &ex.module, name, &entries[c].module)).collect();
    match imported.as_slice() {
        [one] => TypeRef::Found(*one),
        [] if names_import(&ex.imports, lang, name) => TypeRef::Outside,
        _ => TypeRef::Unknown,
    }
}

/// Whether the first segment of a qualified path means something in `ex`:
/// a local definition, a submodule of its module, the package itself, or an
/// imported name.
fn head_in_scope(entries: &[Entry], index: &Index, ex: &FileExtract, lang: &LanguageSpec, head: &str) -> bool {
    let submodule = format!("{}{}{head}", ex.module, lang.separator);
    names_import(&ex.imports, lang, head)
        || ex.aliases.iter().any(|(alias, _)| alias == head)
        || lang.words(&ex.module).first() == Some(&head)
        || index.named(&lang.name, head).iter().any(|&i| entries[i].module == ex.module)
        || index.modules.contains(&(lang.name.clone(), submodule))
}

/// Whether one of `imports` brings `name` in from `module` (the path names it,
/// whole, or a package above it re-exporting; by name or with a glob). A
/// path of only relative markers means the importer's module or one above.
fn imports_from(imports: &[String], lang: &LanguageSpec, importer: &str, name: &str, module: &str) -> bool {
    let importer_words = lang.words(importer);
    let module = lang.words(module);
    imports.iter().any(|imp| {
        let glob = lang.glob_import.as_deref().is_some_and(|g| imp.contains(g));
        let words: Vec<&str> = lang.words(imp).into_iter().filter(|w| !lang.import_keywords.iter().any(|k| k == w)).collect();
        let marked = lang.relative_import_markers.iter().any(|m| imp.contains(m.as_str()));
        let path_ok = |raw: &[&str]| {
            let relative = marked || raw.first().is_some_and(|w| lang.relative_prefixes.iter().any(|r| r == w));
            let path: Vec<&str> = raw.iter().copied().filter(|w| !lang.relative_prefixes.iter().any(|r| r == w)).collect();
            // The module as the path below one of the importer's ancestors (or
            // itself): `use store::X` from the crate root, `from .helpers ...`.
            let below_ancestor = module.ends_with(&path) && importer_words.starts_with(&module[..module.len() - path.len()]);
            if path.is_empty() {
                !raw.is_empty() && importer_words.starts_with(&module)
            } else if relative {
                below_ancestor
            } else {
                // Or from the root: the module itself or a package above it.
                below_ancestor || module.starts_with(&path)
            }
        };
        (0..=words.len()).any(|i| {
            let (head, tail) = words.split_at(i);
            // `use a::b::name` (path first) or `import { name } from "a/b"` (names first).
            (path_ok(head) && (glob || tail.contains(&name))) || (path_ok(tail) && (glob || head.contains(&name)))
        })
    })
}

/// Whether an import names `name` explicitly (from wherever).
fn names_import(imports: &[String], lang: &LanguageSpec, name: &str) -> bool {
    imports.iter().any(|imp| lang.words(imp).contains(&name))
}

/// Candidates whose symbol ends with the written qualified path.
fn path_matches(entries: &[Entry], candidates: &[usize], parts: &[String], lang: &LanguageSpec) -> Vec<usize> {
    if parts.len() < 2 {
        return Vec::new();
    }
    let wanted: Vec<&str> = parts.iter().map(String::as_str).collect();
    candidates.iter().copied().filter(|&c| lang.words(&entries[c].symbol).ends_with(&wanted)).collect()
}

enum CallTarget {
    One(usize),
    /// Proven to be one of these (e.g. the same name imported twice).
    Many(Vec<usize>),
    /// Not provable: the receiver's type is unknown, or the name isn't
    /// local or imported. These share the name.
    Unproven(Vec<usize>),
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

/// What a receiver's type is known to be.
enum Receiver {
    /// A type in the repo.
    Typed(usize),
    /// The bases of a type in the repo (`super`).
    Base(usize),
    /// A type outside the repo (a library type, a primitive).
    Outside,
    Unknown,
}

/// Recursion bound for receivers that are themselves calls.
const MAX_DEPTH: u32 = 4;

struct Cx<'a> {
    entries: &'a [Entry],
    index: &'a Index,
    extracts: &'a [FileExtract],
    packs: &'a PackSet,
    /// Entry -> the interfaces its scopes say it implements.
    implements: &'a BTreeMap<usize, Vec<usize>>,
}

impl Cx<'_> {
    fn lang(&self, entry: usize) -> Option<&LanguageSpec> {
        self.packs.by_name(&self.entries[entry].language).map(|p| &p.spec.language)
    }

    /// The type a written type text names, seen from `context`.
    fn written_type(&self, context: usize, text: &str) -> Receiver {
        let Some(lang) = self.lang(context) else { return Receiver::Unknown };
        let Some(base) = split(text, lang).pop() else { return Receiver::Unknown };
        if lang.self_names.contains(&base) {
            return self_type(self.entries, context).map_or(Receiver::Unknown, Receiver::Typed);
        }
        match find_type(self.entries, self.index, text, &self.extracts[self.entries[context].file], lang) {
            TypeRef::Found(t) => Receiver::Typed(t),
            TypeRef::Unknown => Receiver::Unknown,
            TypeRef::Outside => Receiver::Outside,
        }
    }

    /// The type of what calling `target` gives back.
    fn output_type(&self, target: usize) -> Receiver {
        if self.entries[target].is_type {
            return Receiver::Typed(target); // a constructor call
        }
        match signature(self.entries, self.extracts, target).and_then(|s| s.returns.as_deref()) {
            Some(returns) => self.written_type(target, returns),
            None => Receiver::Unknown,
        }
    }

    fn call_output(&self, caller: usize, callee: &str, depth: u32) -> Receiver {
        if depth >= MAX_DEPTH {
            return Receiver::Unknown;
        }
        match self.resolve_call(caller, callee, None, depth + 1) {
            CallTarget::One(t) => self.output_type(t),
            CallTarget::External => Receiver::Outside,
            CallTarget::Many(_) | CallTarget::Unproven(_) => Receiver::Unknown,
        }
    }

    /// What the receiver `qualifier` of a call made in `caller` is.
    fn receiver(&self, caller: usize, qualifier: &str, call: Option<&RawCall>, depth: u32) -> Receiver {
        let Some(lang) = self.lang(caller) else { return Receiver::Unknown };
        let q = qualifier.trim();
        // super: the enclosing type's bases.
        if lang.super_receivers.iter().any(|s| s == q) {
            return self_type(self.entries, caller).map_or(Receiver::Unknown, Receiver::Base);
        }
        // self / this / the method's named receiver.
        if lang.self_names.iter().any(|s| s == q) || self.entries[caller].self_name.as_deref() == Some(q) {
            return self_type(self.entries, caller).map_or(Receiver::Unknown, Receiver::Typed);
        }
        // A call's result: `Greeter().greet`, `new Greeter().greet`.
        if let Some(inner) = q.strip_suffix(')').and_then(|rest| {
            let mut depth = 1;
            rest.char_indices().rev().find_map(|(i, ch)| {
                match ch {
                    ')' => depth += 1,
                    '(' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then(|| &rest[..i])
            })
        }) {
            return self.call_output(caller, inner, depth);
        }
        // A local: its declared type, else what it was bound to.
        if let Some(call) = call {
            if let Some(ty) = &call.receiver_type {
                return self.written_type(caller, ty);
            }
            if let Some(producer) = &call.receiver_producer {
                return self.call_output(caller, producer, depth);
            }
        }
        // A parameter with a written type.
        let name = lang.strip_references(q);
        if let Some(param) = signature(self.entries, self.extracts, caller).and_then(|s| s.params.iter().find(|p| lang.strip_references(&p.name) == name)) {
            return match &param.ty {
                Some(ty) => self.written_type(caller, ty),
                None => Receiver::Unknown,
            };
        }
        // A type named directly (associated calls on a type value).
        match find_type(self.entries, self.index, q, &self.extracts[self.entries[caller].file], lang) {
            TypeRef::Found(t) => Receiver::Typed(t),
            TypeRef::Outside | TypeRef::Unknown => Receiver::Unknown,
        }
    }

    /// What `ty` extends or implements, transitively: declared bases and
    /// interfaces from scopes (`impl Trait for Type`), nearest first.
    fn bases(&self, ty: usize) -> Vec<usize> {
        let mut types = vec![ty];
        let mut i = 0;
        while i < types.len() && i < 32 {
            let t = types[i];
            let mut next: Vec<usize> = self.implements.get(&t).cloned().unwrap_or_default();
            if let Some((fi, di)) = self.entries[t].def
                && let Some(lang) = self.lang(t)
            {
                next.extend(self.extracts[fi].definitions[di].bases.iter().filter_map(|base| find_type(self.entries, self.index, base, &self.extracts[fi], lang).found()));
            }
            for b in next {
                if !types.contains(&b) {
                    types.push(b);
                }
            }
            i += 1;
        }
        types.remove(0);
        types
    }

    /// Members named `name` of `ty` or what it extends or implements (a
    /// trait's default method counts).
    fn members(&self, ty: usize, name: &str) -> Vec<usize> {
        let mut types = vec![ty];
        types.extend(self.bases(ty));
        self.members_of(&types, name)
    }

    /// The nearest declaration among `types` (an override hides the base's).
    fn members_of(&self, types: &[usize], name: &str) -> Vec<usize> {
        types
            .iter()
            .find_map(|&t| {
                let found: Vec<usize> = self.index.members(t, name).to_vec();
                (!found.is_empty()).then_some(found)
            })
            .unwrap_or_default()
    }

    fn resolve_call(&self, caller: usize, callee: &str, call: Option<&RawCall>, depth: u32) -> CallTarget {
        let Some(lang) = self.lang(caller) else { return CallTarget::External };
        let ex = &self.extracts[self.entries[caller].file];
        let callee = de_alias(strip_constructor(callee, lang), &ex.aliases, lang);
        let is_member = |c: usize| self.entries[c].owner.is_some_and(|o| self.entries[o].is_type);
        let Some((qualifier, sep, name)) = lang.split_last(&callee) else {
            return self.bare(caller, &clean(&callee, lang), &ex.imports, lang);
        };
        let name = clean(name, lang);
        let candidates: Vec<usize> = self.index.named(&lang.name, &name).to_vec();
        let mut receiver = Receiver::Unknown;
        if lang.is_member_separator(sep) {
            receiver = self.receiver(caller, qualifier, call, depth);
            match receiver {
                Receiver::Typed(t) => return narrow(self.members(t, &name)).unwrap_or(CallTarget::External),
                Receiver::Base(t) => {
                    let bases = self.bases(t);
                    return narrow(self.members_of(&bases, &name)).unwrap_or(CallTarget::External);
                }
                Receiver::Outside => return CallTarget::External,
                Receiver::Unknown => {}
            }
        }
        if lang.is_path_separator(sep) {
            let raw = split(&callee, lang);
            let anchored = raw.first().is_some_and(|w| lang.relative_prefixes.contains(w));
            let path: Vec<String> = raw.into_iter().filter(|p| !lang.relative_prefixes.contains(p)).collect();
            let matches = path_matches(self.entries, &candidates, &path, lang);
            if !matches.is_empty() {
                // `a::f` only proves `f` when `a` itself is in scope here.
                return match anchored || path.first().is_some_and(|head| head_in_scope(self.entries, self.index, ex, lang, head)) {
                    true => narrow(matches).unwrap_or(CallTarget::External),
                    false => CallTarget::Unproven(matches),
                };
            }
        }
        // A value of unknown type: any member with the name could be meant.
        let members: Vec<usize> = candidates.into_iter().filter(|&c| is_member(c)).collect();
        match receiver {
            Receiver::Unknown if lang.is_member_separator(sep) && !members.is_empty() => CallTarget::Unproven(members),
            _ => CallTarget::External,
        }
    }

    /// A bare name reaches free functions or types (constructors) in its own
    /// module, or imported ones; anything else is unproven.
    fn bare(&self, caller: usize, name: &str, imports: &[String], lang: &LanguageSpec) -> CallTarget {
        let is_member = |c: usize| self.entries[c].owner.is_some_and(|o| self.entries[o].is_type);
        let free: Vec<usize> = self.index.named(&lang.name, name).iter().copied().filter(|&i| !is_member(i)).collect();
        if free.is_empty() {
            return CallTarget::External;
        }
        let caller_module = self.entries[caller].module.as_str();
        if let Some(target) = narrow(free.iter().copied().filter(|&c| self.entries[c].module == caller_module).collect()) {
            return target;
        }
        let imported: Vec<usize> = free.iter().copied().filter(|&c| imports_from(imports, lang, caller_module, name, &self.entries[c].module)).collect();
        narrow(imported).unwrap_or(CallTarget::Unproven(free))
    }
}
