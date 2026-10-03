//! Finding source files (honoring .gitignore) and naming their modules.

use std::path::{Path, PathBuf};

use arch_graph::RepoPath;

use crate::{IndexError, Pack, PackSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub path: RepoPath,
    pub absolute: PathBuf,
    pub language: String,
    pub module: String,
}

/// All files under `scope` (a directory or file inside `root`) that some pack
/// handles, sorted by path. Ignored files (.gitignore, hidden) are skipped.
pub fn walk(root: &Path, scope: &Path, packs: &PackSet) -> Result<Vec<SourceFile>, IndexError> {
    let start = if scope.is_absolute() { scope.to_path_buf() } else { root.join(scope) };
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(&start).hidden(true).git_ignore(true).git_exclude(true).require_git(false).build();
    for entry in walker {
        let entry = entry.map_err(|e| IndexError::Io(e.to_string()))?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let absolute = entry.path().to_path_buf();
        let Some(pack) = packs.for_path(&absolute) else { continue };
        let relative = absolute.strip_prefix(root).map_err(|_| IndexError::Io(format!("{} is outside {}", absolute.display(), root.display())))?;
        let path = RepoPath::new(&relative.to_string_lossy()).map_err(|e| IndexError::Io(e.to_string()))?;
        let module = module_symbol(root, &path, pack);
        files.push(SourceFile { path, absolute, language: pack.name().to_owned(), module });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The module symbol for a file: package name (from the nearest manifest, if
/// the pack has one) plus its path below the package, minus source roots and
/// the extension; index files name their directory.
pub fn module_symbol(root: &Path, path: &RepoPath, pack: &Pack) -> String {
    let lang = &pack.spec.language;
    let parts: Vec<&str> = path.as_str().split('/').collect();
    let (dirs, file) = parts.split_at(parts.len() - 1);
    // Package root: the deepest directory holding the manifest.
    let mut package_depth = None;
    if let Some(manifest) = &lang.package_manifest {
        for depth in (0..=dirs.len()).rev() {
            let dir: PathBuf = dirs[..depth].iter().collect();
            if root.join(&dir).join(manifest).is_file() {
                package_depth = Some(depth);
                break;
            }
        }
    }
    let mut segments: Vec<String> = Vec::new();
    let rest: &[&str] = match package_depth {
        Some(depth) => {
            let name = match depth {
                0 => root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                _ => dirs[depth - 1].to_owned(),
            };
            let mut name = name;
            for (from, to) in &lang.package_name_replace {
                name = name.replace(from.as_str(), to.as_str());
            }
            segments.push(name);
            &dirs[depth..]
        }
        None => dirs,
    };
    let mut rest = rest.to_vec();
    if rest.first().is_some_and(|first| lang.source_roots.iter().any(|r| r == first)) {
        rest.remove(0);
    }
    segments.extend(rest.iter().map(|s| (*s).to_owned()));
    let file = file[0];
    if !lang.index_files.iter().any(|i| i == file) {
        let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
        segments.push(stem.to_owned());
    }
    if segments.is_empty() {
        segments.push(lang.name.clone());
    }
    segments.join(&lang.separator)
}
