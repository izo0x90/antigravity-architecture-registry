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
            let dir: PathBuf = root.join(dirs[..depth].iter().collect::<PathBuf>());
            let manifest = lang.package_manifest.as_deref().map(|m| dir.join(m));
            let named = manifest.and_then(|m| std::fs::read_to_string(&m).ok().and_then(|text| package_name(&m, &text, pack)));
            // Without a name in the manifest: the directory's own name.
            let mut name = named.unwrap_or_else(|| match depth {
                0 => root.canonicalize().ok().and_then(|r| r.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default(),
                _ => dirs[depth - 1].to_owned(),
            });
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
    segments.retain(|s| !s.is_empty());
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

/// The package name a manifest declares, per the pack: a key path into a
/// TOML or JSON manifest, or a regex's first group.
fn package_name(manifest: &Path, text: &str, pack: &Pack) -> Option<String> {
    let lang = &pack.spec.language;
    if let Some(re) = &pack.package_name_regex {
        return re.captures(text).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned());
    }
    if lang.package_name_key.is_empty() {
        return None;
    }
    let value: serde_json::Value = match manifest.extension().and_then(|e| e.to_str()) {
        Some("json") => serde_json::from_str(text).ok()?,
        _ => serde_json::to_value(toml::from_str::<toml::Value>(text).ok()?).ok()?,
    };
    let mut node = &value;
    for key in &lang.package_name_key {
        node = node.get(key)?;
    }
    node.as_str().map(str::to_owned)
}
