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
    pub lines: u32,
    pub bytes: u32,
    pub definitions: Vec<RawDef>,
    pub calls: Vec<RawCall>,
    pub imports: Vec<String>,
    /// Local names for imported things: (alias, what it stands for).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<(String, String)>,
    pub placeholders: Vec<Span>,
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

fn span(node: &AstNode<'_>) -> Span {
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

/// A plain local name as written in an argument (`&out`, `mut x` -> name).
fn local_name<'a>(pack: &Pack, text: &'a str) -> Option<&'a str> {
    let name = pack.spec.language.strip_references(text);
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
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
            params.push(Param { name, ty, optional });
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
        if lang.qualifier_tokens.iter().any(|t| *t == kind) {
            out.push(kind.into_owned());
        } else if lang.qualifier_containers.iter().any(|c| *c == kind) {
            out.extend(child.children().map(|g| g.kind().into_owned()).filter(|k| lang.qualifier_tokens.contains(k)));
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
}

/// Extracts one file. `module` is its module symbol (see `walk::module_symbol`).
pub fn extract_file(pack: &Pack, file: RepoPath, module: String, content: &str) -> FileExtract {
    let root = pack.lang.ast_grep(content);
    let mut out = FileExtract {
        schema_version: SCHEMA_VERSION,
        file,
        language: pack.name().to_owned(),
        content_hash: content_hash(content),
        pack_fingerprint: pack.fingerprint.clone(),
        module,
        lines: content.lines().count().max(1) as u32,
        bytes: content.len() as u32,
        definitions: Vec::new(),
        calls: Vec::new(),
        imports: Vec::new(),
        aliases: Vec::new(),
        placeholders: Vec::new(),
    };
    let lang = &pack.spec.language;
    // Local names bound to a call's output (or a declared type), per enclosing definition.
    let mut bound: std::collections::HashMap<(Option<usize>, String), Bound> = std::collections::HashMap::new();
    let mut stack: Vec<(AstNode<'_>, Ctx)> = vec![(root.root(), Ctx { parent: None, scope: None, deferred: false, dead: false })];
    while let Some((node, mut ctx)) = stack.pop() {
        if !ctx.dead && pack.dead_code.iter().any(|rule| node.matches(rule)) {
            ctx.dead = true;
        }
        if lang.deferred_scopes.iter().any(|k| *k == node.kind()) {
            ctx.deferred = true;
        }
        let mut child_ctx = ctx.clone();
        if let Some((di, def)) = pack.definitions.iter().zip(&pack.spec.definitions).enumerate().find_map(|(i, (rule, def))| node.matches(rule).then_some((i, def))) {
            if let Some(name) = field_text(&node, &def.name_field) {
                let mut bases: Vec<String> = def
                    .bases_field
                    .as_ref()
                    .and_then(|f| node.field(f))
                    .map(|b| b.children().filter(|c| c.is_named() && !c.kind().contains("comment")).map(|c| c.text().trim().to_owned()).collect())
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
                child_ctx = Ctx { parent: Some(out.definitions.len() - 1), scope: None, deferred: false, dead: ctx.dead };
            }
        } else if let Some(scope) = pack.scopes.iter().zip(&pack.spec.scopes).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(target) = field_text(&node, &scope.target_field)
        {
            let implements = scope.implements_field.as_ref().and_then(|f| field_text(&node, f));
            child_ctx = Ctx { scope: Some(RawScope { target, implements }), ..ctx.clone() };
        }
        if !ctx.dead
            && let Some(binding) = pack.bindings.iter().zip(&pack.spec.bindings).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(name) = field_text(&node, &binding.name_field)
            && let Some(name) = local_name(pack, &name)
        {
            let value = node.field(&binding.value_field);
            let entry = Bound {
                producer: value.as_ref().and_then(|v| as_call(pack, v)),
                ty: binding.type_field.as_ref().and_then(|f| type_text(pack, &node, f)),
            };
            bound.insert((ctx.parent, name.to_owned()), entry);
        }
        if !ctx.dead
            && let Some(call) = pack.calls.iter().zip(&pack.spec.calls).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(callee) = field_text(&node, &call.callee_field)
        {
            let mut args = Vec::new();
            if let Some(list) = call.args_field.as_ref().and_then(|f| node.field(f)) {
                for (position, arg) in list.named_children().filter(|c| !c.kind().contains("comment")).enumerate() {
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
    // Uses of local definitions that expand to a placeholder.
    for ((definition, invocation), spec) in pack.placeholder_aliases.iter().zip(&pack.spec.placeholder_aliases) {
        let names: Vec<String> = root_node.find_all(definition).filter_map(|m| field_text(m.get_node(), &spec.name_field)).collect();
        if names.is_empty() {
            continue;
        }
        for m in root_node.find_all(invocation) {
            let used = field_text(m.get_node(), &spec.invocation_name_field).unwrap_or_default();
            let last = lang.split_last(&used).map_or(used.as_str(), |(_, _, name)| name);
            if names.iter().any(|n| n == last) {
                out.placeholders.push(span(m.get_node()));
            }
        }
    }
    out.placeholders.sort_by_key(|s| s.start_byte);
    out.placeholders.dedup();
    out
}
