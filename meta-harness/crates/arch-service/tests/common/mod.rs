//! What every test here shares.

/// The language packs shipped in this repo, without any project's or
/// user's own (tests must not depend on the machine).
pub fn shipped_packs() -> code_analyzer::PackSet {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs");
    let places = nah_packs::Places::isolated(&std::env::temp_dir().join(format!("nah-test-packs-{}", std::process::id())));
    let catalog = nah_packs::Catalog::load(&nah_packs::Distribution::from_dir(&dir).unwrap(), &places).unwrap();
    code_analyzer::PackSet::from_catalog(&catalog).unwrap()
}

/// What tests ask of a report.
#[allow(dead_code)]
pub trait Findings {
    /// Each finding's code, in order.
    fn codes(&self) -> Vec<arch_graph::FindingCode>;
    /// The errors about `id`.
    fn errors_for<'a>(&'a self, id: &'a arch_graph::ComponentId) -> impl Iterator<Item = &'a arch_graph::Finding>;
}

impl Findings for arch_graph::Report {
    fn codes(&self) -> Vec<arch_graph::FindingCode> {
        self.findings.iter().map(|f| f.code).collect()
    }

    fn errors_for<'a>(&'a self, id: &'a arch_graph::ComponentId) -> impl Iterator<Item = &'a arch_graph::Finding> {
        self.findings.iter().filter(move |f| f.severity == arch_graph::Severity::Error && f.component() == Some(id))
    }
}

/// Saves `plan` the way someone else would (an editor, another process),
/// then tells `service` its file changed, as the watcher does.
#[allow(dead_code)]
pub fn put_plan(service: &arch_service::ArchitectureService, root: &std::path::Path, plan: &arch_graph::Plan) {
    use arch_service::PlanRepository;
    arch_service::FsPlans::new(root).unwrap().save(plan).unwrap();
    service.refresh(&[root.join(format!(".nah/plans/{}.json", plan.id))]).unwrap();
}

/// A copy of `root`'s source files (what the packs read) and the package
/// manifests above them (`Cargo.toml`: they say where a crate starts, so
/// modules are named as in the real tree) in `to`, dated `dated` (an hour
/// back keeps a time too recent to trust from making every file read again).
#[allow(dead_code)]
pub fn copy_sources(root: &std::path::Path, to: &std::path::Path, packs: &code_analyzer::PackSet, dated: std::time::SystemTime) {
    let manifests: Vec<&str> = packs.packs.iter().filter_map(|p| p.spec.language.package_manifest.as_deref()).collect();
    let mut copied = std::collections::BTreeSet::new();
    let mut copy = |from: &std::path::Path, rel: &std::path::Path| {
        if !copied.insert(rel.to_path_buf()) {
            return;
        }
        let target = to.join(rel);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(from, &target).unwrap();
        std::fs::File::options().write(true).open(&target).unwrap().set_modified(dated).unwrap();
    };
    for file in code_analyzer::walk(root, root, packs).unwrap() {
        copy(&file.absolute, std::path::Path::new(file.path.as_str()));
        let mut dir = std::path::Path::new(file.path.as_str()).parent();
        while let Some(d) = dir {
            for m in &manifests {
                if root.join(d).join(m).is_file() {
                    copy(&root.join(d).join(m), &d.join(m));
                }
            }
            dir = d.parent();
        }
    }
}

/// A folder for a fixture copy that lives as long as the test process, in
/// the build's own scratch folder: one per process, and copies left by runs
/// that ended over an hour ago are removed.
#[allow(dead_code)]
pub fn lasting_dir() -> std::path::PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let all = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(env!("CARGO_CRATE_NAME"));
    if n == 0
        && let Ok(entries) = std::fs::read_dir(&all)
    {
        let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        for entry in entries.flatten() {
            if entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < hour_ago) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let dir = all.join(format!("{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}
