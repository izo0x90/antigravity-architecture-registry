//! Per-file extraction: run a pack's rules over one file and record raw
//! definitions, scopes, calls, imports and placeholders. Cached by content
//! hash; symbols and call targets are resolved later, across all files.

use arch_graph::{Param, RepoPath, SCHEMA_VERSION, Signature, Span, check_schema_version};
use ast_grep_core::Node;
use ast_grep_core::tree_sitter::StrDoc;
use ast_grep_language::{LanguageExt, SupportLang};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{IndexError, Pack};

type AstNode<'r> = Node<'r, StrDoc<SupportLang>>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawScope {
    /// Text naming the component the scope's definitions belong to.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawDef {
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub syntax_kind: String,
    pub span: Span,
    /// Index of the enclosing definition in this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<usize>,
    /// The scope it sits directly in (e.g. an impl block), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<RawScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<String>,
    /// The name its owner value goes by inside it (a Go receiver), if the
    /// language names it per definition rather than with a fixed `self`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawCall {
    /// Index of the innermost enclosing definition (None: module level).
    pub caller: Option<usize>,
    /// The callee expression as written.
    pub callee: String,
    pub span: Span,
    /// Arguments produced by another call (directly or through a local name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<RawArg>,
    /// Inside a closure/lambda (a deferred scope): it may never run.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deferred: bool,
    /// The receiver is a local bound to this call's output (`let s = Store::new(); s.save()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_producer: Option<String>,
    /// The receiver is a local declared with this type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_type: Option<String>,
    /// Not a call: a function named as a value (it may be called later). It
    /// only counts when the name is a known function.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reference: bool,
}

/// How data leaves or enters a definition, besides being passed into a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    /// A call's result is what the definition returns.
    Returned,
    /// A call's result is bound to local names (destructured: several).
    Bound,
    /// A value of a type is made.
    Constructs,
    /// A field of the definition's own value is written.
    Writes,
    /// A field of the definition's own value is read.
    Reads,
    /// Data is sent across a boundary (an event, a request).
    Sends,
    /// Data is received from a boundary.
    Receives,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawFlow {
    pub kind: FlowKind,
    /// The definition it's in (None: module level).
    pub at: Option<usize>,
    /// Returned/Bound: the producing call's callee; Constructs: the type as
    /// written; Writes/Reads: the field; Sends/Receives: the key.
    pub what: String,
    /// Bound: the names; Receives: the handlers named (route handlers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// Sends/Receives: the boundary's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boundary: Option<String>,
    pub span: Span,
}

/// A macro (or similar) defined in a file: the names its expansion uses, and
/// whether it is a placeholder itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub placeholder: bool,
}

/// An argument whose value is the output of another call in the same body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawArg {
    /// Position among the call's arguments (0 = first).
    pub position: u32,
    /// The producing call's callee expression as written.
    pub producer: String,
    pub producer_span: Span,
}

/// Everything one file contributes, before cross-file resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileExtract {
    pub schema_version: u32,
    pub file: RepoPath,
    pub language: String,
    pub content_hash: String,
    /// The pack version it was extracted with (see `Pack::fingerprint`).
    #[serde(default)]
    pub pack_fingerprint: String,
    /// The file's module symbol (from its path and package).
    pub module: String,
    /// The syntax kind of the file's root node.
    #[serde(default)]
    pub root_kind: String,
    pub lines: u32,
    pub bytes: u32,
    pub definitions: Vec<RawDef>,
    pub calls: Vec<RawCall>,
    pub imports: Vec<String>,
    /// Local names for imported things: (alias, what it stands for).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<(String, String)>,
    pub placeholders: Vec<Span>,
    /// Macro uses by name (last path segment): which are placeholders is
    /// decided across the repo at resolution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macro_uses: Vec<(String, Span)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macro_defs: Vec<MacroDef>,
    /// Returns, bindings, construction, state and boundaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<RawFlow>,
    /// Where the code rules matched (on the same parse).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rule_hits: Vec<crate::RuleHit>,
    /// The code rules it was read with (see `CodeRules::fingerprint`).
    #[serde(default)]
    pub rules_fingerprint: String,
}

impl FileExtract {
    pub fn from_json(text: &str) -> Result<Self, IndexError> {
        #[derive(Deserialize)]
        struct Header {
            schema_version: u32,
        }
        let header: Header = serde_json::from_str(text).map_err(|e| IndexError::Format(e.to_string()))?;
        check_schema_version(header.schema_version, "index entry").map_err(|e| IndexError::Format(e.to_string()))?;
        serde_json::from_str(text).map_err(|e| IndexError::Format(e.to_string()))
    }

    pub fn to_json(&self) -> Result<String, IndexError> {
        serde_json::to_string_pretty(self).map_err(|e| IndexError::Format(e.to_string()))
    }
}

pub fn content_hash(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn span(node: &AstNode<'_>) -> Span {
    let range = node.range();
    Span {
        start_line: node.start_pos().line() as u32 + 1,
        end_line: node.end_pos().line() as u32 + 1,
        start_byte: range.start as u32,
        end_byte: range.end as u32,
    }
}

fn field_text(node: &AstNode<'_>, field: &str) -> Option<String> {
    node.field(field).map(|n| n.text().trim().to_owned())
}

/// A type's text, unwrapping annotation nodes (e.g. `: string`) to the type.
fn type_text(pack: &Pack, node: &AstNode<'_>, field: &str) -> Option<String> {
    let mut ty = node.field(field)?;
    while pack.spec.language.type_wrappers.iter().any(|w| *w == ty.kind()) {
        let inner = ty.named_children().next();
        ty = inner?;
    }
    Some(ty.text().trim().to_owned())
}

/// The callee and span when `node` is a call (or wraps exactly one).
fn as_call(pack: &Pack, node: &AstNode<'_>) -> Option<(String, Span)> {
    let direct = |n: &AstNode<'_>| {
        pack.calls.iter().zip(&pack.spec.calls).find_map(|(rule, spec)| n.matches(rule).then(|| field_text(n, &spec.callee_field)).flatten()).map(|c| (c, span(n)))
    };
    direct(node).or_else(|| {
        let mut named = node.named_children();
        match (named.next(), named.next()) {
            (Some(only), None) => direct(&only),
            _ => None,
        }
    })
}

/// The call that produces `node`'s value: the call itself, through calls
/// that just wrap their argument (`Ok(f(x))`), or a local bound to a call.
fn producer(pack: &Pack, node: &AstNode<'_>, bound: &std::collections::HashMap<(Option<usize>, String), Bound>, at: Option<usize>) -> Option<(String, Span)> {
    let lang = &pack.spec.language;
    fn call<'r, 'p>(pack: &'p Pack, n: &AstNode<'r>) -> Option<(AstNode<'r>, &'p crate::pack::CallSpec)> {
        pack.calls.iter().zip(&pack.spec.calls).find_map(|(rule, spec)| n.matches(rule).then_some(spec)).map(|spec| (n.clone(), spec))
    }
    let found = call(pack, node).or_else(|| {
        let mut named = node.named_children();
        match (named.next(), named.next()) {
            (Some(only), None) => call(pack, &only),
            _ => None,
        }
    });
    if let Some((n, spec)) = found {
        let callee = field_text(&n, &spec.callee_field)?;
        let last = lang.split_last(&callee).map_or(callee.as_str(), |(_, _, name)| name);
        if lang.transparent_calls.iter().any(|t| *t == callee || *t == last) {
            let arg = spec.args_field.as_ref().and_then(|f| n.field(f)).and_then(|list| list.named_children().find(|c| !lang.ignored(&c.kind())))?;
            return producer(pack, &arg, bound, at);
        }
        return Some((callee, span(&n)));
    }
    local_name(pack, &node.text()).and_then(|name| bound.get(&(at, name.to_owned())).and_then(|b| b.producer.clone()))
}

/// The names a binding's pattern binds: one local, or each name in a
/// destructuring pattern.
fn pattern_names(pack: &Pack, pattern: &AstNode<'_>) -> Vec<String> {
    if let Some(name) = local_name(pack, &pattern.text()) {
        return vec![name.to_owned()];
    }
    // `self.x = ...` sets a field, not locals.
    if pack.state.as_ref().is_some_and(|s| pattern.matches(&s.access)) {
        return Vec::new();
    }
    let kinds = &pack.spec.language.pattern_name_kinds;
    pattern.dfs().filter(|n| kinds.iter().any(|k| *k == n.kind())).filter_map(|n| local_name(pack, &n.text()).map(str::to_owned)).collect()
}

/// A plain local name as written in an argument (`&out`, `mut x` -> name).
fn local_name<'a>(pack: &Pack, text: &'a str) -> Option<&'a str> {
    let name = pack.spec.language.strip_references(text);
    (!name.is_empty() && name.chars().all(|c| pack.spec.language.is_identifier_char(c))).then_some(name)
}

/// Whether `name`, called or named as a value inside definition `at`, is
/// one of its locals (bound in it, or a parameter) rather than a function:
/// `let helper = || ..; helper()` or `fn run(callback: impl Fn()) { callback() }`.
/// A definition of that name nested right there is a function after all.
fn is_local(pack: &Pack, name: &str, at: Option<usize>, defs: &[RawDef], bound: &std::collections::HashMap<(Option<usize>, String), Bound>) -> bool {
    let Some(at) = at else { return false };
    if local_name(pack, name) != Some(name) || defs.iter().any(|d| d.parent == Some(at) && d.name == name) {
        return false;
    }
    let param = defs.get(at).and_then(|d| d.signature.as_ref()).is_some_and(|s| s.params.iter().any(|p| local_name(pack, &p.name) == Some(name)));
    param || bound.contains_key(&(Some(at), name.to_owned()))
}

/// Whether `inner` sits directly in `def` (no other definition in between).
fn directly_in(pack: &Pack, inner: &AstNode<'_>, def: &AstNode<'_>) -> bool {
    for ancestor in inner.ancestors() {
        if ancestor.node_id() == def.node_id() {
            return true;
        }
        if pack.definitions.iter().any(|rule| ancestor.matches(rule)) {
            return false;
        }
    }
    false
}
fn signature(pack: &Pack, def: &crate::pack::DefinitionSpec, node: &AstNode<'_>) -> Option<Signature> {
    // Parameters and return type may live on a child (`const f = (x) => ...`).
    let holder = match &def.signature_field {
        Some(field) => node.field(field)?,
        None => node.clone(),
    };
    let node = &holder;
    let params_node = def.params_field.as_ref().and_then(|f| node.field(f));
    let returns = def.return_field.as_ref().and_then(|f| type_text(pack, node, f));
    if params_node.is_none() && returns.is_none() {
        return None;
    }
    let mut params = Vec::new();
    if let Some(list) = params_node {
        for child in list.children() {
            let kind = child.kind();
            let Some(spec) = pack.spec.params.iter().find(|p| p.kind == kind) else { continue };
            let name = match (&spec.name_field, &spec.name_child_kind) {
                (Some(field), _) => field_text(&child, field),
                (None, Some(kind)) => child.children().find(|c| c.kind() == kind.as_str()).map(|c| c.text().trim().to_owned()),
                (None, None) => Some(child.text().trim().to_owned()),
            };
            let Some(name) = name else { continue };
            let ty = spec.type_field.as_ref().and_then(|f| type_text(pack, &child, f));
            let optional = spec.optional || spec.default_field.as_ref().is_some_and(|f| child.field(f).is_some());
            params.push(Param { name, ty, optional, variadic: spec.variadic });
        }
    }
    Some(Signature { params, returns, qualifiers: qualifiers(pack, node) })
}

/// Qualifier tokens on a definition (directly, or inside a container child).
fn qualifiers(pack: &Pack, node: &AstNode<'_>) -> Vec<String> {
    let lang = &pack.spec.language;
    let mut out = Vec::new();
    for child in node.children() {
        let kind = child.kind();
        // As written (an ABI string is part of `extern "C"`), whitespace collapsed.
        let written = |n: &AstNode<'_>| n.text().split_whitespace().collect::<Vec<_>>().join(" ");
        if lang.qualifier_tokens.iter().any(|t| *t == kind) {
            out.push(written(&child));
        } else if lang.qualifier_containers.iter().any(|c| *c == kind) {
            out.extend(child.children().filter(|g| lang.qualifier_tokens.iter().any(|t| *t == g.kind())).map(|g| written(&g)));
        }
    }
    out
}

/// What a local was last bound to in its definition.
#[derive(Clone, Default)]
struct Bound {
    producer: Option<(String, Span)>,
    ty: Option<String>,
}

#[derive(Clone)]
struct Ctx {
    parent: Option<usize>,
    scope: Option<RawScope>,
    /// Inside a closure or similar (until the next definition).
    deferred: bool,
    /// Inside code that never runs.
    dead: bool,
    /// The node that is the enclosing definition's own body (an arrow
    /// function bound to a name): not a deferred scope.
    own_body: Option<usize>,
}

/// Whether `node` never runs: it matches a dead-code rule whose condition,
/// if it has one, is constant false.
fn dead(pack: &Pack, node: &AstNode<'_>) -> bool {
    pack.dead_code.iter().zip(&pack.spec.dead_code).any(|((rule, preceding, capture), spec)| {
        if !node.matches(rule) {
            return false;
        }
        let Some(condition) = &spec.condition else { return true };
        let text = match (&condition.parent_field, preceding) {
            (Some(field), _) => node.parent().and_then(|p| p.field(field)).map(|c| c.text().to_string()),
            (None, Some(rule)) => node.prev_all().find(|n| n.is_named()).filter(|n| n.matches(rule)).map(|n| n.text().to_string()),
            (None, None) => None,
        };
        let text = match (text, capture) {
            (Some(t), Some(re)) => re.captures(&t).and_then(|c| c.get(1)).map(|g| g.as_str().to_owned()),
            (t, None) => t,
            (None, _) => None,
        };
        text.is_some_and(|t| pack.spec.constant_logic.eval(&t) == Some(false))
    })
}

/// The flows a node makes: a returned call, a value made, a field of the
/// definition's own value written or read, data sent or received across a
/// boundary.
fn flows(pack: &Pack, node: &AstNode<'_>, ctx: &Ctx, bound: &std::collections::HashMap<(Option<usize>, String), Bound>, defs: &[RawDef], out: &mut Vec<RawFlow>) {
    let lang = &pack.spec.language;
    let mut push = |kind, what: String, names: Vec<String>, boundary: Option<String>| out.push(RawFlow { kind, at: ctx.parent, what, names, boundary, span: span(node) });
    if pack.returns.iter().any(|rule| node.matches(rule))
        && let Some((callee, _)) = producer(pack, node, bound, ctx.parent)
    {
        push(FlowKind::Returned, callee, Vec::new(), None);
    }
    for (rule, spec) in pack.constructions.iter().zip(&pack.spec.constructions) {
        if node.matches(rule)
            && let Some(ty) = field_text(node, &spec.type_field)
        {
            push(FlowKind::Constructs, ty, Vec::new(), None);
        }
    }
    if let (Some(state), Some(spec)) = (&pack.state, &pack.spec.state)
        && node.matches(&state.access)
        && let (Some(object), Some(field)) = (field_text(node, &spec.object_field), field_text(node, &spec.field_field))
    {
        let own = ctx.parent.and_then(|p| defs.get(p)).and_then(|d| d.self_name.clone());
        if lang.self_names.contains(&object) || own.as_deref() == Some(object.as_str()) {
            let written = node.parent().is_some_and(|p| state.assignments.iter().any(|a| p.matches(a)) && p.field(&spec.left_field).is_some_and(|l| l.node_id() == node.node_id()));
            push(if written { FlowKind::Writes } else { FlowKind::Reads }, field, Vec::new(), None);
        }
    }
    for (compiled, spec) in pack.boundaries.iter().zip(&pack.spec.boundaries) {
        if node.matches(&compiled.send) {
            let text = node.text();
            if let Some(key) = compiled.send_key.captures(&text).and_then(|c| c.get(1)) {
                push(FlowKind::Sends, compiled.key(key.as_str()), Vec::new(), Some(spec.name.clone()));
            }
        }
        if node.matches(&compiled.receive) {
            let keyed = spec.receive_field.as_ref().and_then(|f| node.field(f)).map_or_else(|| node.text().to_string(), |f| f.text().to_string());
            if let Some(key) = compiled.receive_key.captures(&keyed).and_then(|c| c.get(1)) {
                // Handlers from the same part as the key (a chain of
                // `.route(..)` calls holds every route's handlers).
                let handlers = compiled.receive_handlers.as_ref().map(|re| re.captures_iter(&keyed).filter_map(|c| c.get(1).map(|g| g.as_str().to_owned())).collect()).unwrap_or_default();
                push(FlowKind::Receives, compiled.key(key.as_str()), handlers, Some(spec.name.clone()));
            }
        }
    }
}

/// Extracts one file, parsing it once: definitions, calls, flows,
/// placeholders and the code rules' matches all come from that one parse.
/// `module` is its module symbol (see `walk::module_symbol`).
pub fn extract_file(pack: &Pack, rules: &crate::CodeRules, file: RepoPath, module: String, content: &str) -> FileExtract {
    let root = pack.lang.ast_grep(content);
    let mut out = FileExtract {
        schema_version: SCHEMA_VERSION,
        file,
        language: pack.name().to_owned(),
        content_hash: content_hash(content),
        pack_fingerprint: pack.fingerprint.clone(),
        module,
        root_kind: root.root().kind().into_owned(),
        lines: content.lines().count().max(1) as u32,
        bytes: content.len() as u32,
        definitions: Vec::new(),
        calls: Vec::new(),
        imports: Vec::new(),
        aliases: Vec::new(),
        placeholders: Vec::new(),
        macro_uses: Vec::new(),
        macro_defs: Vec::new(),
        flows: Vec::new(),
        rule_hits: Vec::new(),
        rules_fingerprint: rules.fingerprint(pack.name()).to_owned(),
    };
    let lang = &pack.spec.language;
    // Local names bound to a call's output (or a declared type), per enclosing definition.
    let mut bound: std::collections::HashMap<(Option<usize>, String), Bound> = std::collections::HashMap::new();
    let mut stack: Vec<(AstNode<'_>, Ctx)> = vec![(root.root(), Ctx { parent: None, scope: None, deferred: false, dead: false, own_body: None })];
    while let Some((node, mut ctx)) = stack.pop() {
        if !ctx.dead && dead(pack, &node) {
            ctx.dead = true;
        }
        if lang.deferred_scopes.iter().any(|k| *k == node.kind()) && ctx.own_body != Some(node.node_id()) {
            ctx.deferred = true;
        }
        let mut child_ctx = ctx.clone();
        if let Some((di, def)) = pack.definitions.iter().zip(&pack.spec.definitions).enumerate().find_map(|(i, (rule, def))| node.matches(rule).then_some((i, def))) {
            if let Some(name) = field_text(&node, &def.name_field) {
                let mut bases: Vec<String> = def
                    .bases_field
                    .as_ref()
                    .and_then(|f| node.field(f))
                    .map(|b| b.children().filter(|c| c.is_named() && !lang.ignored(&c.kind())).map(|c| c.text().trim().to_owned()).collect())
                    .unwrap_or_default();
                if let Some(rule) = &pack.bases[di] {
                    bases.extend(node.find_all(rule).map(|m| m.get_node().clone()).filter(|m| directly_in(pack, m, &node)).map(|m| m.text().trim().to_owned()));
                }
                // Declared outside its type (e.g. a method with a receiver).
                let owner = match (&def.owner_field, &def.owner_kind) {
                    (Some(field), Some(kind)) => node.field(field).and_then(|f| f.dfs().find(|n| n.kind() == kind.as_str())).map(|n| n.text().trim().to_owned()),
                    _ => None,
                };
                let self_name = match (&def.owner_field, &def.receiver_name_kind) {
                    (Some(field), Some(kind)) => node.field(field).and_then(|f| f.dfs().find(|n| n.kind() == kind.as_str())).map(|n| n.text().trim().to_owned()),
                    _ => None,
                };
                let scope = match owner {
                    Some(target) => Some(RawScope { target, implements: None }),
                    None => ctx.scope.clone(),
                };
                out.definitions.push(RawDef {
                    name,
                    type_name: def.type_name.clone(),
                    syntax_kind: node.kind().into_owned(),
                    span: span(&node),
                    parent: ctx.parent,
                    scope,
                    signature: signature(pack, def, &node),
                    bases,
                    self_name,
                });
                // A definition is its own caller: not deferred by what encloses it.
                let own_body = def.signature_field.as_ref().and_then(|f| node.field(f)).map(|b| b.node_id());
                child_ctx = Ctx { parent: Some(out.definitions.len() - 1), scope: None, deferred: false, dead: ctx.dead, own_body };
            }
        } else if let Some(scope) = pack.scopes.iter().zip(&pack.spec.scopes).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(target) = field_text(&node, &scope.target_field)
        {
            let implements = scope.implements_field.as_ref().and_then(|f| field_text(&node, f));
            child_ctx = Ctx { scope: Some(RawScope { target, implements }), ..ctx.clone() };
        }
        if !ctx.dead
            && let Some(binding) = pack.bindings.iter().zip(&pack.spec.bindings).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(pattern) = node.field(&binding.name_field)
        {
            let names = pattern_names(pack, &pattern);
            let value = node.field(&binding.value_field);
            let produced = value.as_ref().and_then(|v| producer(pack, v, &bound, ctx.parent));
            let entry = Bound { producer: produced.clone(), ty: binding.type_field.as_ref().and_then(|f| type_text(pack, &node, f)) };
            for name in &names {
                bound.insert((ctx.parent, name.clone()), entry.clone());
            }
            // A call's result kept in locals flows into this definition.
            if let Some((callee, _)) = produced
                && !names.is_empty()
            {
                out.flows.push(RawFlow { kind: FlowKind::Bound, at: ctx.parent, what: callee, names, boundary: None, span: span(&node) });
            }
        }
        if !ctx.dead {
            flows(pack, &node, &ctx, &bound, &out.definitions, &mut out.flows);
        }
        if !ctx.dead
            && let Some(call) = pack.calls.iter().zip(&pack.spec.calls).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(callee) = field_text(&node, &call.callee_field)
            && !is_local(pack, &callee, ctx.parent, &out.definitions, &bound)
        {
            let mut args = Vec::new();
            if let Some(list) = call.args_field.as_ref().and_then(|f| node.field(f)) {
                for (position, arg) in list.named_children().filter(|c| !lang.ignored(&c.kind())).enumerate() {
                    let producer =
                        as_call(pack, &arg).or_else(|| local_name(pack, &arg.text()).and_then(|n| bound.get(&(ctx.parent, n.to_owned())).and_then(|b| b.producer.clone())));
                    if let Some((producer, producer_span)) = producer {
                        args.push(RawArg { position: position as u32, producer, producer_span });
                    }
                }
            }
            // A receiver that is a local: what it was bound to.
            let receiver = lang.split_last(&callee).and_then(|(qualifier, _, _)| local_name(pack, qualifier)).and_then(|n| bound.get(&(ctx.parent, n.to_owned())));
            out.calls.push(RawCall {
                caller: ctx.parent,
                callee,
                span: span(&node),
                args,
                deferred: ctx.deferred,
                receiver_producer: receiver.and_then(|b| b.producer.as_ref().map(|(p, _)| p.clone())),
                receiver_type: receiver.and_then(|b| b.ty.clone()),
                reference: false,
            });
        }
        if !ctx.dead && pack.references.iter().any(|rule| node.matches(rule)) && !is_local(pack, node.text().trim(), ctx.parent, &out.definitions, &bound) {
            out.calls.push(RawCall {
                caller: ctx.parent,
                callee: node.text().trim().to_owned(),
                span: span(&node),
                args: Vec::new(),
                deferred: true,
                receiver_producer: None,
                receiver_type: None,
                reference: true,
            });
        }
        // Children in reverse so the stack visits them in source order.
        let children: Vec<_> = node.children().collect();
        for child in children.into_iter().rev() {
            stack.push((child, child_ctx.clone()));
        }
    }
    let root_node = root.root();
    for rule in &pack.imports {
        out.imports.extend(root_node.find_all(rule).map(|m| m.get_node().text().trim().to_owned()));
    }
    for (rule, spec) in pack.aliases.iter().zip(&pack.spec.aliases) {
        for m in root_node.find_all(rule) {
            let node = m.get_node();
            let alias = match (&spec.alias_field, &spec.alias_child_kind) {
                (Some(field), _) => field_text(node, field),
                (None, Some(kind)) => node.children().find(|c| c.kind() == kind.as_str()).map(|c| c.text().trim().to_owned()),
                (None, None) => None,
            };
            let holder = match &spec.original_ancestor {
                Some(kind) => node.ancestors().find(|a| a.kind() == kind.as_str()),
                None => Some(node.clone()),
            };
            if let (Some(alias), Some(original)) = (alias, holder.and_then(|h| field_text(&h, &spec.original_field))) {
                out.aliases.push((alias, original));
            }
        }
    }
    for rule in &pack.placeholders {
        out.placeholders.extend(root_node.find_all(rule).map(|m| span(m.get_node())));
    }
    if let (Some(macros), Some(spec)) = (&pack.macros, &pack.spec.macros) {
        let last = |name: &str| lang.split_last(name).map_or(name, |(_, _, n)| n).to_owned();
        for m in root_node.find_all(&macros.invocation) {
            if let Some(name) = field_text(m.get_node(), &spec.name_field) {
                out.macro_uses.push((last(&name), span(m.get_node())));
            }
        }
        for m in root_node.find_all(&macros.definition) {
            let node = m.get_node();
            let Some(name) = field_text(node, &spec.definition_name_field) else { continue };
            let text = node.text();
            let mut uses: Vec<String> = macros.uses.captures_iter(&text).filter_map(|c| c.get(1).map(|g| last(g.as_str()))).filter(|u| *u != name).collect();
            uses.sort();
            uses.dedup();
            let placeholder = macros.placeholder.as_ref().is_some_and(|r| r.is_match(&text));
            out.macro_defs.push(MacroDef { name, uses, placeholder });
        }
    }
    out.rule_hits = rules.hits(pack.name(), &root_node);
    out.placeholders.sort_by_key(|s| s.start_byte);
    out.placeholders.dedup();
    // A field read many times in one definition is one flow.
    let mut seen = std::collections::BTreeSet::new();
    out.flows.retain(|f| !matches!(f.kind, FlowKind::Reads | FlowKind::Writes) || seen.insert((f.kind, f.at, f.what.clone())));
    out
}
