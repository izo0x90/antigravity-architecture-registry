//! A harness's skills, found the way that harness finds them: its folders
//! in order (the first skill of a name wins), each a skill itself or a
//! folder of skills one level deep, read within a budget so a huge folder
//! can't stall the listing. How a skill is named, which file it is and the
//! order folders are read in differ by harness; they're [`SkillRules`],
//! given as data (a harness pack's settings).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use harness_contracts::{Scope, Skill};
use serde::Deserialize;

/// Largest skill file read.
pub const MAX_SKILL_BYTES: u64 = 1_000_000;
/// Most bytes, and folder entries, read for one listing.
pub const MAX_SCAN_BYTES: u64 = 8_000_000;
pub const MAX_SCAN_ENTRIES: usize = 10_000;

/// How a harness reads its skills.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillRules {
    pub name: NameRule,
    pub file: FileRule,
    pub order: OrderRule,
}

/// A skill's name when its frontmatter gives none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRule {
    /// Its folder's name.
    #[default]
    Folder,
    /// Its file's name without `.md`.
    File,
}

/// Which file in a folder is the skill.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileRule {
    /// `SKILL.md` exactly.
    #[default]
    Exact,
    /// `skill.md` in any case, as long as it ends `.md`.
    AnyCase,
}

/// The order a folder's skills are read in (it decides which of two of a
/// name wins).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderRule {
    /// By name, byte by byte.
    #[default]
    Plain,
    /// By name as a URL path escapes it (Go's `URL.EscapedPath`).
    EscapedPath,
}

/// A folder the harness reads skills from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRoot {
    pub path: PathBuf,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SkillsError {
    /// The listing would read more than its budget (a skill file, or all of
    /// them, too large; too many entries): it stops rather than return
    /// part of the list.
    #[error("too much to read for the skill list, at {path}")]
    OverBudget { path: String },
    #[error("couldn't read the skill at {path}")]
    Unreadable { path: String },
}

/// The skills in `roots`, read by `rules`, sorted by name.
pub fn discover_skills(roots: &[SkillRoot], rules: SkillRules) -> Result<Vec<Skill>, SkillsError> {
    let mut scan = Scan { rules, bytes: MAX_SCAN_BYTES, entries: MAX_SCAN_ENTRIES, found: HashMap::new() };
    for root in roots {
        scan.folder(&root.path, root.scope, true)?;
    }
    let mut skills: Vec<Skill> = scan.found.into_values().collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

struct Scan {
    rules: SkillRules,
    /// What's left of the budget.
    bytes: u64,
    entries: usize,
    found: HashMap<String, Skill>,
}

impl Scan {
    /// A folder: a skill if it has a skill file, else (for a root) a
    /// folder of skills.
    fn folder(&mut self, dir: &Path, scope: Scope, children: bool) -> Result<(), SkillsError> {
        if !fs::metadata(dir).is_ok_and(|m| m.is_dir()) {
            return Ok(());
        }
        let Ok(read) = fs::read_dir(dir) else { return Ok(()) };
        let mut names: Vec<String> = read.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        if names.len() > self.entries {
            return Err(SkillsError::OverBudget { path: dir.display().to_string() });
        }
        self.entries -= names.len();
        names.sort();

        let file = names.iter().find(|n| match self.rules.file {
            FileRule::Exact => n.as_str() == "SKILL.md",
            FileRule::AnyCase => n.eq_ignore_ascii_case("skill.md"),
        });
        if let Some(file) = file {
            // Found in any case, but only a lowercase `.md` is read.
            if file.ends_with(".md") {
                self.skill(dir, file, scope)?;
            }
            return Ok(());
        }
        if children {
            if self.rules.order == OrderRule::EscapedPath {
                names.sort_by_cached_key(|n| escaped_path(n));
            }
            for name in names {
                self.folder(&dir.join(name), scope, false)?;
            }
        }
        Ok(())
    }

    fn skill(&mut self, dir: &Path, file: &str, scope: Scope) -> Result<(), SkillsError> {
        let path = dir.join(file);
        let Some(contents) = self.read(&path)? else { return Ok(()) };
        let fallback = match self.rules.name {
            NameRule::File => file.strip_suffix(".md").unwrap_or(file).to_owned(),
            NameRule::Folder => dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let Some((name, description)) = frontmatter(&contents, fallback) else { return Ok(()) };
        self.found.entry(name.clone()).or_insert(Skill { name, description, path: Some(path.display().to_string()), scope });
        Ok(())
    }

    /// A skill file's text (none if it's gone, not a file or not UTF-8),
    /// counted against the budget.
    fn read(&mut self, path: &Path) -> Result<Option<String>, SkillsError> {
        let unreadable = || SkillsError::Unreadable { path: path.display().to_string() };
        let over = || SkillsError::OverBudget { path: path.display().to_string() };
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unreadable()),
        };
        if !meta.is_file() {
            return Ok(None);
        }
        let limit = MAX_SKILL_BYTES.min(self.bytes);
        if meta.len() > limit {
            return Err(over());
        }
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unreadable()),
        };
        // It may have grown since.
        if bytes.len() as u64 > limit {
            return Err(over());
        }
        self.bytes -= bytes.len() as u64;
        Ok(String::from_utf8(bytes).ok())
    }
}

/// A name as Go's `URL.EscapedPath` writes it.
fn escaped_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A skill's name and description from its frontmatter (the YAML between
/// the first two `---`), the name `fallback` when it gives none. None when
/// there's no frontmatter, it isn't a map, or a field isn't text.
pub fn frontmatter(contents: &str, fallback: String) -> Option<(String, Option<String>)> {
    let start = contents.find("---")? + 3;
    let end = start + contents[start..].find("---")?;
    let yaml = contents[start..end].trim();
    let value: serde_yaml::Value = if yaml.is_empty() { serde_yaml::Value::Null } else { serde_yaml::from_str(yaml).ok()? };
    let empty = serde_yaml::Mapping::new();
    let map = match &value {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => &empty,
        _ => return None,
    };
    // A field that's absent, null or empty is not given; one that isn't text is wrong.
    let text = |key: &str| -> Option<Option<&str>> {
        match map.get(key) {
            None | Some(serde_yaml::Value::Null) => Some(None),
            Some(v) => v.as_str().map(Some),
        }
    };
    let name = match text("name")? {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => fallback,
    };
    if name.is_empty() || name != name.trim() {
        return None;
    }
    let description = text("description")?.map(str::trim).filter(|d| !d.is_empty()).map(str::to_owned);
    Some((name, description))
}
