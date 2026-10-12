//! A project's analysis settings, the same for the server and for
//! `meta-harness analyze`: its packs (shipped, then the project's and the
//! user's), its types (`.nah/types.toml`), code rules (`.nah/rules/`) and
//! limits (`.nah/limits.toml`).

use std::path::Path;

use arch_graph::TypeSystem;
use code_analyzer::{CodeRules, PackSet};
use serde::Serialize;

pub struct AnalysisSetup {
    pub catalog: nah_packs::Catalog,
    pub packs: PackSet,
    pub types: TypeSystem,
    pub rules: CodeRules,
    pub limits: arch_service::Limits,
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
    let limits = match std::fs::read_to_string(nah.join("limits.toml")) {
        Ok(text) => arch_service::Limits::with_overrides(&text)?,
        Err(_) => arch_service::Limits::defaults()?,
    };
    Ok(AnalysisSetup { catalog, packs, types, rules, limits })
}

/// What `meta-harness analyze` writes: every file's rows, as the server's
/// pipeline stored them.
#[derive(Serialize)]
pub struct AnalysisOutput {
    pub files: Vec<std::sync::Arc<code_analyzer::rows::FileRows>>,
}

/// The server's pipeline over `root` (read, link, wait for the languages'
/// servers to confirm, store, in a store of its own); writes the rows to
/// `out` as JSON and says what it did.
pub fn analyze_to(root: &Path, out: &Path) -> Result<String, SetupError> {
    let setup = analysis_setup(root)?;
    let store = arch_service::temporary_facts(&setup.limits)?;
    let ready = std::time::Duration::from_secs(setup.limits.servers.ready_s);
    let service = arch_service::ArchitectureService::open(root, setup.packs, setup.types, Box::new(arch_service::MemoryPlans::default()), store)?
        .with_limits(setup.limits)
        .with_rules(setup.rules)
        .with_servers(&setup.catalog);
    let report = service.ingest(root)?;
    let waited = service.wait_for_servers(ready)?;
    let proven = service.observed()?.relations().filter(|r| r.proven_by.is_some()).count();
    let servers: String = service.index_status().servers.iter().map(|s| format!("\n{}: {:?}{}", s.language, s.state, s.reason.as_deref().map(|r| format!(" ({r})")).unwrap_or_default())).collect();
    let summary = format!(
        "{} files read; {} components, {} relations, {proven} confirmed by the languages' servers{}{servers}",
        report.files,
        report.components,
        report.relations,
        if waited { "" } else { " (some servers didn't answer in time)" }
    );
    std::fs::write(out, serde_json::to_vec(&AnalysisOutput { files: service.file_rows() })?)?;
    Ok(summary)
}
