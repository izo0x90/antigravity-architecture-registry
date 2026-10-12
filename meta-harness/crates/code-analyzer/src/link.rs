//! Linking, a file at a time. The linker
//! keeps a symbol table of every file's components. Linking a set of files
//! takes their entries out, makes them again from the files' readings, and
//! links what they use: definitions get their symbols and owners, then
//! implements, calls and data flow. Calls resolve by evidence (self and
//! typed receivers, qualified paths, same module, imports and aliases),
//! within one language. A call proven to mean one of several components is
//! ambiguous; one that can't be proven (unknown receiver type, a name
//! neither local nor imported) is unproven; one to nothing in the repo is
//! external. Nothing is guessed. Every ref keeps what it was linked to, or
//! why not.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use arch_graph::{ComponentId, RelationKind, RepoPath, Signature, Span, TypeSystem};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::pack::LanguageSpec;
use crate::rows::{Access, ComponentDetail, ComponentRow, DefPlace, Evidence, FileRow, FileRows, FlowRow, ModuleReading, RefDetail, RefKind, RefRow, RelationDetail, RelationRow, call_name, flow_refs, read_rows};
use crate::{Expr, FileExtract, FlowKind, PackSet, RawCall};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmbiguousCall {
    pub caller: ComponentId,
    pub callee: String,
    pub file: RepoPath,
    pub span: Span,
    pub candidates: Vec<ComponentId>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ExternalCall {
    pub caller: ComponentId,
    pub callee: String,
}

/// Data handed from one call into another that types couldn't confirm.
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

/// What a language's server said a name in a file is (a call, a type
/// named, a field used): the component (None: code outside the repo), who
/// said it, and whether it still holds (a link into a file that changed
/// since is kept, unconfirmed, until asked again).
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub target: Option<ComponentId>,
    /// With no target: the server put it outside the repo,
    pub outside: bool,
    /// or in the repo at a place that's no component (else: no definition).
    pub local: bool,
    pub prover: String,
    pub confirmed: bool,
    /// A field: whether the place reads or writes it, as the server says.
    pub access: Option<Access>,
}

/// What the servers said about a file's names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Saids {
    /// Calls, by the call's place among the file's calls.
    pub calls: BTreeMap<usize, Said>,
    /// Types named, fields used, imports and paths, by where the name starts.
    pub at: BTreeMap<u32, Said>,
}

impl Saids {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.at.is_empty()
    }

    /// Every answer, to mark them (say, no longer confirmed).
    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Said> {
        self.calls.values_mut().chain(self.at.values_mut())
    }
}

/// `row` as the server said it is, when it said anything: its target (the
/// server's wins) and how it's known.
fn as_said(row: &mut RefRow, said: Option<&Said>) {
    let Some(s) = said else { return };
    row.prover = Some(s.prover.clone());
    row.evidence = match (&s.target, s.confirmed) {
        (Some(_), true) => Evidence::Server,
        (None, true) if s.outside => Evidence::External,
        (None, true) if s.local => Evidence::Local,
        (None, true) => Evidence::NotFound,
        // Kept as it was until asked again.
        (_, false) => Evidence::Name,
    };
    if s.confirmed {
        row.target = s.target.clone();
    }
}

/// One component as linking sees it.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    id: ComponentId,
    symbol: String,
    name: String,
    module: String,
    language: String,
    file: RepoPath,
    /// Its index among its file's definitions; None for a module.
    def: Option<usize>,
    /// The component it's declared in: its owner type for scoped
    /// definitions, else the definition around it. None at module level.
    owner: Option<ComponentId>,
    /// Can own members that `self` refers to (a non-executable, non-module type).
    is_type: bool,
    /// Runs (a function, a method): what's declared inside it is its own.
    executable: bool,
    /// A type whose values carry data (it has properties): made, and holds state.
    holds_data: bool,
    /// What the owner value is called inside it, when the language names it
    /// per definition (a Go receiver).
    self_name: Option<String>,
    signature: Option<Signature>,
    /// Bases written in its own definition, as written (resolved in its file).
    bases: Vec<String>,
    /// The types its signature and locals name: (name, whether which type it
    /// is was settled: the server confirmed it, or it's no repo type).
    named_types: Vec<(String, bool)>,
}

impl Entry {
    fn is_module(&self) -> bool {
        self.def.is_none()
    }

    /// Whether every repo type `written` names (a return or parameter type
    /// as written) is the one the server says it is.
    fn types_confirmed(&self, written: &str) -> bool {
        let words: Vec<&str> = written.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
        self.named_types.iter().filter(|(name, _)| words.contains(&name.as_str())).all(|(_, settled)| *settled)
    }
}

/// The types a component's refs name, and whether each is settled.
fn named_types<'a>(refs: impl Iterator<Item = &'a RefRow>, within: &ComponentId) -> Vec<(String, bool)> {
    refs.filter(|r| r.kind == RefKind::Type && r.within.as_ref() == Some(within)).map(|r| (r.name.clone(), r.evidence == Evidence::Server || r.target.is_none())).collect()
}

/// What a file brings into scope.
#[derive(Clone, Debug, Default)]
struct FileScope {
    module: String,
    imports: Imports,
    aliases: Vec<(String, String)>,
    /// Named items it has that aren't components (constants, type aliases).
    items: Vec<String>,
    /// Its entries (the module first).
    entries: Vec<usize>,
}

/// A file's imports, with every name they write (so most lookups are
/// answered without reading them).
#[derive(Clone, Debug, Default)]
struct Imports {
    texts: Vec<String>,
    words: HashSet<String>,
    glob: bool,
}

impl Imports {
    fn new(texts: Vec<String>, lang: Option<&LanguageSpec>) -> Self {
        let words = texts.iter().flat_map(|t| t.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).map(str::to_owned)).collect();
        let glob = lang.and_then(|l| l.glob_import.as_deref()).is_some_and(|g| texts.iter().any(|t| t.contains(g)));
        Self { texts, words, glob }
    }

    /// Whether one of them could bring `name` in (it's written, or a glob is).
    fn could_bring(&self, name: &str) -> bool {
        self.glob || self.words.contains(name)
    }
}

/// Every file's components, with lookups kept as they change.
#[derive(Default)]
pub(crate) struct Linker {
    entries: Vec<Option<Entry>>,
    free: Vec<usize>,
    /// Each id's entry (the first of several with one id).
    by_id: HashMap<ComponentId, usize>,
    /// Every entry with each id (one module in two files, a definition
    /// under two `cfg`s).
    same_id: HashMap<ComponentId, Vec<usize>>,
    /// language -> name -> the definitions with that name.
    named: HashMap<String, HashMap<String, Vec<usize>>>,
    /// owner -> name -> the members of `owner` with that name.
    members: HashMap<ComponentId, HashMap<String, Vec<usize>>>,
    /// (language, symbol) of every module, and how many files make it.
    modules: HashMap<(String, String), usize>,
    /// language -> last name -> the modules' entries with that name.
    module_named: HashMap<String, HashMap<String, Vec<usize>>>,
    files: HashMap<RepoPath, FileScope>,
    /// Type -> what it implements, by the file that says so (scopes in one
    /// file can say it of a type in another).
    implements: HashMap<ComponentId, Vec<(RepoPath, ComponentId)>>,
}

impl std::ops::Index<usize> for Linker {
    type Output = Entry;
    fn index(&self, i: usize) -> &Entry {
        self.entries[i].as_ref().expect("a live entry")
    }
}

impl Linker {
    /// The symbol table from stored rows (nothing is linked).
    pub fn load<'a>(packs: &PackSet, types: &TypeSystem, rows: impl Iterator<Item = &'a FileRows>) -> Self {
        let mut linker = Self::default();
        for file in rows {
            let module = file.module().map(|m| m.detail.symbol.clone()).unwrap_or_default();
            let reading = file.module().and_then(|m| m.detail.module.clone()).unwrap_or_default();
            let lang = packs.by_name(&file.file.language).map(|p| &p.spec.language);
            let mut scope = FileScope { module: module.clone(), imports: Imports::new(reading.imports.into_iter().map(|i| i.text).collect(), lang), aliases: reading.aliases, items: reading.items, entries: Vec::new() };
            let module_id = file.module().map(|m| m.id.clone());
            for c in &file.components {
                let owner = c.parent.clone().filter(|p| Some(p) != module_id.as_ref());
                let mut entry = linker.entry_for(packs, types, &file.file, &module, c, owner);
                entry.named_types = named_types(file.refs.iter(), &c.id);
                scope.entries.push(linker.add(entry));
            }
            for r in file.relations.iter().filter(|r| r.kind == RelationKind::Implements) {
                linker.implements.entry(r.source.clone()).or_default().push((file.file.path.clone(), r.target.clone()));
            }
            linker.files.insert(file.file.path.clone(), scope);
        }
        linker
    }

    fn entry_for(&self, packs: &PackSet, types: &TypeSystem, file: &FileRow, module: &str, c: &ComponentRow, owner: Option<ComponentId>) -> Entry {
        let module_types: BTreeSet<&str> = packs.packs.iter().map(|p| p.spec.language.module_type.as_str()).collect();
        let resolved = types.resolve(&c.type_name).ok();
        let resolved = resolved.as_ref();
        Entry {
            id: c.id.clone(),
            symbol: c.detail.symbol.clone(),
            name: c.name.clone(),
            module: module.to_owned(),
            language: file.language.clone(),
            file: file.path.clone(),
            def: c.detail.def.as_ref().map(|d| d.index),
            owner,
            is_type: c.detail.def.is_some() && resolved.is_some_and(|t| !t.executable && !module_types.iter().any(|m| t.is_a(m))),
            executable: resolved.is_some_and(|t| t.executable),
            holds_data: resolved.is_some_and(|t| t.allows_properties),
            self_name: c.detail.def.as_ref().and_then(|d| d.self_name.clone()),
            signature: c.signature.clone(),
            bases: c.detail.def.as_ref().map(|d| d.bases.iter().map(|b| b.text.clone()).collect()).unwrap_or_default(),
            named_types: Vec::new(),
        }
    }

    fn add(&mut self, entry: Entry) -> usize {
        let i = match self.free.pop() {
            Some(i) => i,
            None => {
                self.entries.push(None);
                self.entries.len() - 1
            }
        };
        if entry.is_module() {
            *self.modules.entry((entry.language.clone(), entry.symbol.clone())).or_default() += 1;
            self.module_named.entry(entry.language.clone()).or_default().entry(entry.name.clone()).or_default().push(i);
        } else {
            self.named.entry(entry.language.clone()).or_default().entry(entry.name.clone()).or_default().push(i);
            if let Some(owner) = &entry.owner {
                self.members.entry(owner.clone()).or_default().entry(entry.name.clone()).or_default().push(i);
            }
        }
        // Two files can make one module (a crate's lib.rs and main.rs): the
        // first keeps the id.
        self.by_id.entry(entry.id.clone()).or_insert(i);
        self.same_id.entry(entry.id.clone()).or_default().push(i);
        self.entries[i] = Some(entry);
        i
    }

    fn remove(&mut self, i: usize) {
        let Some(entry) = self.entries[i].take() else { return };
        if entry.is_module() {
            let key = (entry.language.clone(), entry.symbol.clone());
            if let Some(n) = self.modules.get_mut(&key) {
                *n -= 1;
                if *n == 0 {
                    self.modules.remove(&key);
                }
            }
            if let Some(names) = self.module_named.get_mut(&entry.language)
                && let Some(list) = names.get_mut(&entry.name)
            {
                list.retain(|&e| e != i);
                if list.is_empty() {
                    names.remove(&entry.name);
                }
            }
        } else {
            if let Some(names) = self.named.get_mut(&entry.language)
                && let Some(list) = names.get_mut(&entry.name)
            {
                list.retain(|&e| e != i);
                if list.is_empty() {
                    names.remove(&entry.name);
                }
            }
            if let Some(owner) = &entry.owner
                && let Some(names) = self.members.get_mut(owner)
            {
                if let Some(list) = names.get_mut(&entry.name) {
                    list.retain(|&e| e != i);
                    if list.is_empty() {
                        names.remove(&entry.name);
                    }
                }
                if names.is_empty() {
                    self.members.remove(owner);
                }
            }
        }
        let others = self.same_id.get_mut(&entry.id).map(|list| {
            list.retain(|&e| e != i);
            list.first().copied()
        });
        match others.flatten() {
            // Another entry with the same id keeps it.
            Some(other) => {
                self.by_id.insert(entry.id.clone(), other);
            }
            None => {
                self.by_id.remove(&entry.id);
                self.same_id.remove(&entry.id);
            }
        }
        self.free.push(i);
    }

    /// Drops a file's entries and what it said.
    pub fn forget(&mut self, file: &RepoPath) {
        if let Some(scope) = self.files.remove(file) {
            for i in scope.entries {
                self.remove(i);
            }
        }
        for list in self.implements.values_mut() {
            list.retain(|(from, _)| from != file);
        }
        self.implements.retain(|_, list| !list.is_empty());
    }

    fn named(&self, language: &str, name: &str) -> &[usize] {
        self.named.get(language).and_then(|n| n.get(name)).map_or(&[], Vec::as_slice)
    }

    fn members_named(&self, owner: &ComponentId, name: &str) -> &[usize] {
        self.members.get(owner).and_then(|n| n.get(name)).map_or(&[], Vec::as_slice)
    }

    fn owner(&self, i: usize) -> Option<usize> {
        self[i].owner.as_ref().and_then(|o| self.by_id.get(o).copied())
    }

    /// Whether a callee says it's in the repo: a path whose first name
    /// resolves into it (`crate::util::f`, `util::f` with `util` a
    /// submodule, `Store::new` with `Store` a repo type in scope), or a name
    /// an alias or import brings in from a repo path.
    fn leads_into_repo(&self, scope: &FileScope, lang: &LanguageSpec, callee: &Expr) -> bool {
        let written: Vec<&str> = match callee {
            Expr::Path(parts, _) => parts.iter().map(String::as_str).collect(),
            Expr::Name(name) => vec![name.as_str()],
            _ => return false,
        };
        match written.as_slice() {
            [] => false,
            [name] => self.brought_from_repo(scope, lang, name),
            [first, ..] => self.first_in_repo(scope, lang, first),
        }
    }

    /// Whether `word`, first in a written path, names something of the
    /// repo from this file.
    fn first_in_repo(&self, scope: &FileScope, lang: &LanguageSpec, word: &str) -> bool {
        lang.relative_prefixes.iter().any(|r| r == word)
            || lang.words(&scope.module).first() == Some(&word)
            || self.modules.contains_key(&(lang.name.clone(), format!("{}{}{word}", scope.module, lang.separator)))
            || find_type(self, word, scope, lang).found_ref().is_some()
            || self.brought_from_repo(scope, lang, word)
    }

    /// What a written path (its words so far, relative prefixes and all)
    /// names: a repo module ending with it, else a repo type; else why not
    /// (missing if it's written as the repo's, else outside).
    fn path_target(&self, scope: &FileScope, lang: &LanguageSpec, written: &[&str]) -> Result<ComponentId, Evidence> {
        let anchored = written.first().is_some_and(|w| lang.relative_prefixes.iter().any(|r| r == w));
        let path: Vec<&str> = written.iter().copied().filter(|w| !lang.relative_prefixes.iter().any(|r| r == w)).collect();
        let path = path.as_slice();
        let Some(last) = path.last() else { return Err(Evidence::External) };
        let modules: Vec<usize> = self.module_named.get(&lang.name).and_then(|n| n.get(*last)).map_or(&[][..], Vec::as_slice).iter().copied().filter(|&m| lang.words(&self[m].symbol).ends_with(path)).collect();
        let mut ids: Vec<&ComponentId> = modules.iter().map(|&m| &self[m].id).collect();
        ids.sort();
        ids.dedup();
        if let [one] = ids.as_slice() {
            return Ok((*one).clone());
        }
        if let TypeRef::Found(t) = find_type(self, &written.join(&lang.separator), scope, lang) {
            return Ok(self[t].id.clone());
        }
        match ids.len() {
            0 if anchored || self.first_in_repo(scope, lang, path[0]) => Err(Evidence::NotFound),
            0 => Err(Evidence::External),
            _ => Err(Evidence::Ambiguous),
        }
    }

    /// The modules a written path names (relative prefixes and all): from
    /// here (`crate::`, `super::`, `self::`), exactly that module; else the
    /// modules ending with its longest leading part that is one (a brace
    /// list's names come after it), or the package root when it's empty.
    fn written_modules(&self, lang: &LanguageSpec, scope: &FileScope, path: &[&str]) -> Vec<usize> {
        let here = lang.words(&scope.module);
        let package = here.first().copied().unwrap_or_default();
        let named_modules = |words: &[&str], whole: bool| -> Vec<usize> {
            let Some(last) = words.last() else { return Vec::new() };
            let modules = self.module_named.get(&lang.name).and_then(|n| n.get(*last)).map_or(&[][..], Vec::as_slice);
            modules.iter().copied().filter(|&m| if whole { lang.words(&self[m].symbol) == words } else { lang.words(&self[m].symbol).ends_with(words) }).collect()
        };
        let has = |list: &[String], w: &str| list.iter().any(|p| p == w);
        let mut from: Option<Vec<&str>> = None;
        let mut rest = path;
        while let Some((first, after)) = rest.split_first() {
            if has(&lang.root_prefixes, first) {
                from = Some(vec![package]);
            } else if has(&lang.parent_prefixes, first) {
                from.get_or_insert_with(|| here.clone()).pop();
            } else if has(&lang.here_prefixes, first) {
                from.get_or_insert_with(|| here.clone());
            } else if !has(&lang.relative_prefixes, first) {
                break;
            }
            rest = after;
        }
        match from {
            Some(base) => (0..=rest.len())
                .rev()
                .map(|n| {
                    let mut words = base.clone();
                    words.extend_from_slice(&rest[..n]);
                    named_modules(&words, true)
                })
                .find(|m| !m.is_empty())
                .unwrap_or_default(),
            None => (1..=rest.len()).rev().map(|n| named_modules(&rest[..n], false)).find(|m| !m.is_empty()).unwrap_or_else(|| named_modules(&[package], false)),
        }
    }

    /// The definitions named `name` that the module `path` names (as
    /// written, see `written_modules`) brings in with its own imports.
    fn reexported(&self, lang: &LanguageSpec, scope: &FileScope, path: &[&str], name: &str) -> Vec<usize> {
        let modules = self.written_modules(lang, scope, path);
        let mut out = Vec::new();
        for m in modules {
            let Some(there) = self.files.get(&self[m].file) else { continue };
            out.extend(self.named(&lang.name, name).iter().copied().filter(|&c| self[c].owner.is_none() && imports_from(&there.imports, lang, &there.module, name, &self[c].module)));
        }
        out
    }

    /// Whether a callee is written as a member of a repo type (`T::name`,
    /// `module::T::name`).
    fn of_repo_type(&self, scope: &FileScope, lang: &LanguageSpec, callee: &Expr) -> bool {
        let Expr::Path(parts, _) = callee else { return false };
        parts.len() >= 2 && matches!(find_type(self, &parts[..parts.len() - 1].join(&lang.separator), scope, lang), TypeRef::Found(_))
    }

    /// Whether a callee is written as a member of the enclosing repo type
    /// (`Self::name` inside its impl).
    fn of_self_type(&self, caller: usize, lang: &LanguageSpec, callee: &Expr) -> bool {
        matches!(callee, Expr::Path(parts, _) if parts.len() == 2 && lang.self_type_names.contains(&parts[0])) && self_type(self, caller).is_some()
    }

    /// Whether `path` names a repo module (the module's symbol ends with it).
    fn names_module(&self, lang: &LanguageSpec, path: &[&str]) -> bool {
        let Some(last) = path.last() else { return false };
        self.module_named.get(&lang.name).and_then(|n| n.get(*last)).is_some_and(|ms| ms.iter().any(|&m| lang.words(&self[m].symbol).ends_with(path)))
    }

    /// What `name` is when one of a file's imports brings it from a repo
    /// module that doesn't define it but re-exports it (`use crate::X`, the
    /// crate root having `pub use component::X`): the definitions that
    /// module brings in, one step.
    fn through_reexport(&self, scope: &FileScope, lang: &LanguageSpec, name: &str) -> Vec<usize> {
        if !scope.imports.words.contains(name) {
            return Vec::new();
        }
        let mut out: Vec<usize> = Vec::new();
        for imp in &scope.imports.texts {
            let words: Vec<&str> = lang.words(imp).into_iter().filter(|w| !lang.import_keywords.iter().any(|k| k == w)).collect();
            let Some(at) = words.iter().position(|w| *w == name) else { continue };
            // The path is written before the name (`use a::b::{X, Y}`), and
            // is the repo's: from here (`crate::`, `self::`) or a repo module.
            let relative = words.first().is_some_and(|w| lang.relative_prefixes.iter().any(|r| r == w));
            let path: Vec<&str> = words[..at].iter().copied().filter(|w| !lang.relative_prefixes.iter().any(|r| r == w)).collect();
            if relative || self.names_module(lang, &path) {
                out.extend(self.reexported(lang, scope, &words[..at], name));
            }
        }
        out.sort_by(|a, b| self[*a].id.cmp(&self[*b].id));
        out.dedup_by(|a, b| self[*a].id == self[*b].id);
        out
    }

    /// Whether an alias or an import brings `name` in from a repo path.
    fn brought_from_repo(&self, scope: &FileScope, lang: &LanguageSpec, name: &str) -> bool {
        if let Some((_, original)) = scope.aliases.iter().find(|(alias, _)| alias == name) {
            return self.import_from_repo(scope, lang, original);
        }
        scope.imports.words.contains(name) && scope.imports.texts.iter().any(|imp| lang.words(imp).contains(&name) && self.import_from_repo(scope, lang, imp)) && !self.reexported_from_outside(scope, lang, name)
    }

    /// Whether the repo module an import brings `name` from only passes it
    /// on from outside the repo (`pub use std::time::Duration`).
    fn reexported_from_outside(&self, scope: &FileScope, lang: &LanguageSpec, name: &str) -> bool {
        let package = lang.words(&scope.module).first().copied().unwrap_or_default();
        scope.imports.texts.iter().any(|imp| {
            let words: Vec<&str> = lang.words(imp).into_iter().filter(|w| !lang.import_keywords.iter().any(|k| k == w)).collect();
            let Some(at) = words.iter().position(|w| *w == name) else { return false };
            let path: Vec<&str> = words[..at].iter().copied().filter(|w| !lang.relative_prefixes.iter().any(|r| r == w)).collect();
            let wanted: &[&str] = if path.is_empty() { std::slice::from_ref(&package) } else { &path };
            let Some(last) = wanted.last() else { return false };
            let modules = self.module_named.get(&lang.name).and_then(|n| n.get(*last)).map_or(&[][..], Vec::as_slice);
            modules.iter().filter(|&&m| lang.words(&self[m].symbol).ends_with(wanted)).any(|&m| {
                // It defines nothing by the name, and brings it from outside.
                let Some(there) = self.files.get(&self[m].file) else { return false };
                !self.named(&lang.name, name).iter().any(|&c| self[c].module == self[m].symbol)
                    && there.imports.texts.iter().any(|t| lang.words(t).contains(&name) && !self.import_from_repo(there, lang, t))
            })
        })
    }

    /// Whether an import's path starts in the repo: marked relative, from a
    /// relative prefix, or from the file's own package.
    fn import_from_repo(&self, scope: &FileScope, lang: &LanguageSpec, imp: &str) -> bool {
        if lang.relative_import_markers.iter().any(|m| imp.contains(m.as_str())) {
            return true;
        }
        let package = lang.words(&scope.module).first().copied();
        let first = lang.words(imp).into_iter().find(|w| !lang.import_keywords.iter().any(|k| k == w));
        first.is_some_and(|w| lang.relative_prefixes.iter().any(|r| r == w) || Some(w) == package)
    }

    /// The named items (not components) the files have, by language.
    fn items(&self) -> HashSet<(&str, &str)> {
        self.files.values().filter_map(|f| Some((self[*f.entries.first()?].language.as_str(), &f.items))).flat_map(|(lang, items)| items.iter().map(move |i| (lang, i.as_str()))).collect()
    }

    fn scope_of(&self, i: usize) -> &FileScope {
        &self.files[&self[i].file]
    }

    /// Links `files` (each one's stamp and reading). Their entries are made
    /// again from the readings; other files' entries are read only. Returns
    /// every row of each file.
    pub fn link(&mut self, packs: &PackSet, types: &TypeSystem, files: Vec<(FileRow, FileExtract, Saids)>) -> Vec<FileRows> {
        let (files, saids): (Vec<(FileRow, FileExtract)>, Vec<Saids>) = files.into_iter().map(|(f, e, s)| ((f, e), s)).unzip();
        for (file, _) in &files {
            self.forget(&file.path);
        }
        let module_types: BTreeSet<&str> = packs.packs.iter().map(|p| p.spec.language.module_type.as_str()).collect();
        let is_type = |type_name: &str| types.resolve(type_name).map(|t| !t.executable && !module_types.iter().any(|m| t.is_a(m))).unwrap_or(false);
        // Modules first.
        let mut def_entry: HashMap<(usize, usize), usize> = HashMap::new();
        let mut module_entry: Vec<usize> = Vec::new();
        for (file, ex) in &files {
            let sep = packs.by_name(&ex.language).map_or("::", |p| p.spec.language.separator.as_str());
            let name = ex.module.rsplit(sep).next().unwrap_or(&ex.module).to_owned();
            let entry = Entry {
                id: ComponentId::for_symbol(&ex.language, &ex.module),
                symbol: ex.module.clone(),
                name,
                module: ex.module.clone(),
                language: ex.language.clone(),
                file: file.path.clone(),
                def: None,
                owner: None,
                is_type: false,
                executable: false,
                holds_data: false,
                self_name: None,
                signature: None,
                bases: Vec::new(),
                named_types: Vec::new(),
            };
            let i = self.add(entry);
            module_entry.push(i);
            let lang = packs.by_name(&ex.language).map(|p| &p.spec.language);
            self.files.insert(file.path.clone(), FileScope { module: ex.module.clone(), imports: Imports::new(ex.imports.iter().map(|i| i.text.clone()).collect(), lang), aliases: ex.aliases.clone(), items: ex.items.clone(), entries: vec![i] });
        }
        // Symbols: definitions outside scopes first (types), then scoped ones
        // and everything nested in them, since a scope needs its owner type known.
        let scoped_chain = |fi: usize, di: usize| {
            let defs = &files[fi].1.definitions;
            let mut current = Some(di);
            while let Some(i) = current {
                if defs[i].scope.is_some() {
                    return true;
                }
                current = defs[i].parent;
            }
            false
        };
        let mut pending: Vec<(usize, usize)> = files.iter().enumerate().flat_map(|(fi, (_, ex))| (0..ex.definitions.len()).map(move |di| (fi, di))).collect();
        for allow_scoped in [false, true] {
            loop {
                let before = pending.len();
                let mut still = Vec::new();
                for (fi, di) in std::mem::take(&mut pending) {
                    let (file, ex) = &files[fi];
                    let def = &ex.definitions[di];
                    let Some(pack) = packs.by_name(&ex.language) else { continue };
                    let lang = &pack.spec.language;
                    let parent_entry = def.parent.and_then(|p| def_entry.get(&(fi, p)).copied());
                    if (!allow_scoped && scoped_chain(fi, di)) || (def.parent.is_some() && parent_entry.is_none()) {
                        still.push((fi, di));
                        continue;
                    }
                    let (prefix, owner) = match (&def.scope, parent_entry) {
                        (Some(scope), _) => match find_type(self, &scope.target.text, &self.files[&file.path], lang).found() {
                            Some(owner) => (self[owner].symbol.clone(), Some(self[owner].id.clone())),
                            None => (format!("{}{}{}", ex.module, lang.separator, clean(&scope.target.text, lang)), None),
                        },
                        (None, Some(parent)) => (self[parent].symbol.clone(), Some(self[parent].id.clone())),
                        (None, None) => (ex.module.clone(), None),
                    };
                    let symbol = format!("{prefix}{}{}", lang.separator, def.name);
                    let resolved = types.resolve(&def.type_name).ok();
                    let resolved = resolved.as_ref();
                    let entry = Entry {
                        id: ComponentId::for_symbol(&ex.language, &symbol),
                        symbol,
                        name: def.name.clone(),
                        module: ex.module.clone(),
                        language: ex.language.clone(),
                        file: file.path.clone(),
                        def: Some(di),
                        owner,
                        is_type: is_type(&def.type_name),
                        executable: resolved.is_some_and(|t| t.executable),
                        holds_data: resolved.is_some_and(|t| t.allows_properties),
                        self_name: def.self_name.clone(),
                        signature: def.signature.clone(),
                        bases: def.bases.iter().map(|b| b.text.clone()).collect(),
                        named_types: Vec::new(),
                    };
                    let i = self.add(entry);
                    def_entry.insert((fi, di), i);
                    self.files.get_mut(&file.path).expect("added above").entries.push(i);
                }
                pending = still;
                if pending.len() == before || pending.is_empty() {
                    break;
                }
            }
        }
        // Implements: declared bases and scope interfaces (the rest of each
        // file's type refs are made with its other rows, in parallel).
        // The base's type as the server says (it wins over names), with who said it.
        let mut implements: Vec<Vec<(ComponentId, ComponentId, Option<String>)>> = vec![Vec::new(); files.len()];
        for (fi, (file, ex)) in files.iter().enumerate() {
            let Some(pack) = packs.by_name(&ex.language) else { continue };
            let lang = &pack.spec.language;
            let scope = &self.files[&file.path];
            let mut seen_scopes: HashSet<u32> = HashSet::new();
            for (di, def) in ex.definitions.iter().enumerate() {
                let Some(&ei) = def_entry.get(&(fi, di)) else { continue };
                let mut pairs: Vec<(usize, &crate::Named)> = def.bases.iter().map(|b| (ei, b)).collect();
                if let Some(s) = &def.scope
                    && seen_scopes.insert(s.target.span.start_byte)
                    && let (Some(iface), Some(owner)) = (&s.implements, self.owner(ei))
                {
                    pairs.push((owner, iface));
                }
                for (source, named) in pairs {
                    let server = saids[fi].at.get(&named.span.start_byte).filter(|s| s.confirmed);
                    let target = match server {
                        Some(s) => s.target.as_ref().and_then(|t| self.by_id.get(t).copied()),
                        None => find_type(self, &named.text, scope, lang).found(),
                    };
                    if let Some(target) = target
                        && target != source
                    {
                        implements[fi].push((self[source].id.clone(), self[target].id.clone(), server.map(|s| s.prover.clone())));
                    }
                }
            }
        }
        for (fi, (file, _)) in files.iter().enumerate() {
            for (source, target, _) in &implements[fi] {
                let list = self.implements.entry(source.clone()).or_default();
                if !list.iter().any(|(_, t)| t == target) {
                    list.push((file.path.clone(), target.clone()));
                }
            }
        }
        // Each file's type refs first: which type each signature names, as
        // the server says, is part of what a data flow through it carries.
        let this = &*self;
        let type_refs: Vec<Vec<RefRow>> = files.par_iter().zip(&saids).enumerate().map(|(fi, ((file, ex), said))| this.type_and_import_refs(packs, file, ex, fi, module_entry[fi], &def_entry, said)).collect();
        for (&(fi, _), &ei) in &def_entry {
            let id = self[ei].id.clone();
            if let Some(entry) = self.entries[ei].as_mut() {
                entry.named_types = named_types(type_refs[fi].iter(), &id);
            }
        }
        // Each file's rows: linked in parallel, against a symbol table no
        // longer changing.
        let this = &*self;
        let items = this.items();
        files
            .par_iter()
            .zip(&implements)
            .zip(&saids)
            .zip(type_refs)
            .enumerate()
            .map(|(fi, ((((file, ex), implements), said), mut refs))| {
                let mut rows = read_rows(file.clone(), ex);
                rows.components = this.component_rows(packs, ex, module_entry[fi], &def_entry, fi);
                let mut relations: BTreeMap<String, RelationRow> = BTreeMap::new();
                // Definitions are contained by their owner, else by their file's module.
                for (di, _) in ex.definitions.iter().enumerate() {
                    let Some(&ei) = def_entry.get(&(fi, di)) else { continue };
                    let container = this[ei].owner.clone().unwrap_or_else(|| this[module_entry[fi]].id.clone());
                    add_relation(&mut relations, RelationKind::Contains, container, this[ei].id.clone(), RelationDetail::default(), None);
                }
                for (source, target, prover) in implements {
                    add_relation(&mut relations, RelationKind::Implements, source.clone(), target.clone(), RelationDetail::default(), prover.clone());
                }
                this.link_calls(packs, ex, fi, module_entry[fi], &def_entry, said, &mut refs, &mut relations, &mut rows.flows);
                // Missing names the repo has as items no card shows (a
                // constant, a type alias): something of the repo after all.
                for r in refs.iter_mut().filter(|r| r.evidence == Evidence::NotFound && r.prover.is_none()) {
                    if items.contains(&(ex.language.as_str(), r.name.as_str())) {
                        r.evidence = Evidence::Member;
                    }
                }
                refs.sort_by_key(|r| r.start);
                rows.refs = unique_starts(refs);
                rows.relations = relations.into_values().collect();
                rows
            })
            .collect()
    }

    /// A file's type refs (scope targets, bases, the types its signatures
    /// and locals name) and import refs.
    #[allow(clippy::too_many_arguments)]
    fn type_and_import_refs(&self, packs: &PackSet, file: &FileRow, ex: &FileExtract, fi: usize, module: usize, def_entry: &HashMap<(usize, usize), usize>, said: &Saids) -> Vec<RefRow> {
        let mut out = Vec::new();
        let Some(pack) = packs.by_name(&ex.language) else { return out };
        let lang = &pack.spec.language;
        let scope = &self.files[&file.path];
        let mut seen_scopes: HashSet<u32> = HashSet::new();
        let typed = |out: &mut Vec<RefRow>, written: &crate::Named, within: usize| {
            for named in type_paths(written, lang) {
                // `Self` is the enclosing type; `Self::X`, something of it
                // (an associated type, a variant).
                let words = split(&named.text, lang);
                let enclosing = words.first().filter(|w| lang.self_type_names.contains(w)).and_then(|_| self_type(self, within));
                let found = match (enclosing, words.len()) {
                    (Some(t), 1) => TypeRef::Found(t),
                    (Some(_), _) => TypeRef::Member,
                    (None, _) => find_type(self, &named.text, scope, lang),
                };
                let mut row = type_ref(&named, Some(self[within].id.clone()), found.as_ref(self));
                as_said(&mut row, said.at.get(&named.span.start_byte));
                out.push(row);
            }
        };
        for (di, def) in ex.definitions.iter().enumerate() {
            let Some(&ei) = def_entry.get(&(fi, di)) else { continue };
            // The scope's target and interface, once per scope.
            if let Some(s) = &def.scope
                && seen_scopes.insert(s.target.span.start_byte)
            {
                typed(&mut out, &s.target, module);
                if let Some(iface) = &s.implements {
                    typed(&mut out, iface, module);
                }
            }
            for base in &def.bases {
                typed(&mut out, base, ei);
            }
            // The types its signature and locals name, each type in them.
            for written in &def.type_names {
                typed(&mut out, written, ei);
            }
        }
        let module_id = &self[module].id;
        for imp in &ex.imports {
            out.extend(import_refs(self, imp, scope, lang, module_id).into_iter().map(|mut row| {
                let start = row.start;
                as_said(&mut row, said.at.get(&start));
                row
            }));
        }
        out
    }

    fn component_rows(&self, packs: &PackSet, ex: &FileExtract, module: usize, def_entry: &HashMap<(usize, usize), usize>, fi: usize) -> Vec<ComponentRow> {
        let module_type = packs.by_name(&ex.language).map(|p| p.spec.language.module_type.clone()).unwrap_or_default();
        let mut out = vec![ComponentRow {
            id: self[module].id.clone(),
            parent: None,
            type_name: module_type,
            name: self[module].name.clone(),
            span: Span { start_line: 1, end_line: ex.lines, start_byte: 0, end_byte: ex.bytes },
            name_span: None,
            signature: None,
            detail: ComponentDetail {
                symbol: ex.module.clone(),
                syntax_kind: ex.root_kind.clone(),
                def: None,
                module: Some(ModuleReading {
                    imports: ex.imports.clone(),
                    aliases: ex.aliases.clone(),
                    macro_defs: ex.macro_defs.clone(),
                    items: ex.items.clone(),
                    unnamed_calls: ex.calls.iter().enumerate().filter(|(_, c)| c.name_span.is_none()).map(|(i, c)| (i, c.clone())).collect(),
                }),
            },
        }];
        for (di, def) in ex.definitions.iter().enumerate() {
            let Some(&ei) = def_entry.get(&(fi, di)) else { continue };
            let e = &self[ei];
            out.push(ComponentRow {
                id: e.id.clone(),
                parent: Some(e.owner.clone().unwrap_or_else(|| self[module].id.clone())),
                type_name: def.type_name.clone(),
                name: def.name.clone(),
                span: def.span,
                name_span: def.name_span,
                signature: def.signature.clone(),
                detail: ComponentDetail {
                    symbol: e.symbol.clone(),
                    syntax_kind: def.syntax_kind.clone(),
                    def: Some(DefPlace { index: di, parent: def.parent, scope: def.scope.clone(), bases: def.bases.clone(), self_name: def.self_name.clone(), type_names: def.type_names.clone() }),
                    module: None,
                },
            });
        }
        out
    }

    /// Calls, then data flow beyond arguments, of one file.
    #[allow(clippy::too_many_arguments)]
    fn link_calls(
        &self,
        packs: &PackSet,
        ex: &FileExtract,
        fi: usize,
        module: usize,
        def_entry: &HashMap<(usize, usize), usize>,
        said: &Saids,
        refs: &mut Vec<RefRow>,
        relations: &mut BTreeMap<String, RelationRow>,
        flows: &mut [FlowRow],
    ) {
        let Some(pack) = packs.by_name(&ex.language) else { return };
        let lang = &pack.spec.language;
        let cx = Cx { linker: self, packs, via: std::cell::RefCell::new(Vec::new()) };
        let callers: Vec<Option<usize>> = ex
            .calls
            .iter()
            .map(|call| match call.caller {
                Some(di) => def_entry.get(&(fi, di)).copied(),
                None => Some(module),
            })
            .collect();
        // What the server said wins over names; a component it named that's
        // gone (its file changed) is kept as the call's target, with no link.
        let mut gone: HashMap<usize, ComponentId> = HashMap::new();
        // The type each call went through (its receiver's): a change to that
        // type's bases or impls in another file re-links this one.
        let mut vias: HashMap<usize, Vec<String>> = HashMap::new();
        let targets: Vec<CallTarget> = ex
            .calls
            .iter()
            .zip(&callers)
            .enumerate()
            .map(|(ci, (call, caller))| match (said.calls.get(&ci), caller) {
                (Some(s), Some(_)) => match &s.target {
                    Some(id) => match self.by_id.get(id) {
                        Some(&e) => CallTarget::One(e),
                        None => {
                            gone.insert(ci, id.clone());
                            CallTarget::External
                        }
                    },
                    None => CallTarget::External,
                },
                (_, caller) => caller.map_or(CallTarget::External, |caller| {
                    cx.via.borrow_mut().clear();
                    let target = cx.resolve_call(caller, &call.callee, Some(call), 0);
                    let through = cx.via.take();
                    if !through.is_empty() {
                        vias.insert(ci, through.into_iter().map(|t| self[t].name.clone()).collect());
                    }
                    target
                }),
            })
            .collect();
        let proven = |ci: usize| said.calls.get(&ci).filter(|s| s.confirmed).map(|s| s.prover.clone());
        // (call span) -> its target and the call's place, for flows that
        // start at a call.
        let call_targets: HashMap<(u32, u32), (usize, Option<usize>)> = ex
            .calls
            .iter()
            .zip(&targets)
            .enumerate()
            .filter_map(|(ci, (call, t))| if let CallTarget::One(t) = t { Some(((call.span.start_byte, call.span.end_byte), (*t, Some(ci)))) } else { None })
            .collect();
        let mut order: BTreeMap<ComponentId, u32> = BTreeMap::new();
        for (ci, ((call, caller), target)) in ex.calls.iter().zip(&callers).zip(&targets).enumerate() {
            let within = caller.map(|c| self[c].id.clone());
            // A ref is a name: calling what a call returns (`make()(x)`) names
            // nothing, so it has no ref (its call still links below).
            let named = call.name_span.is_some();
            let name = match &call.callee {
                // Written in a form the reader keeps as text
                // (`<T as Trait>::name`): the name is its last word.
                Expr::Other(text) => lang.words(text).last().map_or_else(|| call_name(&call.callee), |w| (*w).to_owned()),
                callee => call_name(callee),
            };
            let mut row = RefRow {
                start: call.name_span.map_or(call.span.start_byte, |s| s.start_byte),
                end: call.name_span.map_or(call.span.end_byte, |s| s.end_byte),
                name,
                kind: if call.reference { RefKind::Value } else { RefKind::Call },
                within: within.clone(),
                target: None,
                evidence: Evidence::External,
                prover: None,
                detail: RefDetail { call: Some((ci, call.clone())), via: vias.remove(&ci).unwrap_or_default(), ..RefDetail::default() },
            };
            let Some(caller) = *caller else {
                if named {
                    refs.push(row);
                }
                continue;
            };
            let caller_id = self[caller].id.clone();
            let prover = proven(ci);
            match target {
                CallTarget::One(t) => {
                    row.target = Some(self[*t].id.clone());
                    row.evidence = Evidence::Name;
                }
                CallTarget::Many(c) | CallTarget::Unproven(c) => {
                    row.evidence = if matches!(target, CallTarget::Many(_)) { Evidence::Ambiguous } else { Evidence::Unproven };
                    // Each once (one module in two files is two entries).
                    let mut candidates: Vec<ComponentId> = c.iter().map(|&c| self[c].id.clone()).collect();
                    candidates.sort();
                    candidates.dedup();
                    row.detail.candidates = candidates;
                }
                // `T::name` with `T` a repo type that has no such component:
                // a member it doesn't make one of (a variant, a derived method).
                CallTarget::External if self.of_repo_type(self.scope_of(module), lang, &call.callee) || self.of_self_type(caller, lang, &call.callee) => row.evidence = Evidence::Member,
                // Written as in the repo, and nothing there has the name: not found.
                CallTarget::External if self.leads_into_repo(self.scope_of(module), lang, &call.callee) => row.evidence = Evidence::NotFound,
                CallTarget::External => {}
            }
            as_said(&mut row, said.calls.get(&ci));
            if let Some(id) = gone.get(&ci) {
                row.target = Some(id.clone());
            }
            if call.reference {
                // A function named as a value may be called later: a deferred
                // edge when the name is a known function, nothing otherwise.
                if let CallTarget::One(t) = target
                    && self[*t].def.is_some()
                    && !self[*t].is_type
                {
                    add_relation(relations, RelationKind::Calls, caller_id, self[*t].id.clone(), RelationDetail { sites: vec![call.span], deferred: true, ..RelationDetail::default() }, prover);
                }
                if named {
                    refs.push(row);
                }
                continue;
            }
            if let CallTarget::One(t) = target {
                let t = *t;
                let next = order.entry(caller_id.clone()).or_insert(0);
                let detail = RelationDetail { sites: vec![call.span], order: Some(*next), deferred: call.deferred, ..RelationDetail::default() };
                *next += 1;
                add_relation(relations, RelationKind::Calls, caller_id.clone(), self[t].id.clone(), detail, prover.clone());
                // Calling a type makes a value of it (`Report(...)`, `new Store()`).
                if self[t].holds_data {
                    let detail = RelationDetail { sites: vec![call.span], ports: Some(("makes".to_owned(), "value".to_owned())), data: Some(self[t].name.clone()), ..RelationDetail::default() };
                    add_relation(relations, RelationKind::DataFlow, caller_id.clone(), self[t].id.clone(), detail, prover.clone());
                }
                // Outputs of other repo calls passed in as arguments: the
                // producing call's target.
                for arg in &call.args {
                    let source = match call_targets.get(&(arg.producer_span.start_byte, arg.producer_span.end_byte)) {
                        Some((s, by)) => Some((*s, by.and_then(&proven))),
                        None => match cx.resolve_call(caller, &arg.producer, None, 0) {
                            CallTarget::One(s) => Some((s, None)),
                            _ => None,
                        },
                    };
                    let Some((source, source_proven)) = source else { continue };
                    match flow(self, source, t, arg.position, lang) {
                        Ok((ports, carried)) => {
                            // Proven only when both ends are, and the type
                            // each end names is the one the server says.
                            let typed = self[source].signature.as_ref().and_then(|s| s.returns.as_deref()).is_none_or(|r| self[source].types_confirmed(r)) && self[t].types_confirmed(&carried);
                            let detail = RelationDetail { sites: vec![call.span], ports: Some(ports), data: Some(carried), ..RelationDetail::default() };
                            add_relation(relations, RelationKind::DataFlow, self[source].id.clone(), self[t].id.clone(), detail, prover.clone().filter(|_| source_proven.is_some() && typed));
                        }
                        Err(reason) => row.detail.unverified.push((self[source].id.clone(), arg.position, reason)),
                    }
                }
            }
            if named {
                refs.push(row);
            }
        }
        // The paths calls are written through (`util` in `util::shout()`).
        let scope = self.scope_of(module);
        for (call, caller) in ex.calls.iter().zip(&callers) {
            let (Expr::Path(parts, sep), Some(name_span)) = (&call.callee, call.name_span) else { continue };
            let qualifiers = &parts[..parts.len() - 1];
            let written: usize = qualifiers.iter().map(|q| q.len() + sep.len()).sum();
            let Some(mut at) = (name_span.start_byte as usize).checked_sub(written) else { continue };
            if (at as u32) < call.span.start_byte {
                continue;
            }
            let within = caller.map(|c| self[c].id.clone());
            let mut written: Vec<&str> = Vec::new();
            for q in qualifiers {
                let span = Span { start_line: name_span.start_line, end_line: name_span.start_line, start_byte: at as u32, end_byte: (at + q.len()) as u32 };
                at += q.len() + sep.len();
                written.push(q.as_str());
                if lang.relative_prefixes.iter().any(|r| r == q) {
                    continue;
                }
                let found = self.path_target(scope, lang, &written);
                let mut row = ref_row(RefKind::Path, q, span, within.clone(), found, Vec::new());
                as_said(&mut row, said.at.get(&span.start_byte));
                refs.push(row);
            }
        }
        // Data flow beyond arguments: results returned or kept, values made,
        // the state of a definition's own type, and boundaries crossed.
        for flow_row in flows.iter_mut() {
            let raw = &flow_row.flow;
            let at = match raw.at {
                Some(di) => def_entry.get(&(fi, di)).copied(),
                None => Some(module),
            };
            let Some(at) = at else { continue };
            flow_row.within = Some(self[at].id.clone());
            let add = |relations: &mut BTreeMap<String, RelationRow>, source: usize, target: usize, ports: (String, String), data: Option<String>, prover: Option<String>| {
                if source != target {
                    let detail = RelationDetail { sites: vec![raw.span], ports: Some(ports), data, ..RelationDetail::default() };
                    add_relation(relations, RelationKind::DataFlow, self[source].id.clone(), self[target].id.clone(), detail, prover);
                }
            };
            match raw.kind {
                FlowKind::Returned | FlowKind::Bound => {
                    let Some(call) = &raw.call else { continue };
                    // The producing call's target, as the calls above found it.
                    let found = raw.call_span.and_then(|s| call_targets.get(&(s.start_byte, s.end_byte)).copied());
                    let (source, by) = match found {
                        Some((found, ci)) => (found, ci.and_then(&proven)),
                        None => match cx.resolve_call(at, call, None, 0) {
                            CallTarget::One(source) => (source, None),
                            _ => continue,
                        },
                    };
                    if self[source].is_type {
                        continue;
                    }
                    // What comes back, as written (`Result<Report, Error>`, `(u32, u32)`).
                    let carried = self[source].signature.as_ref().and_then(|s| s.returns.as_deref()).map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "));
                    let into = if raw.kind == FlowKind::Returned { "returns".to_owned() } else { format!("kept as {}", raw.names.join(", ")) };
                    // Proven when the call is, and what it returns is the type the server says.
                    let typed = carried.as_deref().is_none_or(|c| self[source].types_confirmed(c));
                    add(relations, source, at, ("return".to_owned(), into), carried, by.filter(|_| typed));
                }
                FlowKind::Constructs => {
                    let found = find_type(self, &raw.what, self.scope_of(at), lang);
                    // The type made, as the server says (it wins over names).
                    let server = raw.what_span.and_then(|s| said.at.get(&s.start_byte)).filter(|s| s.confirmed);
                    let made = match server {
                        Some(s) => s.target.as_ref().and_then(|t| self.by_id.get(t).copied()),
                        None => found.found_ref(),
                    };
                    if let Some(ty) = made.filter(|t| self[*t].holds_data) {
                        add(relations, at, ty, ("makes".to_owned(), "value".to_owned()), Some(self[ty].name.clone()), server.map(|s| s.prover.clone()));
                    }
                    for (kind, name, span) in flow_refs(raw) {
                        let found = match found.found_ref() {
                            Some(t) if self[t].holds_data => Ok(self[t].id.clone()),
                            Some(_) => Err(Evidence::External),
                            None => Err(found.evidence()),
                        };
                        let mut row = ref_row(kind, name, span, Some(self[at].id.clone()), found, Vec::new());
                        as_said(&mut row, said.at.get(&span.start_byte));
                        refs.push(row);
                    }
                }
                FlowKind::Writes | FlowKind::Reads => {
                    let read_as = if raw.kind == FlowKind::Writes { Access::Write } else { Access::Read };
                    // A field of another value (`c.count`): which type it's
                    // on, only the server can say.
                    let reader_owner = if raw.through.is_some() { None } else { self.owner(at).filter(|o| self[*o].holds_data) };
                    // The field's type and use, as the server says (it wins).
                    let server = raw.what_span.and_then(|s| said.at.get(&s.start_byte)).filter(|s| s.confirmed);
                    let (owner, access) = match server {
                        Some(s) => (s.target.as_ref().and_then(|t| self.by_id.get(t).copied()), s.access.unwrap_or(read_as)),
                        None => (reader_owner, read_as),
                    };
                    // Proven when the server said both which field and how
                    // it's used; else what's read here is the reader's guess.
                    let proven_by = server.filter(|s| s.access.is_some()).map(|s| s.prover.clone());
                    for (kind, name, span) in flow_refs(raw) {
                        let found = owner.map(|o| self[o].id.clone()).ok_or(if raw.through.is_some() { Evidence::Unproven } else { Evidence::NotFound });
                        let mut row = ref_row(kind, name, span, Some(self[at].id.clone()), found, Vec::new());
                        as_said(&mut row, said.at.get(&span.start_byte));
                        row.detail.access = Some(access);
                        row.detail.server_access = server.and_then(|s| s.access);
                        refs.push(row);
                    }
                    let Some(owner) = owner.filter(|o| self[*o].holds_data) else { continue };
                    let data = Some(format!("{}.{}", self[owner].name, raw.what));
                    let prover = proven_by;
                    match access {
                        Access::Write => add(relations, at, owner, ("writes".to_owned(), raw.what.clone()), data, prover),
                        Access::Read => add(relations, owner, at, (raw.what.clone(), "reads".to_owned()), data, prover),
                    }
                }
                FlowKind::Sends => {}
                FlowKind::Receives => {
                    if raw.names.is_empty() {
                        flow_row.receivers.push(self[at].id.clone());
                    }
                    // Named handlers (route handlers) receive, wherever they're registered.
                    for (kind, name, span) in flow_refs(raw) {
                        let target = cx.resolve_call(at, &Expr::from_written(name, lang), None, 0);
                        let (found, candidates) = match &target {
                            CallTarget::One(h) => {
                                flow_row.receivers.push(self[*h].id.clone());
                                (Ok(self[*h].id.clone()), Vec::new())
                            }
                            CallTarget::Many(c) => (Err(Evidence::Ambiguous), c.iter().map(|&c| self[c].id.clone()).collect()),
                            CallTarget::Unproven(c) => (Err(Evidence::Unproven), c.iter().map(|&c| self[c].id.clone()).collect()),
                            CallTarget::External => (Err(Evidence::External), Vec::new()),
                        };
                        refs.push(ref_row(kind, name, span, Some(self[at].id.clone()), found, candidates));
                    }
                }
            }
        }
    }
}

/// A ref row for a name linked to `found` (its target, or why not).
fn ref_row(kind: RefKind, name: &str, span: Span, within: Option<ComponentId>, found: Result<ComponentId, Evidence>, mut candidates: Vec<ComponentId>) -> RefRow {
    candidates.sort();
    candidates.dedup();
    let (target, evidence) = match found {
        Ok(t) => (Some(t), Evidence::Name),
        Err(e) => (None, e),
    };
    RefRow { start: span.start_byte, end: span.end_byte, name: name.to_owned(), kind, within, target, evidence, prover: None, detail: RefDetail { candidates, ..RefDetail::default() } }
}

/// A type named (a scope's target or interface, a base), linked.
fn type_ref(named: &crate::Named, within: Option<ComponentId>, found: Result<ComponentId, Evidence>) -> RefRow {
    let name = named.text.rsplit(|c: char| !(c.is_alphanumeric() || c == '_')).find(|w| !w.is_empty()).unwrap_or(&named.text);
    ref_row(RefKind::Type, name, named.span, within, found, Vec::new())
}

/// The type paths in a written type, each spanning its name
/// (`Result<Report, Error>` names `Result`, `Report` and `Error`;
/// `store::Store` is one path, spanning `Store`).
fn type_paths(written: &crate::Named, lang: &LanguageSpec) -> Vec<crate::Named> {
    let text = written.text.as_str();
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut end = 0;
    let mut i = 0;
    // Each path, spanning its name (the last word: `Display` in `std::fmt::Display`).
    let push = |out: &mut Vec<crate::Named>, from: usize, to: usize| {
        let path = &text[from..to];
        let noise = lang.type_noise.iter().any(|w| w == path) || lang.reference_markers.iter().any(|m| m == path);
        if !noise {
            let name_from = path.char_indices().filter(|(_, c)| !lang.is_identifier_char(*c)).map(|(i, c)| i + c.len_utf8()).next_back().map_or(from, |i| from + i);
            out.push(crate::Named { text: path.to_owned(), span: sub_span(written.span, text, name_from, to) });
        }
    };
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().expect("in bounds");
        if lang.is_identifier_char(c) {
            start.get_or_insert(i);
            i += c.len_utf8();
            end = i;
            continue;
        }
        // A separator inside a path (`a::B`), between two names.
        if let Some(sep) = start.and(lang.call_separators.iter().find(|sep| rest.starts_with(sep.as_str()) && rest[sep.len()..].chars().next().is_some_and(|n| lang.is_identifier_char(n)))) {
            i += sep.len();
            continue;
        }
        if let Some(from) = start.take() {
            push(&mut out, from, end);
        }
        i += c.len_utf8();
    }
    if let Some(from) = start {
        push(&mut out, from, end);
    }
    out
}

/// The span of `text[from..to]`, `text` being written at `outer`.
fn sub_span(outer: Span, text: &str, from: usize, to: usize) -> Span {
    let line = |at: usize| outer.start_line + text[..at].matches('\n').count() as u32;
    Span { start_line: line(from), end_line: line(to), start_byte: outer.start_byte + from as u32, end_byte: outer.start_byte + to as u32 }
}

/// Each name an import writes, linked: a definition it brings in, a repo
/// module its path names, or why neither (names in the repo elsewhere:
/// external; nowhere: not found). The names an alias gives aren't refs.
fn import_refs(linker: &Linker, imp: &crate::Named, scope: &FileScope, lang: &LanguageSpec, within: &ComponentId) -> Vec<RefRow> {
    let text = imp.text.as_str();
    let from_repo = linker.import_from_repo(scope, lang, text);
    let mut out = Vec::new();
    let mut path: Vec<&str> = Vec::new();
    // The same, relative prefixes kept (`super::x` is the module above).
    let mut written: Vec<&str> = Vec::new();
    let mut at = 0;
    for word in text.split(|c: char| !lang.is_identifier_char(c)) {
        let from = at;
        at += word.len() + text[at + word.len()..].chars().next().map_or(0, char::len_utf8);
        if word.is_empty() || lang.import_keywords.iter().any(|k| k == word) {
            continue;
        }
        if lang.relative_prefixes.iter().any(|r| r == word) {
            written.push(word);
            continue;
        }
        if scope.aliases.iter().any(|(alias, _)| alias == word) {
            continue;
        }
        path.push(word);
        written.push(word);
        let span = sub_span(imp.span, text, from, from + word.len());
        // Importable definitions only (not members), once each.
        let mut brought: Vec<usize> = linker.named(&lang.name, word).iter().copied().filter(|&c| linker[c].owner.is_none() && imports_text_from(std::slice::from_ref(&imp.text), lang, &scope.module, word, &linker[c].module)).collect();
        // Not defined where the path says: what that module brings in
        // itself (a re-export), one step.
        if brought.is_empty() && from_repo {
            brought = linker.reexported(lang, scope, &written[..written.len() - 1], word);
        }
        brought.sort_by(|a, b| linker[*a].id.cmp(&linker[*b].id));
        brought.dedup_by(|a, b| linker[*a].id == linker[*b].id);
        // Modules with the name, narrowed to those whose path ends with the
        // most of the path written so far.
        let named_modules = linker.module_named.get(&lang.name).and_then(|n| n.get(word)).map_or(&[][..], Vec::as_slice);
        let matched = |m: usize| (1..=path.len()).rev().find(|&n| lang.words(&linker[m].symbol).ends_with(&path[path.len() - n..])).unwrap_or(0);
        let best = named_modules.iter().map(|&m| matched(m)).max().unwrap_or(0);
        let mut modules: Vec<usize> = named_modules.iter().copied().filter(|&m| matched(m) == best).collect();
        modules.sort_by(|a, b| linker[*a].id.cmp(&linker[*b].id));
        modules.dedup_by(|a, b| linker[*a].id == linker[*b].id);
        let (found, candidates) = match (brought.as_slice(), modules.as_slice()) {
            ([one], _) | ([], [one]) => (Ok(linker[*one].id.clone()), Vec::new()),
            // Nothing found: missing if the import's path is the repo's, else outside.
            ([], []) if from_repo => (Err(Evidence::NotFound), Vec::new()),
            ([], []) => (Err(Evidence::External), Vec::new()),
            ([], many) => (Err(Evidence::Ambiguous), many.iter().map(|&c| linker[c].id.clone()).collect()),
            (many, _) => (Err(Evidence::Ambiguous), many.iter().map(|&c| linker[c].id.clone()).collect()),
        };
        out.push(ref_row(RefKind::Import, word, span, Some(within.clone()), found, candidates));
    }
    out
}

/// One ref per name (the table's key is the file and start): of refs
/// starting at the same byte, the one spanning the least is the name (a
/// wider one spans more than its name), and of equals the first is kept
/// (calls come before the handlers and types that name the same place).
fn unique_starts(refs: Vec<RefRow>) -> Vec<RefRow> {
    let mut at: HashMap<u32, usize> = HashMap::new();
    let mut out: Vec<RefRow> = Vec::with_capacity(refs.len());
    for r in refs {
        match at.get(&r.start) {
            Some(&i) if r.end - r.start < out[i].end - out[i].start => out[i] = r,
            Some(_) => {}
            None => {
                at.insert(r.start, out.len());
                out.push(r);
            }
        }
    }
    out
}

/// Adds a relation to a file's, or folds it into the one with the same id.
fn add_relation(relations: &mut BTreeMap<String, RelationRow>, kind: RelationKind, source: ComponentId, target: ComponentId, detail: RelationDetail, prover: Option<String>) {
    if source == target {
        return;
    }
    let mut relation = arch_graph::Relation::new(kind, source.clone(), target.clone(), arch_graph::Provenance::OBSERVED);
    relation.ports = detail.ports.clone();
    let id = relation.id().0;
    match relations.get_mut(&id) {
        Some(row) => {
            row.detail.deferred = row.detail.deferred && detail.deferred;
            row.detail.order = match (row.detail.order, detail.order) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            for site in detail.sites {
                if !row.detail.sites.contains(&site) {
                    row.detail.sites.push(site);
                }
            }
            if row.detail.data.is_none() {
                row.detail.data = detail.data;
            }
            // Proven once any of its calls is.
            if row.prover.is_none() && prover.is_some() {
                row.prover = prover;
                row.evidence = Evidence::Server;
            }
        }
        None => {
            let evidence = if prover.is_some() { Evidence::Server } else { Evidence::Name };
            relations.insert(id, RelationRow { kind, source, target, detail, evidence, prover });
        }
    }
}

/// Strips bracketed groups (generics, arguments) and leading reference
/// markers/noise words from a type or callee text.
pub(crate) fn clean(text: &str, lang: &LanguageSpec) -> String {
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

/// A callee with a leading alias replaced by what it stands for (its last name).
fn de_alias_expr(callee: &Expr, aliases: &[(String, String)], lang: &LanguageSpec) -> Expr {
    let original = callee.head().and_then(|head| aliases.iter().find(|(alias, _)| alias == head)).and_then(|(_, original)| lang.words(original).last().map(|w| (*w).to_owned()));
    match original {
        Some(original) => callee.with_head(&original),
        None => callee.clone(),
    }
}

/// A written type with a leading alias replaced by what it stands for (its last name).
fn de_alias(text: &str, aliases: &[(String, String)], lang: &LanguageSpec) -> String {
    let text = text.trim();
    let head_end = lang.call_separators.iter().filter_map(|sep| text.find(sep.as_str())).min().unwrap_or(text.len());
    let head = &text[..head_end];
    match aliases.iter().find(|(alias, _)| alias == head).and_then(|(_, original)| lang.words(original).last().map(|w| (*w).to_owned())) {
        Some(original) => format!("{original}{}", &text[head_end..]),
        None => text.to_owned(),
    }
}

/// Ports for `source`'s output flowing into `target`'s argument `position`,
/// when both types are written and carry the same data.
fn flow(linker: &Linker, source: usize, target: usize, position: u32, lang: &LanguageSpec) -> Result<((String, String), String), String> {
    // A type by its last path segment (`store.Store`, `crate::x::Store` -> Store).
    let base = |t: &str| split(t, lang).pop().unwrap_or_default();
    let produced = linker[source].signature.as_ref().and_then(|s| s.returns.as_deref()).map(|t| {
        // `Self` (and the like) means the producer's own type.
        let t = base(t);
        match self_type(linker, source) {
            Some(own) if lang.self_names.contains(&t) => linker[own].name.clone(),
            _ => t,
        }
    });
    let Some(produced) = produced else { return Err(format!("{} declares no return type", linker[source].symbol)) };
    let params: Vec<&arch_graph::Param> =
        linker[target].signature.as_ref().map(|s| s.params.iter().filter(|p| !lang.words(&p.name).iter().any(|w| lang.self_names.iter().any(|n| n == w))).collect()).unwrap_or_default();
    let Some(param) = params.get(position as usize) else { return Err(format!("{} has no parameter at position {position}", linker[target].symbol)) };
    let Some(wanted) = param.ty.as_deref().map(base) else { return Err(format!("parameter '{}' of {} has no type", param.name, linker[target].symbol)) };
    let compatible = produced == wanted || lang.compatible_types.iter().any(|group| group.contains(&produced) && group.contains(&wanted));
    if compatible { Ok((("return".to_owned(), param.name.clone()), produced)) } else { Err(format!("{produced} doesn't match parameter '{}': {wanted}", param.name)) }
}

/// What a written type name refers to, seen from a file.
enum TypeRef {
    /// A type in the repo.
    Found(usize),
    /// Not a repo type: the name is written as from outside the repo, the
    /// qualified path leads elsewhere, or the file imports it from elsewhere.
    Outside,
    /// Written as in the repo (a path or an import from it), and no repo
    /// definition has the name.
    Missing,
    /// Repo types share the name, but nothing proves which (not local, not
    /// imported).
    Unknown,
    /// Something of a repo type that isn't a type of its own (a variant
    /// written as a struct, `Self::Err`).
    Member,
}

impl TypeRef {
    fn found(self) -> Option<usize> {
        self.found_ref()
    }

    fn found_ref(&self) -> Option<usize> {
        match self {
            TypeRef::Found(t) => Some(*t),
            _ => None,
        }
    }

    fn evidence(&self) -> Evidence {
        match self {
            TypeRef::Found(_) => Evidence::Name,
            TypeRef::Outside => Evidence::External,
            TypeRef::Missing => Evidence::NotFound,
            TypeRef::Unknown => Evidence::Unproven,
            TypeRef::Member => Evidence::Member,
        }
    }

    fn as_ref(&self, linker: &Linker) -> Result<ComponentId, Evidence> {
        match self {
            TypeRef::Found(t) => Ok(linker[*t].id.clone()),
            other => Err(other.evidence()),
        }
    }
}

/// Finds a type component (one that can own members) by its written name,
/// among the components of the same language, by the same evidence as calls:
/// a qualified path, the file's own module, or its imports (aliases first).
fn find_type(linker: &Linker, text: &str, scope: &FileScope, lang: &LanguageSpec) -> TypeRef {
    let text = de_alias(text, &scope.aliases, lang);
    let raw = split(&text, lang);
    let anchored = raw.first().is_some_and(|w| lang.relative_prefixes.contains(w));
    let parts: Vec<String> = raw.iter().filter(|p| !lang.relative_prefixes.contains(p)).cloned().collect();
    let Some(name) = parts.last() else { return TypeRef::Outside };
    let candidates: Vec<usize> = linker.named(&lang.name, name).iter().copied().filter(|&i| linker[i].is_type).collect();
    // Written from here (`crate::T`, `super::T`): the type that module
    // defines, or brings in.
    if anchored {
        let written: Vec<&str> = raw[..raw.len() - 1].iter().map(String::as_str).collect();
        let modules: Vec<&str> = linker.written_modules(lang, scope, &written).into_iter().map(|m| linker[m].symbol.as_str()).collect();
        let defined: Vec<usize> = candidates.iter().copied().filter(|&c| modules.contains(&linker[c].module.as_str())).collect();
        let found = if defined.is_empty() { linker.reexported(lang, scope, &written, name).into_iter().filter(|&c| linker[c].is_type).collect() } else { defined };
        match found.as_slice() {
            [one] => return TypeRef::Found(*one),
            [_, ..] => return TypeRef::Unknown,
            [] => {}
        }
    }
    if candidates.is_empty() {
        // `T::Variant` with `T` a repo type: something of it.
        if parts.len() >= 2 && matches!(find_type(linker, &raw[..raw.len() - 1].join(&lang.separator), scope, lang), TypeRef::Found(_)) {
            return TypeRef::Member;
        }
        // Written as in the repo, and nothing there has the name: missing.
        let written = if parts.len() >= 2 { Expr::Path(parts.clone(), String::new()) } else { Expr::Name(name.clone()) };
        let missing = anchored || linker.leads_into_repo(scope, lang, &written);
        return if missing { TypeRef::Missing } else { TypeRef::Outside };
    }
    if parts.len() >= 2 {
        return match path_matches(linker, &candidates, &parts, lang).as_slice() {
            // Not defined where the path says: a repo module it names may
            // bring it in (`crate::T`, `other_crate::T` re-exported).
            [] => {
                let qualifier: Vec<&str> = parts[..parts.len() - 1].iter().map(String::as_str).collect();
                if !(anchored || linker.names_module(lang, &qualifier)) {
                    return TypeRef::Outside;
                }
                let written: Vec<&str> = raw[..raw.len() - 1].iter().map(String::as_str).collect();
                let through: Vec<usize> = linker.reexported(lang, scope, &written, name).into_iter().filter(|&c| linker[c].is_type).collect();
                match through.as_slice() {
                    [one] => TypeRef::Found(*one),
                    [] => TypeRef::Outside,
                    _ => TypeRef::Unknown,
                }
            }
            // `a::T` proves nothing unless `a` itself is in scope.
            [one] if anchored || head_in_scope(linker, scope, lang, &parts[0]) => TypeRef::Found(*one),
            _ => TypeRef::Unknown,
        };
    }
    let local: Vec<usize> = candidates.iter().copied().filter(|&c| linker[c].module == scope.module).collect();
    if let [one] = local.as_slice() {
        return TypeRef::Found(*one);
    }
    let imported: Vec<usize> = candidates.iter().copied().filter(|&c| imports_from(&scope.imports, lang, &scope.module, name, &linker[c].module)).collect();
    match imported.as_slice() {
        [one] => TypeRef::Found(*one),
        [] if scope.imports.words.contains(name.as_str()) => {
            // Brought from a repo module that re-exports it.
            let through: Vec<usize> = linker.through_reexport(scope, lang, name).into_iter().filter(|&c| linker[c].is_type).collect();
            match through.as_slice() {
                [one] => TypeRef::Found(*one),
                [] => TypeRef::Outside,
                _ => TypeRef::Unknown,
            }
        }
        _ => TypeRef::Unknown,
    }
}

/// Whether the first segment of a qualified path means something in a
/// file: a local definition, a submodule of its module, the package itself,
/// or an imported name.
fn head_in_scope(linker: &Linker, scope: &FileScope, lang: &LanguageSpec, head: &str) -> bool {
    let submodule = format!("{}{}{head}", scope.module, lang.separator);
    scope.imports.words.contains(head)
        || scope.aliases.iter().any(|(alias, _)| alias == head)
        || lang.words(&scope.module).first() == Some(&head)
        || linker.named(&lang.name, head).iter().any(|&i| linker[i].module == scope.module)
        || linker.modules.contains_key(&(lang.name.clone(), submodule))
}

/// Whether one of `imports` brings `name` in from `module` (the path names it,
/// whole, or a package above it re-exporting; by name or with a glob). A
/// path of only relative markers means the importer's module or one above.
fn imports_from(imports: &Imports, lang: &LanguageSpec, importer: &str, name: &str, module: &str) -> bool {
    if !imports.could_bring(name) {
        return false;
    }
    imports_text_from(&imports.texts, lang, importer, name, module)
}

/// `imports_from`, over import texts.
fn imports_text_from(imports: &[String], lang: &LanguageSpec, importer: &str, name: &str, module: &str) -> bool {
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


/// Candidates whose symbol ends with the written qualified path.
fn path_matches(linker: &Linker, candidates: &[usize], parts: &[String], lang: &LanguageSpec) -> Vec<usize> {
    if parts.len() < 2 {
        return Vec::new();
    }
    let wanted: Vec<&str> = parts.iter().map(String::as_str).collect();
    candidates.iter().copied().filter(|&c| lang.words(&linker[c].symbol).ends_with(&wanted)).collect()
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
fn self_type(linker: &Linker, entry: usize) -> Option<usize> {
    let mut current = linker.owner(entry);
    let mut steps = 0;
    while let Some(o) = current {
        if linker[o].is_type {
            return Some(o);
        }
        steps += 1;
        if steps > 64 {
            return None;
        }
        current = linker.owner(o);
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
    linker: &'a Linker,
    packs: &'a PackSet,
    /// The type the last call resolved at the top went through (its
    /// receiver's), whether or not it had the member.
    via: std::cell::RefCell<Vec<usize>>,
}

impl Cx<'_> {
    fn lang(&self, entry: usize) -> Option<&LanguageSpec> {
        self.packs.by_name(&self.linker[entry].language).map(|p| &p.spec.language)
    }

    /// The type a written type text names, seen from `context`.
    fn written_type(&self, context: usize, text: &str) -> Receiver {
        let Some(lang) = self.lang(context) else { return Receiver::Unknown };
        let Some(base) = split(text, lang).pop() else { return Receiver::Unknown };
        if lang.self_names.contains(&base) {
            return self_type(self.linker, context).map_or(Receiver::Unknown, Receiver::Typed);
        }
        match find_type(self.linker, text, self.linker.scope_of(context), lang) {
            TypeRef::Found(t) => Receiver::Typed(t),
            TypeRef::Unknown | TypeRef::Member => Receiver::Unknown,
            TypeRef::Outside | TypeRef::Missing => Receiver::Outside,
        }
    }

    /// The type of what calling `target` gives back.
    fn output_type(&self, target: usize) -> Receiver {
        if self.linker[target].is_type {
            return Receiver::Typed(target); // a constructor call
        }
        match self.linker[target].signature.as_ref().and_then(|s| s.returns.as_deref()) {
            Some(returns) => self.written_type(target, returns),
            None => Receiver::Unknown,
        }
    }

    fn call_output(&self, caller: usize, callee: &Expr, depth: u32) -> Receiver {
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
    fn receiver(&self, caller: usize, qualifier: &Expr, call: Option<&RawCall>, depth: u32) -> Receiver {
        let Some(lang) = self.lang(caller) else { return Receiver::Unknown };
        let written = qualifier.render();
        // super: the enclosing type's bases (`super`, `super()`).
        if lang.super_receivers.contains(&written) {
            return self_type(self.linker, caller).map_or(Receiver::Unknown, Receiver::Base);
        }
        match qualifier {
            // A call's result: `Greeter().greet`, `new Greeter().greet`.
            Expr::Call(inner) => return self.call_output(caller, inner, depth),
            Expr::Name(q) => {
                // self / this / the method's named receiver.
                if lang.self_names.iter().any(|s| s == q) || self.linker[caller].self_name.as_deref() == Some(q.as_str()) {
                    return self_type(self.linker, caller).map_or(Receiver::Unknown, Receiver::Typed);
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
                if let Some(param) = self.linker[caller].signature.as_ref().and_then(|s| s.params.iter().find(|p| lang.strip_references(&p.name) == name)) {
                    return match &param.ty {
                        Some(ty) => self.written_type(caller, ty),
                        None => Receiver::Unknown,
                    };
                }
            }
            _ => {}
        }
        // A type named directly (associated calls on a type value, `ns.Store`).
        if qualifier.names().is_none() {
            return Receiver::Unknown;
        }
        match find_type(self.linker, &written, self.linker.scope_of(caller), lang) {
            TypeRef::Found(t) => Receiver::Typed(t),
            TypeRef::Outside | TypeRef::Missing | TypeRef::Unknown | TypeRef::Member => Receiver::Unknown,
        }
    }

    /// What `ty` extends or implements, transitively: declared bases and
    /// interfaces from scopes (`impl Trait for Type`), nearest first.
    fn bases(&self, ty: usize) -> Vec<usize> {
        let linker = self.linker;
        let mut types = vec![ty];
        let mut i = 0;
        while i < types.len() && i < 32 {
            let t = types[i];
            let mut next: Vec<usize> = linker.implements.get(&linker[t].id).into_iter().flatten().filter_map(|(_, target)| linker.by_id.get(target).copied()).collect();
            if linker[t].def.is_some()
                && let Some(lang) = self.lang(t)
            {
                let scope = linker.scope_of(t);
                next.extend(linker[t].bases.iter().filter_map(|base| find_type(linker, base, scope, lang).found()));
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
                let found: Vec<usize> = self.linker.members_named(&self.linker[t].id, name).to_vec();
                (!found.is_empty()).then_some(found)
            })
            .unwrap_or_default()
    }

    fn is_member(&self, c: usize) -> bool {
        self.linker.owner(c).is_some_and(|o| self.linker[o].is_type)
    }

    fn resolve_call(&self, caller: usize, callee: &Expr, call: Option<&RawCall>, depth: u32) -> CallTarget {
        let Some(lang) = self.lang(caller) else { return CallTarget::External };
        let scope = self.linker.scope_of(caller);
        let callee = de_alias_expr(callee, &scope.aliases, lang);
        let Some((qualifier, sep, name)) = callee.split() else {
            return match &callee {
                Expr::Name(name) => self.bare(caller, name, &scope.imports, lang),
                // What a call gives, called (`make()()`), or something else: not a component.
                _ => CallTarget::External,
            };
        };
        let candidates: Vec<usize> = self.linker.named(&lang.name, name).to_vec();
        let mut receiver = Receiver::Unknown;
        if lang.is_member_separator(sep) {
            receiver = self.receiver(caller, &qualifier, call, depth);
            if depth == 0
                && let Receiver::Typed(t) | Receiver::Base(t) = receiver
            {
                let mut through = vec![t];
                through.extend(self.bases(t));
                *self.via.borrow_mut() = through;
            }
            match receiver {
                Receiver::Typed(t) => return narrow(self.members(t, name)).unwrap_or(CallTarget::External),
                Receiver::Base(t) => {
                    let bases = self.bases(t);
                    return narrow(self.members_of(&bases, name)).unwrap_or(CallTarget::External);
                }
                Receiver::Outside => return CallTarget::External,
                Receiver::Unknown => {}
            }
        }
        if lang.is_path_separator(sep)
            && let Some(raw) = callee.names()
        {
            // `Self::name` in an impl: the enclosing type's own.
            if let [head, _] = raw.as_slice()
                && lang.self_type_names.iter().any(|s| s == head)
                && let Some(t) = self_type(self.linker, caller)
            {
                return narrow(self.members(t, name)).unwrap_or(CallTarget::External);
            }
            let raw: Vec<String> = raw.into_iter().map(str::to_owned).collect();
            let anchored = raw.first().is_some_and(|w| lang.relative_prefixes.contains(w));
            let path: Vec<String> = raw.iter().filter(|p| !lang.relative_prefixes.contains(p)).cloned().collect();
            let mut matches = path_matches(self.linker, &candidates, &path, lang);
            // `a::f` where module `a` doesn't define `f` but re-exports it.
            let qualifier: Vec<&str> = path[..path.len().saturating_sub(1)].iter().map(String::as_str).collect();
            if matches.is_empty() && (anchored || self.linker.names_module(lang, &qualifier)) {
                let written: Vec<&str> = raw[..raw.len().saturating_sub(1)].iter().map(String::as_str).collect();
                matches = self.linker.reexported(lang, scope, &written, name).into_iter().filter(|&c| !self.is_member(c)).collect();
            }
            if !matches.is_empty() {
                // `a::f` only proves `f` when `a` itself is in scope here.
                return match anchored || path.first().is_some_and(|head| head_in_scope(self.linker, scope, lang, head)) {
                    true => narrow(matches).unwrap_or(CallTarget::External),
                    false => CallTarget::Unproven(matches),
                };
            }
        }
        // A value of unknown type: any member with the name could be meant.
        let members: Vec<usize> = candidates.into_iter().filter(|&c| self.is_member(c)).collect();
        match receiver {
            Receiver::Unknown if lang.is_member_separator(sep) && !members.is_empty() => CallTarget::Unproven(members),
            _ => CallTarget::External,
        }
    }

    /// The module declared inside a file that `entry` sits in (an inline
    /// `mod`), through the functions and types around it; None: the file's own.
    fn space(&self, entry: usize) -> Option<usize> {
        let mut at = self.linker.owner(entry);
        while let Some(o) = at {
            let e = &self.linker[o];
            if !e.executable && !e.is_type && !e.is_module() {
                return Some(o);
            }
            at = self.linker.owner(o);
        }
        None
    }

    /// A bare name reaches free functions or types (constructors) in its own
    /// module, or imported ones; anything else is unproven.
    fn bare(&self, caller: usize, name: &str, imports: &Imports, lang: &LanguageSpec) -> CallTarget {
        let linker = self.linker;
        let free: Vec<usize> = linker.named(&lang.name, name).iter().copied().filter(|&i| !self.is_member(i)).collect();
        if free.is_empty() {
            return CallTarget::External;
        }
        // A definition nested in a function is seen only inside that function:
        // the nearest one around the caller wins, as scoping does.
        let local_to = |c: usize| linker.owner(c).filter(|&o| linker[o].executable);
        let mut scope = Some(caller);
        while let Some(s) = scope {
            if let Some(target) = narrow(free.iter().copied().filter(|&c| local_to(c) == Some(s)).collect()) {
                return target;
            }
            scope = linker.owner(s).filter(|&o| linker[o].executable);
        }
        let free: Vec<usize> = free.into_iter().filter(|&c| local_to(c).is_none()).collect();
        if free.is_empty() {
            return CallTarget::External;
        }
        // In its own module: the same file, and the same module inside it
        // (what's in an inline `mod tests` isn't seen outside it by name).
        let caller_module = linker[caller].module.as_str();
        let space = self.space(caller);
        if let Some(target) = narrow(free.iter().copied().filter(|&c| linker[c].module == caller_module && self.space(c) == space).collect()) {
            return target;
        }
        let imported: Vec<usize> = free.iter().copied().filter(|&c| imports_from(imports, lang, caller_module, name, &linker[c].module)).collect();
        if let Some(target) = narrow(imported) {
            return target;
        }
        // Brought from a repo module that re-exports it.
        let through: Vec<usize> = linker.through_reexport(linker.scope_of(caller), lang, name).into_iter().filter(|&c| !self.is_member(c)).collect();
        narrow(through).unwrap_or(CallTarget::Unproven(free))
    }
}
