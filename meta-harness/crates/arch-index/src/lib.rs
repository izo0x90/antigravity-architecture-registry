//! Language-generic code ingestion. A language is a pack of ast-grep rules
//! (data); the engine extracts definitions, scopes, calls, imports and
//! placeholders per file, then resolves symbols and calls across files into
//! the observed architecture graph.

mod extract;
mod pack;
mod resolve;
mod walk;

use std::path::Path;

use arch_graph::TypeSystem;

pub use extract::{FileExtract, RawCall, RawDef, RawScope, content_hash, extract_file};
pub use pack::{CallSpec, DefinitionSpec, LanguageSpec, Pack, PackSet, PackSpec, ParamSpec, RuleSpec, ScopeSpec};
pub use resolve::{AmbiguousCall, ExternalCall, Resolved, resolve};
pub use walk::{SourceFile, module_symbol, walk};

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
pub fn ingest(root: &Path, scope: &Path, packs: &PackSet, types: &TypeSystem) -> Result<(Vec<FileExtract>, Resolved), IndexError> {
    let mut extracts = Vec::new();
    for file in walk(root, scope, packs)? {
        let Some(pack) = packs.by_name(&file.language) else { continue };
        let content = std::fs::read_to_string(&file.absolute).map_err(|e| IndexError::Io(format!("{}: {e}", file.path)))?;
        extracts.push(extract_file(pack, file.path, file.module, &content));
    }
    let resolved = resolve(&extracts, packs, types);
    Ok((extracts, resolved))
}
