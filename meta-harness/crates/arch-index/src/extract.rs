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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawCall {
    /// Index of the innermost enclosing definition (None: module level).
    pub caller: Option<usize>,
    /// The callee expression as written.
    pub callee: String,
    pub span: Span,
}

/// Everything one file contributes, before cross-file resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileExtract {
    pub schema_version: u32,
    pub file: RepoPath,
    pub language: String,
    pub content_hash: String,
    /// The file's module symbol (from its path and package).
    pub module: String,
    pub lines: u32,
    pub bytes: u32,
    pub definitions: Vec<RawDef>,
    pub calls: Vec<RawCall>,
    pub imports: Vec<String>,
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

fn signature(pack: &Pack, def: &crate::pack::DefinitionSpec, node: &AstNode<'_>) -> Option<Signature> {
    let params_node = def.params_field.as_ref().and_then(|f| node.field(f));
    let returns = def.return_field.as_ref().and_then(|f| field_text(node, f));
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
            let ty = spec.type_field.as_ref().and_then(|f| field_text(&child, f));
            params.push(Param { name, ty });
        }
    }
    Some(Signature { params, returns })
}

#[derive(Clone)]
struct Ctx {
    parent: Option<usize>,
    scope: Option<RawScope>,
}

/// Extracts one file. `module` is its module symbol (see `walk::module_symbol`).
pub fn extract_file(pack: &Pack, file: RepoPath, module: String, content: &str) -> FileExtract {
    let root = pack.lang.ast_grep(content);
    let mut out = FileExtract {
        schema_version: SCHEMA_VERSION,
        file,
        language: pack.name().to_owned(),
        content_hash: content_hash(content),
        module,
        lines: content.lines().count().max(1) as u32,
        bytes: content.len() as u32,
        definitions: Vec::new(),
        calls: Vec::new(),
        imports: Vec::new(),
        placeholders: Vec::new(),
    };
    let mut stack: Vec<(AstNode<'_>, Ctx)> = vec![(root.root(), Ctx { parent: None, scope: None })];
    while let Some((node, ctx)) = stack.pop() {
        let mut child_ctx = ctx.clone();
        if let Some(def) = pack.definitions.iter().zip(&pack.spec.definitions).find_map(|(rule, def)| node.matches(rule).then_some(def)) {
            if let Some(name) = field_text(&node, &def.name_field) {
                let bases = def
                    .bases_field
                    .as_ref()
                    .and_then(|f| node.field(f))
                    .map(|b| b.children().filter(|c| c.is_named() && !c.kind().contains("comment")).map(|c| c.text().trim().to_owned()).collect())
                    .unwrap_or_default();
                out.definitions.push(RawDef {
                    name,
                    type_name: def.type_name.clone(),
                    syntax_kind: node.kind().into_owned(),
                    span: span(&node),
                    parent: ctx.parent,
                    scope: ctx.scope.clone(),
                    signature: signature(pack, def, &node),
                    bases,
                });
                child_ctx = Ctx { parent: Some(out.definitions.len() - 1), scope: None };
            }
        } else if let Some(scope) = pack.scopes.iter().zip(&pack.spec.scopes).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(target) = field_text(&node, &scope.target_field)
        {
            let implements = scope.implements_field.as_ref().and_then(|f| field_text(&node, f));
            child_ctx = Ctx { parent: ctx.parent, scope: Some(RawScope { target, implements }) };
        }
        if let Some(call) = pack.calls.iter().zip(&pack.spec.calls).find_map(|(rule, spec)| node.matches(rule).then_some(spec))
            && let Some(callee) = field_text(&node, &call.callee_field)
        {
            out.calls.push(RawCall { caller: ctx.parent, callee, span: span(&node) });
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
    for rule in &pack.placeholders {
        out.placeholders.extend(root_node.find_all(rule).map(|m| span(m.get_node())));
    }
    out.placeholders.sort_by_key(|s| s.start_byte);
    out
}
