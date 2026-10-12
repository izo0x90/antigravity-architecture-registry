//! Language-generic code ingestion. A language is a pack of ast-grep rules
//! (data); the engine extracts definitions, scopes, calls, imports and
//! placeholders per file, then resolves symbols, calls and typed data flow
//! across files into the observed architecture graph. Code rules (also data)
//! check components for forbidden or required patterns.

mod expr;
mod extract;
pub mod highlight;
mod pack;
mod link;
mod model;
mod observed;
pub mod rows;
mod rules;
pub mod typing;
mod walk;

pub use expr::Expr;
pub use extract::{FileExtract, FlowKind, MacroDef, Named, RawArg, RawCall, RawDef, RawFlow, RawScope, content_hash, extract_file};
pub use pack::{AliasSpec, BindingSpec, CallSpec, DefinitionSpec, ExpressionShape, ExpressionSpec, LanguageSpec, Pack, PackSet, PackSpec, ParamSpec, MacroSpec, DeadCodeSpec, ConditionSpec, ConstantLogic, RuleSpec, ScopeSpec};
pub use arch_graph::code::Occurrence;
pub use link::{AmbiguousCall, ExternalCall, Said, Saids, UnverifiedFlow};
pub use model::{Answers, Change, FileWrite, Files, Model, ServerAnswer, Written};
pub use observed::{Keeper, Observed, entry_points, occurrences};
#[cfg(any(test, feature = "testing"))]
pub use observed::observed;
pub use rules::{CodeRule, CodeRuleSpec, CodeRules, RuleHit, RuleKind, RuleSeverity};
pub use walk::{Ignores, PROJECT_IGNORE, SourceFile, module_symbol, walk, walk_with};

/// Bumped whenever what extraction reads from a file changes (not the
/// packs: their fingerprint is kept with each file). Facts kept from an
/// older reader are read again.
pub const EXTRACTOR_VERSION: u32 = 12;

/// Bumped whenever what linking makes of a file's reading changes: every
/// file is linked again from its rows.
pub const LINKER_VERSION: u32 = 12;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IndexError {
    #[error("language pack: {0}")]
    Pack(String),
    #[error("io: {0}")]
    Io(String),
}

/// The language packs shipped in this repo, without any project's or
/// user's own (tests must not depend on the machine).
#[cfg(test)]
pub(crate) fn shipped_packs() -> PackSet {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
    let places = nah_packs::Places::isolated(&std::env::temp_dir().join(format!("nah-test-packs-{}", std::process::id())));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&dir).unwrap(), &places).unwrap();
    PackSet::from_catalog(&catalog).unwrap()
}
