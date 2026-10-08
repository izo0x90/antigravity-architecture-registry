//! Language-generic code ingestion. A language is a pack of ast-grep rules
//! (data); the engine extracts definitions, scopes, calls, imports and
//! placeholders per file, then resolves symbols, calls and typed data flow
//! across files into the observed architecture graph. Code rules (also data)
//! check components for forbidden or required patterns.

mod expr;
mod extract;
pub mod highlight;
pub mod index;
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
pub use resolve::{AmbiguousCall, Combined, ExternalCall, Occurrence, Resolved, UnverifiedFlow, combine};
pub use rules::{CodeRule, CodeRuleSpec, CodeRules, RuleHit, RuleKind, RuleSeverity};
pub use walk::{SourceFile, ignored, module_symbol, walk};

/// Bumped whenever what extraction reads from a file changes (not the
/// packs: their fingerprint is kept with each file). Facts kept from an
/// older reader are read again.
pub const EXTRACTOR_VERSION: u32 = 7;

/// Bumped whenever what the combine stage makes of the facts changes: a
/// combined result kept from an older one is made again.
pub const COMBINE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IndexError {
    #[error("language pack: {0}")]
    Pack(String),
    #[error("io: {0}")]
    Io(String),
    #[error("format: {0}")]
    Format(String),
}

/// The languages' own indexers that may run, and what's installed.
#[derive(Clone, Copy)]
pub struct Indexers<'a> {
    pub catalog: &'a nah_packs::Catalog,
    pub lock: &'a nah_packs::Lock,
}

/// A whole project, analyzed.
#[derive(Debug)]
pub struct Analysis {
    pub extracts: Vec<FileExtract>,
    /// What the languages' indexers said, per file.
    pub indexed: Vec<index::IndexedFile>,
    /// Indexers that couldn't run, and why (the language is matched by name).
    pub skipped: Vec<(String, String)>,
    pub resolved: Resolved,
}

/// Every stage over `scope` (inside `root`): each file read; each
/// language's own indexer run when `indexers` is given and its pack is
/// ready; everything combined. The server runs the same stages, keeping
/// what it can between runs.
pub fn analyze(root: &Path, scope: &Path, packs: &PackSet, rules: &CodeRules, types: &TypeSystem, indexers: Option<Indexers<'_>>) -> Result<Analysis, IndexError> {
    let mut extracts = Vec::new();
    for file in walk(root, scope, packs)? {
        let Some(pack) = packs.for_path(&file.absolute) else { continue };
        let content = std::fs::read_to_string(&file.absolute).map_err(|e| IndexError::Io(format!("{}: {e}", file.path)))?;
        extracts.push(extract_file(pack, rules, file.path, file.module, &content));
    }
    let mut indexed = Vec::new();
    let mut skipped = Vec::new();
    if let Some(Indexers { catalog, lock }) = indexers {
        for pack in &packs.packs {
            let Some(indexer) = &pack.indexer else { continue };
            let hashes: std::collections::BTreeMap<_, _> = extracts.iter().filter(|e| e.language == pack.name()).map(|e| (e.file.clone(), e.content_hash.clone())).collect();
            if hashes.is_empty() {
                continue;
            }
            match index::run(catalog, lock, indexer, root, &hashes) {
                Ok(run) => indexed.extend(run.files),
                Err(e) => skipped.push((pack.name().to_owned(), e.to_string())),
            }
        }
    }
    let resolved = combine(&extracts, &indexed, packs, types);
    Ok(Analysis { extracts, indexed, skipped, resolved })
}
