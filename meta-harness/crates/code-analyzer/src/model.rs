//! Every file's rows in memory, and what a change does to them:
//! the changed files are linked again
//! from their new reading; so are the files with refs whose name a changed
//! file defines (now or before) or whose target was in one; nothing else is
//! touched. What changed comes back per file and table, to be written.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use arch_graph::{ComponentId, RepoPath, TypeSystem};

use crate::link::{Linker, Said, Saids};
use crate::rows::{ComponentRow, FileRow, FileRows, FlowRow, PlaceholderRow, RefRow, RelationRow};
use crate::{FileExtract, PackSet, RuleHit};

/// What one change writes for one file: each table's rows when they changed.
#[derive(Clone, Debug, PartialEq)]
pub struct FileWrite {
    pub path: RepoPath,
    pub file: Option<FileRow>,
    pub components: Option<Vec<ComponentRow>>,
    pub refs: Option<Vec<RefRow>>,
    pub relations: Option<Vec<RelationRow>>,
    pub flows: Option<Vec<FlowRow>>,
    pub placeholders: Option<Vec<PlaceholderRow>>,
    pub rule_hits: Option<Vec<RuleHit>>,
    pub highlights: Option<crate::highlight::Highlights>,
}

impl FileWrite {
    pub fn new(path: RepoPath) -> Self {
        Self {
            path,
            file: None,
            components: None,
            refs: None,
            relations: None,
            flows: None,
            placeholders: None,
            rule_hits: None,
            highlights: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.file.is_none()
            && self.components.is_none()
            && self.refs.is_none()
            && self.relations.is_none()
            && self.flows.is_none()
            && self.placeholders.is_none()
            && self.rule_hits.is_none()
            && self.highlights.is_none()
    }
}

/// A change: files read (new, or their content changed), files touched
/// (same content, new stamp), files gone.
#[derive(Debug, Default)]
pub struct Change {
    pub read: Vec<(FileRow, FileExtract)>,
    pub touched: Vec<FileRow>,
    pub removed: Vec<RepoPath>,
}

/// What a change wrote.
#[derive(Debug, Default)]
pub struct Written {
    pub writes: Vec<FileWrite>,
    pub removed: Vec<RepoPath>,
    /// Files linked again (the changed ones and those that use them).
    pub linked: BTreeSet<RepoPath>,
    /// Files only touched (a new stamp, the same content).
    pub touched: BTreeSet<RepoPath>,
}

impl Written {
    /// Whether anything but stamps changed.
    pub fn graph_changed(&self) -> bool {
        !self.removed.is_empty() || self.writes.iter().any(|w| !self.touched.contains(&w.path))
    }
}

/// Every file's rows, persistent: a snapshot shares it whole, copying
/// nothing, and a change makes a new version sharing what it didn't change.
pub type Files = imbl::OrdMap<RepoPath, Arc<FileRows>>;

/// Which files have refs with each name, and to each target (what a change
/// can affect, found without looking at every file).
#[derive(Default)]
struct Uses {
    by_name: HashMap<String, HashSet<RepoPath>>,
    by_target: HashMap<ComponentId, HashSet<RepoPath>>,
    /// By the name of the type a call went through (its receiver's).
    by_via: HashMap<String, HashSet<RepoPath>>,
}

/// What other files can see change when a file goes from `old` to `new`.
struct Seen {
    /// Names whose definitions changed (or every name the file has).
    names: BTreeSet<String>,
    /// The types those definitions belong to (`impl Trait for T`'s `T`).
    scoped: BTreeSet<String>,
}

/// Compares what linking other files reads of a file: its definitions
/// (names, kinds, nesting, scopes, bases, signatures, the types they name),
/// imports, aliases, items, macros and boundary keys; never where any of it
/// is. A change only inside bodies changes none of it.
fn seen_change(old: Option<&FileExtract>, new: Option<&FileExtract>) -> Seen {
    let texts = |named: &[crate::Named]| named.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join(",");
    let shared = |ex: &FileExtract| {
        let boundaries: Vec<String> = ex
            .flows
            .iter()
            .filter(|f| f.boundary.is_some())
            .map(|f| format!("{:?}|{:?}|{}|{:?}", f.kind, f.boundary, f.what, f.at.map(|i| &ex.definitions[i].name)))
            .collect();
        format!("{}\n{}\n{:?}\n{:?}\n{:?}", ex.module, texts(&ex.imports), ex.aliases, ex.macro_defs, boundaries)
    };
    let defs = |ex: &FileExtract| -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for d in &ex.definitions {
            let scope = d
                .scope
                .as_ref()
                .map(|s| format!("{}:{}", s.target.text, s.implements.as_ref().map_or("", |i| i.text.as_str())))
                .unwrap_or_default();
            let parent = d.parent.map(|i| ex.definitions[i].name.as_str());
            out.entry(d.name.clone()).or_default().push(format!(
                "{}|{}|{parent:?}|{scope}|{:?}|{}|{:?}|{}",
                d.type_name,
                d.syntax_kind,
                d.signature,
                texts(&d.bases),
                d.self_name,
                texts(&d.type_names)
            ));
        }
        for lines in out.values_mut() {
            lines.sort();
        }
        out
    };
    let scopes = |ex: &FileExtract, only: Option<&BTreeSet<String>>| -> Vec<String> {
        ex.definitions
            .iter()
            .filter(|d| only.is_none_or(|n| n.contains(&d.name)))
            .filter_map(|d| d.scope.as_ref())
            .map(|s| s.target.text.clone())
            .collect()
    };
    let everything = |exs: &[&FileExtract]| Seen {
        names: exs.iter().flat_map(|ex| ex.definitions.iter().map(|d| d.name.clone()).chain(ex.items.iter().cloned())).collect(),
        scoped: exs.iter().flat_map(|ex| scopes(ex, None)).collect(),
    };
    match (old, new) {
        (Some(old), Some(new)) if shared(old) == shared(new) => {
            let (before, after) = (defs(old), defs(new));
            let mut names: BTreeSet<String> =
                before.keys().chain(after.keys()).filter(|n| before.get(*n) != after.get(*n)).cloned().collect();
            let items_before: BTreeSet<&String> = old.items.iter().collect();
            let items_after: BTreeSet<&String> = new.items.iter().collect();
            names.extend(items_before.symmetric_difference(&items_after).map(|n| (*n).clone()));
            let scoped = scopes(old, Some(&names)).into_iter().chain(scopes(new, Some(&names))).collect();
            Seen { names, scoped }
        }
        (old, new) => everything(&old.into_iter().chain(new).collect::<Vec<_>>()),
    }
}

impl Uses {
    fn add(&mut self, rows: &FileRows) {
        for r in &rows.refs {
            self.by_name.entry(r.name.clone()).or_default().insert(rows.file.path.clone());
            if let Some(t) = &r.target {
                self.by_target.entry(t.clone()).or_default().insert(rows.file.path.clone());
            }
            for v in &r.detail.via {
                self.by_via.entry(v.clone()).or_default().insert(rows.file.path.clone());
            }
        }
    }

    fn remove(&mut self, rows: &FileRows) {
        let path = &rows.file.path;
        for r in &rows.refs {
            for v in &r.detail.via {
                if let Some(files) = self.by_via.get_mut(v) {
                    files.remove(path);
                    if files.is_empty() {
                        self.by_via.remove(v);
                    }
                }
            }
            if let Some(files) = self.by_name.get_mut(&r.name) {
                files.remove(path);
                if files.is_empty() {
                    self.by_name.remove(&r.name);
                }
            }
            if let Some(t) = &r.target
                && let Some(files) = self.by_target.get_mut(t)
            {
                files.remove(path);
                if files.is_empty() {
                    self.by_target.remove(t);
                }
            }
        }
    }
}

/// What a server said a call is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerAnswer {
    /// A component in the repo.
    At(ComponentId),
    /// Something outside the repo (a library's).
    Outside,
    /// A place in the repo that's no component (a local, a parameter, or
    /// anything in a file that isn't read).
    Local,
    /// No definition.
    Nothing,
}

/// What a server said of a file's names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answers {
    /// Each call's, by its place among the file's calls.
    pub calls: BTreeMap<usize, ServerAnswer>,
    /// Each type named and field used, by where its name starts; a field's
    /// with whether the place reads or writes it.
    pub at: BTreeMap<u32, (ServerAnswer, Option<crate::rows::Access>)>,
}

#[derive(Default)]
pub struct Model {
    /// Shared with snapshots: a file's rows are replaced, never changed.
    files: Files,
    uses: Uses,
    /// From before a full re-read: what servers said of each file's calls,
    /// by its content then, to keep (unconfirmed) where it's still the same.
    kept_answers: BTreeMap<RepoPath, (String, Saids)>,
    linker: Linker,
    /// The graph, kept up to date with the rows.
    keeper: crate::observed::Keeper,
}

impl Model {
    /// The rows as stored (nothing is linked).
    pub fn load(packs: &PackSet, types: &TypeSystem, rows: Vec<FileRows>) -> Self {
        let linker = Linker::load(packs, types, rows.iter());
        let mut model = Self { linker, ..Self::default() };
        for r in rows {
            model.put(Arc::new(r));
        }
        let all: BTreeSet<RepoPath> = model.files.keys().cloned().collect();
        model.keep(&all, packs);
        model
    }

    pub fn files(&self) -> impl Iterator<Item = &FileRows> + Clone {
        self.files.iter().map(|(_, f)| &**f)
    }

    pub fn get(&self, path: &RepoPath) -> Option<&FileRows> {
        self.files.get(path).map(|f| &**f)
    }

    /// The graph and what couldn't be linked, as the rows are now.
    pub fn keeper(&self) -> &crate::observed::Keeper {
        &self.keeper
    }

    /// Brings the graph up to date with `changed` files.
    fn keep(&mut self, changed: &BTreeSet<RepoPath>, packs: &PackSet) {
        let files = &self.files;
        self.keeper.update(files.iter().map(|(_, f)| &**f), |p| files.get(p).map(|f| &**f), changed, packs);
    }

    /// Starts again from no rows (a full re-read), keeping what the
    /// servers said, unconfirmed, for files that turn out the same.
    pub fn reset_keeping_answers(&mut self) {
        let kept: BTreeMap<RepoPath, (String, Saids)> = self
            .files
            .iter()
            .map(|(path, rows)| {
                let mut said = saids_of(rows);
                for s in said.all_mut() {
                    s.confirmed = false;
                }
                (path.clone(), (rows.file.content_hash.clone(), said))
            })
            .filter(|(_, (_, said))| !said.is_empty())
            .collect();
        *self = Model { kept_answers: kept, ..Model::default() };
    }

    /// Every file's rows, shared (for a snapshot: nothing is copied).
    pub fn shared(&self) -> Files {
        self.files.clone()
    }

    /// Keeps a file's rows (replacing what it had).
    fn put(&mut self, rows: Arc<FileRows>) {
        if let Some(old) = self.files.get(&rows.file.path) {
            self.uses.remove(old);
        }
        self.uses.add(&rows);
        self.files.insert(rows.file.path.clone(), rows);
    }

    /// Drops a file's rows.
    fn take(&mut self, path: &RepoPath) -> Option<Arc<FileRows>> {
        let old = self.files.remove(path)?;
        self.uses.remove(&old);
        Some(old)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Applies `change`: stamps, then every file it can affect linked again.
    pub fn apply(&mut self, packs: &PackSet, types: &TypeSystem, change: Change) -> Written {
        let mut written = Written::default();
        for file in change.touched {
            if let Some(rows) = self.files.get_mut(&file.path)
                && rows.file != file
            {
                Arc::make_mut(rows).file = file.clone();
                written.touched.insert(file.path.clone());
                written.writes.push(FileWrite { file: Some(file.clone()), ..FileWrite::new(file.path.clone()) });
            }
        }
        let changed: BTreeSet<RepoPath> = change.read.iter().map(|(f, _)| f.path.clone()).chain(change.removed.iter().cloned()).collect();
        if changed.is_empty() {
            return written;
        }
        // What other files can see change: the names whose definitions
        // changed (all of a file's when its imports, macros or boundaries
        // changed, or it's new or gone), and the types those belong to.
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut scoped: BTreeSet<String> = BTreeSet::new();
        // Every link into a changed file is asked about again (RB.10).
        let mut ids: BTreeSet<ComponentId> = BTreeSet::new();
        // Names link within one language: only its files can use them.
        let mut languages: BTreeSet<String> = BTreeSet::new();
        let read: HashMap<&RepoPath, &FileExtract> = change.read.iter().map(|(f, ex)| (&f.path, ex)).collect();
        for path in &changed {
            let before = self.files.get(path);
            if let Some(rows) = before {
                languages.insert(rows.file.language.clone());
                ids.extend(rows.components.iter().map(|c| c.id.clone()));
            }
            let after = read.get(path).copied();
            if let Some(ex) = after {
                languages.insert(ex.language.clone());
            }
            let old = before.map(|rows| rows.reading());
            let seen = seen_change(old.as_ref(), after);
            names.extend(seen.names);
            // The type each scope is written for, by its name (`Holder` for
            // `Holder<T>`), as the language writes types.
            let language = after.map(|ex| ex.language.as_str()).or(before.map(|rows| rows.file.language.as_str()));
            if let Some(lang) = language.and_then(|l| packs.by_name(l)).map(|p| &p.spec.language) {
                scoped.extend(seen.scoped.iter().filter_map(|t| lang.words(&crate::link::clean(t, lang)).last().map(|w| (*w).to_owned())));
            }
            if old.is_none()
                && let Some(ex) = after
                && let Some(pack) = packs.by_name(&ex.language)
            {
                // A new file is a new module: paths through its name.
                names.insert(ex.module.rsplit(pack.spec.language.separator.as_str()).next().unwrap_or(&ex.module).to_owned());
            }
        }
        // The files that use them.
        let same_language = |p: &&RepoPath| self.files.get(*p).is_some_and(|rows| languages.contains(&rows.file.language));
        let by_name = names.iter().filter_map(|n| self.uses.by_name.get(n)).flatten().filter(same_language);
        let by_target = ids.iter().filter_map(|t| self.uses.by_target.get(t)).flatten();
        // Calls that went through a type whose definition, impls or members
        // changed.
        let type_names: BTreeSet<&str> = names.iter().map(String::as_str).chain(scoped.iter().map(String::as_str)).collect();
        let by_via = type_names.iter().filter_map(|n| self.uses.by_via.get(*n)).flatten().filter(same_language);
        let users: BTreeSet<RepoPath> = by_name.chain(by_target).chain(by_via).filter(|p| !changed.contains(*p)).cloned().collect();
        for path in &change.removed {
            if self.take(path).is_some() {
                self.linker.forget(path);
                written.removed.push(path.clone());
            }
        }
        // Files read again: what servers said of them no longer holds, unless
        // it's a full re-read and the file is the same (then it's kept,
        // unconfirmed, until asked again). Files linked again from their rows
        // keep it, unconfirmed where it named something in a changed file.
        let mut to_link: Vec<(FileRow, FileExtract, Saids)> = change
            .read
            .into_iter()
            .map(|(f, e)| {
                let said =
                    self.kept_answers.remove(&f.path).filter(|(hash, _)| *hash == f.content_hash).map(|(_, s)| s).unwrap_or_default();
                (f, e, said)
            })
            .collect();
        for path in &users {
            let rows = &self.files[path];
            let mut said = saids_of(rows);
            // A call's name, by its place among the calls.
            let names_of: BTreeMap<usize, &str> =
                rows.refs.iter().filter_map(|r| r.detail.call.as_ref().map(|(ci, _)| (*ci, r.name.as_str()))).collect();
            for (ci, s) in said.calls.iter_mut() {
                // Into a changed file, or nowhere while a changed file now
                // defines (or did define) its name: asked again.
                let into_changed = s.target.as_ref().is_some_and(|t| ids.contains(t));
                let named_now = s.target.is_none() && names_of.get(ci).is_some_and(|n| names.contains(*n));
                if into_changed || named_now {
                    s.confirmed = false;
                }
            }
            // A type or field's answer into a changed file: asked again.
            for s in said.at.values_mut().filter(|s| s.target.as_ref().is_some_and(|t| ids.contains(t))) {
                s.confirmed = false;
            }
            to_link.push((rows.file.clone(), rows.reading(), said));
        }
        written.linked = to_link.iter().map(|(f, _, _)| f.path.clone()).collect();
        for rows in self.linker.link(packs, types, to_link) {
            let path = rows.file.path.clone();
            let write = diff(&path, self.get(&path), &rows);
            if !write.is_empty() {
                written.writes.push(write);
            }
            self.put(Arc::new(rows));
        }
        let changed: BTreeSet<RepoPath> = written
            .writes
            .iter()
            .filter(|w| !written.touched.contains(&w.path))
            .map(|w| w.path.clone())
            .chain(written.removed.iter().cloned())
            .collect();
        if !changed.is_empty() {
            self.keep(&changed, packs);
        }
        written
    }

    /// Every file linked again from its rows (the types or packs changed).
    pub fn relink_all(&mut self, packs: &PackSet, types: &TypeSystem) -> Written {
        let all: Vec<(FileRow, FileExtract, Saids)> = self.files.values().map(|r| (r.file.clone(), r.reading(), saids_of(r))).collect();
        self.linker = Linker::default();
        let mut written = Written { linked: all.iter().map(|(f, _, _)| f.path.clone()).collect(), ..Written::default() };
        for rows in self.linker.link(packs, types, all) {
            let path = rows.file.path.clone();
            let write = diff(&path, self.get(&path), &rows);
            if !write.is_empty() {
                written.writes.push(write);
            }
            self.put(Arc::new(rows));
        }
        self.keeper = Default::default();
        let all: BTreeSet<RepoPath> = self.files.keys().cloned().collect();
        self.keep(&all, packs);
        written
    }
}

impl Model {
    /// What `prover` (a server and its version) said of the names in
    /// `path`, as it was when its content hashed to `content_hash`: each
    /// call's, type's and field's component (None: outside the repo). The
    /// file is linked again with it; a file changed since is left for a
    /// newer answer.
    pub fn confirm(
        &mut self,
        packs: &PackSet,
        types: &TypeSystem,
        path: &RepoPath,
        content_hash: &str,
        answers: &Answers,
        prover: &str,
    ) -> Written {
        let mut written = Written::default();
        let Some(rows) = self.files.get(path).filter(|r| r.file.content_hash == content_hash) else { return written };
        let mut said = saids_of(rows);
        let heard = |answer: &ServerAnswer, access| {
            let (target, outside, local) = match answer {
                ServerAnswer::At(id) => (Some(id.clone()), false, false),
                ServerAnswer::Outside => (None, true, false),
                ServerAnswer::Local => (None, false, true),
                ServerAnswer::Nothing => (None, false, false),
            };
            Said { target, outside, local, prover: prover.to_owned(), confirmed: true, access }
        };
        for (ci, answer) in &answers.calls {
            said.calls.insert(*ci, heard(answer, None));
        }
        for (start, (answer, access)) in &answers.at {
            said.at.insert(*start, heard(answer, *access));
        }
        let before = settled_types(rows);
        let to_link = vec![(rows.file.clone(), rows.reading(), said)];
        written.linked = BTreeSet::from([path.clone()]);
        for rows in self.linker.link(packs, types, to_link) {
            let write = diff(path, self.get(path), &rows);
            // Unchanged rows stay the same rows (nothing new to ask about).
            if !write.is_empty() {
                written.writes.push(write);
                self.put(Arc::new(rows));
            }
        }
        // Which types this file's signatures name is part of what flows
        // through its functions carry: when the server settled one, the
        // files with data flows through them are linked again (with what
        // the servers said of them, kept).
        if let Some(rows) = self.files.get(path).cloned()
            && settled_types(&rows) != before
        {
            let ids: BTreeSet<&ComponentId> = rows.components.iter().map(|c| &c.id).collect();
            let dependents: Vec<RepoPath> = self
                .files
                .iter()
                .filter(|(p, _)| *p != path)
                .filter(|(_, r)| {
                    r.relations.iter().any(|rel| {
                        rel.kind == arch_graph::RelationKind::DataFlow && (ids.contains(&rel.source) || ids.contains(&rel.target))
                    })
                })
                .map(|(p, _)| p.clone())
                .collect();
            if !dependents.is_empty() {
                let to_link: Vec<_> =
                    dependents.iter().filter_map(|p| self.files.get(p)).map(|r| (r.file.clone(), r.reading(), saids_of(r))).collect();
                for linked in self.linker.link(packs, types, to_link) {
                    let at = linked.file.path.clone();
                    let write = diff(&at, self.get(&at), &linked);
                    if !write.is_empty() {
                        written.writes.push(write);
                        written.linked.insert(at);
                        self.put(Arc::new(linked));
                    }
                }
            }
        }
        if !written.writes.is_empty() {
            self.keep(&written.linked.clone(), packs);
        }
        written
    }
}

/// Which of a file's type refs are settled (the server confirmed them, or
/// they're no repo type), by where they start.
fn settled_types(rows: &FileRows) -> Vec<(u32, bool)> {
    use crate::rows::{Evidence, RefKind};
    rows.refs.iter().filter(|r| r.kind == RefKind::Type).map(|r| (r.start, r.evidence == Evidence::Server || r.target.is_none())).collect()
}

/// What servers said of a file's names, as its refs keep it.
fn saids_of(rows: &FileRows) -> Saids {
    use crate::rows::{Evidence, RefKind};
    let mut out = Saids::default();
    for r in &rows.refs {
        let Some(prover) = r.prover.clone() else { continue };
        let confirmed = matches!(r.evidence, Evidence::Server | Evidence::External | Evidence::NotFound | Evidence::Local);
        let said = Said {
            target: r.target.clone(),
            outside: r.evidence == Evidence::External,
            local: r.evidence == Evidence::Local,
            prover,
            confirmed,
            access: r.detail.server_access,
        };
        match (&r.detail.call, r.kind) {
            (Some((ci, _)), _) => {
                out.calls.insert(*ci, said);
            }
            (None, RefKind::Type | RefKind::Field | RefKind::Import | RefKind::Path) => {
                out.at.insert(r.start, said);
            }
            _ => {}
        }
    }
    out
}

/// The tables whose rows for `path` differ between `old` and `new`.
fn diff(path: &RepoPath, old: Option<&FileRows>, new: &FileRows) -> FileWrite {
    fn changed<T: PartialEq + Clone>(old: Option<&T>, new: &T) -> Option<T> {
        (old != Some(new)).then(|| new.clone())
    }
    FileWrite {
        path: path.clone(),
        file: changed(old.map(|o| &o.file), &new.file),
        components: changed(old.map(|o| &o.components), &new.components),
        refs: changed(old.map(|o| &o.refs), &new.refs),
        relations: changed(old.map(|o| &o.relations), &new.relations),
        flows: changed(old.map(|o| &o.flows), &new.flows),
        placeholders: changed(old.map(|o| &o.placeholders), &new.placeholders),
        rule_hits: changed(old.map(|o| &o.rule_hits), &new.rule_hits),
        highlights: changed(old.map(|o| &o.highlights), &new.highlights),
    }
}
