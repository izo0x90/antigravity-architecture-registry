//! A project's analysis settings, the same for the server and for
//! `meta-harness analyze`: its packs (shipped, then the project's and the
//! user's), its types (`.nah/types.toml`) and code rules (`.nah/rules/`).

use std::path::Path;

use arch_graph::TypeSystem;
use code_analyzer::{CodeRules, Indexers, PackSet};
use serde::Serialize;

pub struct AnalysisSetup {
    pub catalog: nah_packs::Catalog,
    pub packs: PackSet,
    pub types: TypeSystem,
    pub rules: CodeRules,
}

pub type SetupError = Box<dyn std::error::Error + Send + Sync>;

pub fn analysis_setup(root: &Path) -> Result<AnalysisSetup, SetupError> {
    let nah = root.join(".nah");
    let catalog = crate::packs::catalog(root)?;
    let packs = PackSet::from_catalog(&catalog)?;
    let mut types = TypeSystem::defaults()?;
    if let Ok(text) = std::fs::read_to_string(nah.join("types.toml")) {
        types.extend_from_toml(&text)?;
    }
    let mut rules = CodeRules::defaults(&packs, &types)?;
    if nah.join("rules").is_dir() {
        rules.load_dir(&nah.join("rules"), &packs, &types)?;
    }
    Ok(AnalysisSetup { catalog, packs, types, rules })
}

/// What `meta-harness analyze` writes: what the server keeps.
#[derive(Serialize)]
pub struct AnalysisOutput {
    pub combined: code_analyzer::Combined,
    pub occurrences: std::collections::BTreeMap<arch_graph::RepoPath, Vec<code_analyzer::Occurrence>>,
    pub indexed: Vec<code_analyzer::index::IndexedFile>,
    /// Indexers that couldn't run, and why.
    pub skipped: Vec<(String, String)>,
}

/// Every stage over `root` (its languages' indexers included, where
/// installed); writes the result to `out` as JSON and says what it did.
pub fn analyze_to(root: &Path, out: &Path) -> Result<String, SetupError> {
    let setup = analysis_setup(root)?;
    let lock = nah_packs::Lock::read(&setup.catalog.lock_path)?;
    let analysis = code_analyzer::analyze(root, root, &setup.packs, &setup.rules, &setup.types, Some(Indexers { catalog: &setup.catalog, lock: &lock }))?;
    let files = analysis.extracts.len();
    let indexed = analysis.indexed.len();
    let (combined, occurrences) = analysis.resolved.into_kept();
    let summary = format!(
        "{files} files read, {indexed} indexed; {} components, {} relations ({} proven){}",
        combined.components.len(),
        combined.relations.len(),
        combined.relations.iter().filter(|r| r.proven_by.is_some()).count(),
        analysis.skipped.iter().map(|(l, why)| format!("\n{l}: not indexed: {why}")).collect::<String>()
    );
    let output = AnalysisOutput { combined, occurrences, indexed: analysis.indexed, skipped: analysis.skipped };
    std::fs::write(out, serde_json::to_vec(&output)?)?;
    Ok(summary)
}
