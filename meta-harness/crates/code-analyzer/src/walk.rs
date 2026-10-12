//! Finding source files and naming their modules. What's ignored has one
//! definition, [`Ignores`], which the walk and every other caller ask.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arch_graph::RepoPath;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;

use crate::{IndexError, Pack, PackSet};

/// The project's own ignore file, for generated output and the like
/// (gitignore syntax, relative to the project's root).
pub const PROJECT_IGNORE: &str = ".nah/ignore";

/// The folder a repository's top keeps git's own files in.
const GIT_DIR: &str = ".git";

/// The one definition of "ignored" for a project at `root` (which may sit
/// below its repository's top). A path is ignored when it is outside the
/// root, hidden (a segment starting with `.`), or matched, as git decides
/// it level by level, by: the project's `.nah/ignore`; the `.ignore` and
/// `.gitignore` of every directory from the repository's top down to it
/// (deeper first, `.ignore` before `.gitignore`); the repository's
/// `.git/info/exclude`; and the user's global excludes. Each rule file is
/// read once and again only when its modification time changes.
pub struct Ignores {
    /// The root as given (what callers' paths start with) and resolved.
    given: PathBuf,
    root: PathBuf,
    top: PathBuf,
    /// The repository's `info/exclude` (in its common git folder, for a
    /// worktree).
    exclude: PathBuf,
    global: Option<Arc<Gitignore>>,
    /// Rule file -> its time when read, and its rules.
    files: Mutex<HashMap<PathBuf, RuleFile>>,
}

/// The rules of the folders from one folder up to the repository's top,
/// deepest first (`.ignore` before `.gitignore` in each).
type Folders = Vec<Arc<Gitignore>>;

/// A rule file's time and size when read, and its rules (none when it
/// isn't there).
type RuleFile = (Option<(std::time::SystemTime, u64)>, Option<Arc<Gitignore>>);

/// `path` with its deepest existing ancestor resolved (symlinks, `..`), so
/// it compares with the resolved root even before it exists.
fn resolved(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = at.canonicalize() {
            return rest.iter().rev().fold(real, |p, part| p.join(part));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                at = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// A rule file's time and size (read again when either changes).
fn stamp(file: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn rules(base: &Path, file: &Path) -> Option<Gitignore> {
    if !file.is_file() {
        return None;
    }
    let mut builder = GitignoreBuilder::new(base);
    builder.add(file);
    builder.build().ok().filter(|g| !g.is_empty())
}

/// Where a repository at `top` keeps `info/exclude`: its `.git` folder, or
/// for a worktree (`.git` is a file naming its git folder) the common one.
fn exclude_of(top: &Path) -> PathBuf {
    let dot_git = top.join(GIT_DIR);
    let Ok(text) = std::fs::read_to_string(&dot_git) else { return dot_git.join("info/exclude") };
    let Some(git_dir) = text.lines().find_map(|l| l.strip_prefix("gitdir:")).map(|d| top.join(d.trim())) else { return dot_git.join("info/exclude") };
    let common = std::fs::read_to_string(git_dir.join("commondir")).map(|c| git_dir.join(c.trim())).unwrap_or(git_dir);
    common.join("info/exclude")
}

impl Ignores {
    pub fn new(given: &Path) -> Self {
        let root = given.canonicalize().unwrap_or_else(|_| given.to_path_buf());
        let top = root.ancestors().find(|d| d.join(GIT_DIR).exists()).unwrap_or(&root).to_path_buf();
        let global = GitignoreBuilder::new(&top).build_global().0;
        Self {
            global: (!global.is_empty()).then(|| Arc::new(global)),
            exclude: exclude_of(&top),
            given: given.to_path_buf(),
            root,
            top,
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Whether `path` (which needn't exist) is ignored.
    pub fn is_ignored(&self, path: &Path) -> bool {
        self.is_ignored_as(path, path.is_dir())
    }

    /// The same, saying whether the path is a directory (for a path that
    /// doesn't exist any more).
    pub fn is_ignored_as(&self, path: &Path, is_dir: bool) -> bool {
        self.ignores(path, is_dir, true)
    }

    /// Whether the rules leave `path` out, hidden files kept (what search
    /// lists: CI files, `.gitignore`, plans; never the reader's input). The
    /// repository's own git folder is always left out.
    pub fn rules_ignore(&self, path: &Path) -> bool {
        path.starts_with(self.top.join(GIT_DIR)) || self.ignores(path, path.is_dir(), false)
    }

    fn ignores(&self, path: &Path, is_dir: bool, hidden_too: bool) -> bool {
        let absolute = if path.is_absolute() { path.to_path_buf() } else { self.root.join(path) };
        // Below the root as given or as resolved, without following links
        // on the way; else (reached some other way) resolved.
        let (base, relative) = match (absolute.strip_prefix(&self.given), absolute.strip_prefix(&self.root)) {
            (Ok(rest), _) => (self.given.clone(), rest.to_path_buf()),
            (_, Ok(rest)) => (self.root.clone(), rest.to_path_buf()),
            _ => match resolved(&absolute).strip_prefix(&self.root) {
                Ok(rest) => (self.root.clone(), rest.to_path_buf()),
                Err(_) => return true,
            },
        };
        let parts: Vec<_> = relative.components().collect();
        // A link isn't followed: what's through one is as if it weren't there.
        let mut at = base;
        for part in &parts {
            at.push(part);
            if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
                return true;
            }
        }
        // Git's rule: a path is ignored when it, or a directory above it
        // (below the root), is. Each level is decided by the rules of the
        // folders above it.
        let mut folders = self.folders_above(&self.root);
        let mut at = self.root.clone();
        for (i, part) in parts.iter().enumerate() {
            at.push(part);
            let as_dir = i + 1 < parts.len() || is_dir;
            if self.decides(&folders, &at, as_dir, hidden_too) {
                return true;
            }
            if as_dir {
                folders = self.with_folder(&at, &folders);
            }
        }
        false
    }

    /// The folder rules that apply to entries of `dir`: its own and those
    /// of every folder above it, up to the repository's top.
    fn folders_above(&self, dir: &Path) -> Folders {
        let mut chain: Vec<&Path> = dir.ancestors().take_while(|d| d.starts_with(&self.top)).collect();
        chain.reverse();
        chain.iter().fold(Folders::new(), |above, d| self.with_folder(d, &above))
    }

    /// `above` with `dir`'s own rules in front.
    fn with_folder(&self, dir: &Path, above: &Folders) -> Folders {
        let mut out: Folders = [".ignore", ".gitignore"].iter().filter_map(|name| self.rules_of(dir, &dir.join(name))).collect();
        out.extend(above.iter().cloned());
        out
    }

    /// Whether `path` itself is ignored (hidden, when `hidden_too`; or by the
    /// first rule that matches it: the project's, its folders', git's
    /// exclude, the user's).
    fn decides(&self, folders: &Folders, path: &Path, is_dir: bool, hidden_too: bool) -> bool {
        if hidden_too && path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            return true;
        }
        let project = self.rules_of(&self.root, &self.root.join(PROJECT_IGNORE));
        let exclude = self.rules_of(&self.top, &self.exclude);
        for rules in project.iter().chain(folders).chain(exclude.iter()).chain(self.global.iter()) {
            match rules.matched(path, is_dir) {
                Match::Ignore(_) => return true,
                Match::Whitelist(_) => return false,
                Match::None => {}
            }
        }
        false
    }

    /// A rule file's rules (relative to `base`), read again only when its
    /// time changed.
    fn rules_of(&self, base: &Path, file: &Path) -> Option<Arc<Gitignore>> {
        let now = stamp(file);
        let mut files = self.files.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match files.get(file) {
            Some((when, kept)) if *when == now => kept.clone(),
            _ => {
                let read = now.and_then(|_| rules(base, file)).map(Arc::new);
                files.insert(file.to_path_buf(), (now, read.clone()));
                read
            }
        }
    }

    /// The folders outside the project whose rule files apply to it (the
    /// repository's folders above it, and where its exclude file is): a
    /// watcher watches them too, not their subfolders.
    pub fn rule_folders_outside(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.root.ancestors().skip(1).take_while(|d| d.starts_with(&self.top)).map(Path::to_path_buf).collect();
        if let Some(info) = self.exclude.parent()
            && !info.starts_with(&self.root)
        {
            out.push(info.to_path_buf());
        }
        out
    }

    /// Whether a changed path changes the rules (an ignore file).
    pub fn is_rule_file(&self, path: &Path) -> bool {
        self.rules_scope(path).is_some()
    }

    /// The folder (inside the root, as the root was given) whose files a
    /// changed rule file can make ignored or not: the project for its own,
    /// git's and the parent repo's rules; a folder's own `.gitignore` or
    /// `.ignore` for that folder. None for anything else, and for a rule
    /// file inside an ignored folder (it changes nothing that's read).
    pub fn rules_scope(&self, path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_string_lossy();
        // The project's own, and the repository's exclude (no other file of
        // the same name).
        let own = [self.given.join(PROJECT_IGNORE), self.root.join(PROJECT_IGNORE), self.exclude.clone()];
        if own.iter().any(|o| o == path) || resolved(path) == self.exclude {
            return Some(self.given.clone());
        }
        if name != ".gitignore" && name != ".ignore" {
            return None;
        }
        let dir = resolved(path.parent()?);
        match dir.strip_prefix(&self.root) {
            Ok(rest) if rest.as_os_str().is_empty() => Some(self.given.clone()),
            Ok(rest) => (!self.is_ignored_as(&dir, true)).then(|| self.given.join(rest)),
            // Above the project, in the same repository.
            Err(_) => self.root.starts_with(&dir).then(|| self.given.clone()),
        }
    }

    /// Every file under `scope` that isn't ignored, sorted, under the root
    /// as given. Each entry is decided once, at its own level, with its
    /// folder's rules.
    pub fn files(&self, scope: &Path) -> Vec<PathBuf> {
        let start = resolved(&if scope.is_absolute() { scope.to_path_buf() } else { self.root.join(scope) });
        if !start.starts_with(&self.root) || (start != self.root && self.is_ignored_as(&start, true)) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut stack = vec![(start.clone(), self.folders_above(&start))];
        while let Some((at, folders)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&at) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                if self.decides(&folders, &path, kind.is_dir(), true) {
                    continue;
                }
                if kind.is_dir() {
                    let inner = self.with_folder(&path, &folders);
                    stack.push((path, inner));
                } else if kind.is_file()
                    && let Ok(rest) = path.strip_prefix(&self.root)
                {
                    out.push(self.given.join(rest));
                }
            }
        }
        out.sort();
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceFile {
    pub path: RepoPath,
    pub absolute: PathBuf,
    pub language: String,
    pub module: String,
}

/// All files under `scope` (a directory or file inside `root`) that some pack
/// handles and that [`Ignores`] doesn't skip, sorted by path (a scope that is
/// itself ignored yields nothing).
pub fn walk(root: &Path, scope: &Path, packs: &PackSet) -> Result<Vec<SourceFile>, IndexError> {
    walk_with(&Ignores::new(root), root, scope, packs)
}

/// The same with the project's [`Ignores`] already made.
pub fn walk_with(ignores: &Ignores, root: &Path, scope: &Path, packs: &PackSet) -> Result<Vec<SourceFile>, IndexError> {
    let start = if scope.is_absolute() { scope.to_path_buf() } else { root.join(scope) };
    let candidates = if start.is_file() {
        if ignores.is_ignored(&start) { Vec::new() } else { vec![start] }
    } else {
        ignores.files(&start)
    };
    let real_root = resolved(root);
    let mut files = Vec::new();
    for absolute in candidates {
        let Some(pack) = packs.for_path(&absolute) else { continue };
        let relative = resolved(&absolute);
        let relative = relative.strip_prefix(&real_root).map_err(|_| IndexError::Io(format!("{} is outside {}", absolute.display(), root.display())))?;
        let path = RepoPath::new(&relative.to_string_lossy()).map_err(|e| IndexError::Io(e.to_string()))?;
        let module = module_symbol(root, &path, pack);
        files.push(SourceFile { path, absolute: root.join(relative), language: pack.name().to_owned(), module });
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
