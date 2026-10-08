//! Language-generic code ingestion. A language is a pack of ast-grep rules
//! (data); the engine extracts definitions, scopes, calls, imports and
//! placeholders per file, then resolves symbols, calls and typed data flow
//! across files into the observed architecture graph. Code rules (also data)
//! check components for forbidden or required patterns.

mod expr;
mod extract;
mod pack;
mod resolve;
mod rules;
pub mod typing;
mod walk;

use std::path::Path;

use arch_graph::TypeSystem;

pub use expr::Expr;
pub use extract::{FileExtract, FlowKind, MacroDef, RawArg, RawCall, RawDef, RawFlow, RawScope, content_hash, extract_file};
pub use pack::{AliasSpec, BindingSpec, CallSpec, DefinitionSpec, ExpressionShape, ExpressionSpec, LanguageSpec, Pack, PackSet, PackSpec, ParamSpec, MacroSpec, DeadCodeSpec, ConditionSpec, ConstantLogic, RuleSpec, ScopeSpec};
pub use resolve::{AmbiguousCall, ExternalCall, Resolved, UnverifiedFlow, resolve};
pub use rules::{CodeRule, CodeRuleSpec, CodeRules, RuleHit, RuleKind, RuleSeverity};
pub use walk::{SourceFile, ignored, module_symbol, walk};

/// Bumped whenever what extraction reads from a file changes (not the
/// packs: their fingerprint is kept with each file). Facts kept from an
/// older reader are read again.
pub const EXTRACTOR_VERSION: u32 = 5;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IndexError {
    #[error("language pack: {0}")]
    Pack(String),
    #[error("io: {0}")]
    Io(String),
    #[error("format: {0}")]
    Format(String),
}

/// Ingests `scope` (inside `root`) without a cache: walk, extract, resolve.
pub fn ingest(root: &Path, scope: &Path, packs: &PackSet, rules: &CodeRules, types: &TypeSystem) -> Result<(Vec<FileExtract>, Resolved), IndexError> {
    let mut extracts = Vec::new();
    for file in walk(root, scope, packs)? {
        let Some(pack) = packs.for_path(&file.absolute) else { continue };
        let content = std::fs::read_to_string(&file.absolute).map_err(|e| IndexError::Io(format!("{}: {e}", file.path)))?;
        extracts.push(extract_file(pack, rules, file.path, file.module, &content));
    }
    let resolved = resolve(&extracts, packs, types);
    Ok((extracts, resolved))
}
