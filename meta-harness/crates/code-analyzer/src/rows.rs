//! A file's rows:
//! what reading it gave, and what linking made of it. Every row belongs to
//! one file. A file's rows hold its whole reading, so a change elsewhere
//! re-links it without reading it again.

use arch_graph::{ComponentId, RelationKind, RepoPath, Signature, Span};
use serde::{Deserialize, Serialize};

use crate::highlight::Highlights;
use crate::{Expr, FileExtract, FlowKind, MacroDef, Named, RawCall, RawDef, RawFlow, RawScope, RuleHit};

/// A source file: its stamp, its content's hash, and what read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRow {
    pub path: RepoPath,
    pub language: String,
    pub size: u64,
    /// Modification time (ns since the epoch); 0 when too recent to trust.
    pub modified: i64,
    pub content_hash: String,
    /// The reader, pack and code rules it was read with: another means
    /// it's read again.
    pub read_version: String,
}

/// What reading a file depends on: the reader, its pack and the code rules.
/// A file read under another is read again.
pub fn read_version(pack_fingerprint: &str, rules_fingerprint: &str) -> String {
    format!("{}:{pack_fingerprint}:{rules_fingerprint}", crate::EXTRACTOR_VERSION)
}

/// The code rules a file was read with (see `read_version`).
pub fn rules_read_with(read_version: &str) -> &str {
    read_version.rsplit(':').next().unwrap_or_default()
}

/// A module or a definition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ComponentRow {
    pub id: ComponentId,
    /// The component it's in (its owner, or its file's module); None for a
    /// module.
    pub parent: Option<ComponentId>,
    pub type_name: String,
    pub name: String,
    pub span: Span,
    pub name_span: Option<Span>,
    pub signature: Option<Signature>,
    pub detail: ComponentDetail,
}

/// What a component row keeps beyond the plan's columns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ComponentDetail {
    pub symbol: String,
    pub syntax_kind: String,
    /// A definition's place in its file's reading; None for the module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub def: Option<DefPlace>,
    /// The module's: what the file imports and defines as macros.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<ModuleReading>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DefPlace {
    /// Its index among the file's definitions, and its parent's.
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RawScope>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<Named>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_names: Vec<Named>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModuleReading {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<Named>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macro_defs: Vec<MacroDef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
    /// Calls with no name (`make()()`), which have no ref to keep them, by
    /// their place among the file's calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unnamed_calls: Vec<(usize, RawCall)>,
}

/// What a name used in the code is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    Call,
    /// A function named as a value (it may be called later).
    Value,
    /// A type named: a scope's target or interface, a base, a value made, a
    /// parameter's, a return's, a local's.
    Type,
    /// A name an import brings in (each name in it, and the path's).
    Import,
    /// A name a call's path goes through (`util` in `util::shout()`).
    Path,
    /// A handler named where a boundary is received (a route).
    Handler,
    /// A field of the definition's own value, read or written (its target:
    /// the type the field is in).
    Field,
}

/// How a place uses a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    Write,
}

impl RefKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::Value => "value",
            Self::Type => "type",
            Self::Handler => "handler",
            Self::Import => "import",
            Self::Path => "path",
            Self::Field => "field",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Call, Self::Value, Self::Type, Self::Handler, Self::Import, Self::Path, Self::Field].into_iter().find(|k| k.name() == name)
    }
}

/// How a ref was linked, or why it wasn't (RB.12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Matched by name (unconfirmed).
    Name,
    /// The language's server said so.
    Server,
    /// Proven to mean one of several (the candidates are kept).
    Ambiguous,
    /// Can't be proven (a receiver of unknown type, a name neither local
    /// nor imported); the components sharing the name are kept.
    Unproven,
    /// Code outside the repo.
    External,
    /// Nothing in the repo by that name.
    NotFound,
    /// A member of a repo type that isn't a component of its own: an enum's
    /// variant, a constant, a method a derive or a trait gives it.
    Member,
    /// The server put it in the repo, at something that isn't a component
    /// (a local binding, a parameter, a type's generic, anything in a file
    /// that isn't read).
    Local,
}

impl Evidence {
    pub fn name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Server => "server",
            Self::Ambiguous => "ambiguous",
            Self::Unproven => "unproven",
            Self::External => "external",
            Self::NotFound => "not_found",
            Self::Member => "member",
            Self::Local => "local",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Name, Self::Server, Self::Ambiguous, Self::Unproven, Self::External, Self::NotFound, Self::Member, Self::Local]
            .into_iter()
            .find(|e| e.name() == name)
    }
}

/// A name used in the code, where (its name's bytes), and what it is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RefRow {
    /// Unique in the file (the table's key): the name's first byte.
    pub start: u32,
    pub end: u32,
    pub name: String,
    pub kind: RefKind,
    /// The component it's used in.
    pub within: Option<ComponentId>,
    pub target: Option<ComponentId>,
    pub evidence: Evidence,
    /// The server that confirmed it, and its version.
    pub prover: Option<String>,
    pub detail: RefDetail,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RefDetail {
    /// A call as read, and its place among the file's calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<(usize, RawCall)>,
    /// What an ambiguous or unproven ref could mean.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ComponentId>,
    /// Data handed from another call into this one that types couldn't
    /// confirm: (from, argument position, why).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<(ComponentId, u32, String)>,
    /// A field: read or written (the server's word when it gave one, else
    /// the reader's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<Access>,
    /// A field: read or written, as the server said (None: it didn't).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_access: Option<Access>,
    /// A call: the names of the repo types it was resolved through (its
    /// receiver's and that type's bases, as searched), whether or not one
    /// had the member.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub via: Vec<String>,
}

/// A link the file's code makes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationRow {
    pub kind: RelationKind,
    pub source: ComponentId,
    pub target: ComponentId,
    pub detail: RelationDetail,
    pub evidence: Evidence,
    pub prover: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RelationDetail {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
}

/// Data leaving or entering a definition, as read; `within` and
/// `receivers` as linked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FlowRow {
    /// Its place among the file's flows.
    pub index: usize,
    pub flow: RawFlow,
    pub within: Option<ComponentId>,
    /// Receives: who receives (the definition, or the handlers named).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receivers: Vec<ComponentId>,
}

/// A placeholder, or a macro use that is one when the macro is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaceholderRow {
    pub span: Span,
    pub macro_name: Option<String>,
}

/// Every row of one file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileRows {
    pub file: FileRow,
    pub components: Vec<ComponentRow>,
    pub refs: Vec<RefRow>,
    pub relations: Vec<RelationRow>,
    pub flows: Vec<FlowRow>,
    pub placeholders: Vec<PlaceholderRow>,
    pub rule_hits: Vec<RuleHit>,
    pub highlights: Highlights,
}

impl FileRows {
    /// The module row.
    pub fn module(&self) -> Option<&ComponentRow> {
        self.components.iter().find(|c| c.detail.def.is_none())
    }

    /// The file's reading, as it was read, from its rows.
    pub fn reading(&self) -> FileExtract {
        let module = self.module();
        let reading = module.and_then(|m| m.detail.module.clone()).unwrap_or_default();
        let mut defs: Vec<(usize, RawDef)> = self
            .components
            .iter()
            .filter_map(|c| {
                let place = c.detail.def.as_ref()?;
                Some((
                    place.index,
                    RawDef {
                        name: c.name.clone(),
                        type_name: c.type_name.clone(),
                        syntax_kind: c.detail.syntax_kind.clone(),
                        span: c.span,
                        name_span: c.name_span,
                        parent: place.parent,
                        scope: place.scope.clone(),
                        signature: c.signature.clone(),
                        bases: place.bases.clone(),
                        self_name: place.self_name.clone(),
                        type_names: place.type_names.clone(),
                    },
                ))
            })
            .collect();
        defs.sort_by_key(|(i, _)| *i);
        let mut calls: Vec<(usize, RawCall)> =
            self.refs.iter().filter_map(|r| r.detail.call.clone()).chain(reading.unnamed_calls.iter().cloned()).collect();
        calls.sort_by_key(|(i, _)| *i);
        let mut flows: Vec<&FlowRow> = self.flows.iter().collect();
        flows.sort_by_key(|f| f.index);
        FileExtract {
            schema_version: arch_graph::SCHEMA_VERSION,
            file: self.file.path.clone(),
            language: self.file.language.clone(),
            content_hash: self.file.content_hash.clone(),
            pack_fingerprint: String::new(),
            module: module.map(|m| m.detail.symbol.clone()).unwrap_or_default(),
            root_kind: module.map(|m| m.detail.syntax_kind.clone()).unwrap_or_default(),
            lines: module.map_or(0, |m| m.span.end_line),
            bytes: module.map_or(0, |m| m.span.end_byte),
            definitions: defs.into_iter().map(|(_, d)| d).collect(),
            calls: calls.into_iter().map(|(_, c)| c).collect(),
            imports: reading.imports,
            aliases: reading.aliases,
            placeholders: self.placeholders.iter().filter(|p| p.macro_name.is_none()).map(|p| p.span).collect(),
            macro_uses: self.placeholders.iter().filter_map(|p| p.macro_name.clone().map(|n| (n, p.span))).collect(),
            macro_defs: reading.macro_defs,
            items: reading.items,
            flows: flows.into_iter().map(|f| f.flow.clone()).collect(),
            rule_hits: self.rule_hits.clone(),
            rules_fingerprint: String::new(),
            highlights: self.highlights.clone(),
        }
    }
}

/// What reading `file` gave, as rows not linked yet: the read parts only
/// (ids, targets and relations come from linking).
pub fn read_rows(file: FileRow, extract: &FileExtract) -> FileRows {
    FileRows {
        file,
        components: Vec::new(),
        refs: Vec::new(),
        relations: Vec::new(),
        flows: extract
            .flows
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, flow)| FlowRow { index, flow, within: None, receivers: Vec::new() })
            .collect(),
        placeholders: extract
            .placeholders
            .iter()
            .map(|span| PlaceholderRow { span: *span, macro_name: None })
            .chain(extract.macro_uses.iter().map(|(name, span)| PlaceholderRow { span: *span, macro_name: Some(name.clone()) }))
            .collect(),
        rule_hits: extract.rule_hits.clone(),
        highlights: extract.highlights.clone(),
    }
}

/// The read flows a ref of kind `Type`, `Handler` or `Field` may come from.
pub(crate) fn flow_refs(flow: &RawFlow) -> Vec<(RefKind, &str, Span)> {
    match flow.kind {
        FlowKind::Constructs => flow.what_span.map(|s| vec![(RefKind::Type, flow.what.as_str(), s)]).unwrap_or_default(),
        FlowKind::Reads | FlowKind::Writes => flow.what_span.map(|s| vec![(RefKind::Field, flow.what.as_str(), s)]).unwrap_or_default(),
        FlowKind::Receives => flow.names.iter().zip(&flow.name_spans).map(|(n, s)| (RefKind::Handler, n.as_str(), *s)).collect(),
        _ => Vec::new(),
    }
}

/// A call's ref name: the last name in what's called.
pub(crate) fn call_name(callee: &Expr) -> String {
    callee
        .split()
        .map(|(_, _, name)| name.to_owned())
        .or_else(|| match callee {
            Expr::Name(n) => Some(n.clone()),
            _ => None,
        })
        .unwrap_or_else(|| callee.render())
}
