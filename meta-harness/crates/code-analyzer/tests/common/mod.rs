//! What every test here shares.

use std::path::{Path, PathBuf};

use code_analyzer::PackSet;

pub fn shipped_packs_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs")
}

/// The language packs shipped in this repo, without any project's or
/// user's own (tests must not depend on the machine).
pub fn shipped_packs() -> PackSet {
    let distribution = nah_packs::Distribution::from_dir(&shipped_packs_dir()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let catalog = nah_packs::Catalog::load(&distribution, &nah_packs::Places::isolated(dir.path())).unwrap();
    PackSet::from_catalog(&catalog).unwrap()
}
