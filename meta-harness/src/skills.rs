use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

pub const MAX_SKILL_BYTES: u64 = 1_000_000;
pub const MAX_SCAN_BYTES: u64 = 8_000_000;
pub const MAX_SCAN_ENTRIES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProviderSkill {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub path: String,
    pub scope: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AntigravitySkillsProbeError {
    #[error("Antigravity skill discovery exceeded its scan limit at '{path}'.")]
    ScanBudgetExhausted { path: String },
    #[error("Antigravity could not read skills at '{path}'.")]
    FilesystemError { path: String },
}

#[derive(Debug)]
pub struct ScanBudget {
    pub remaining_bytes: u64,
    pub remaining_entries: usize,
}

#[derive(Debug, Clone)]
pub struct DiscoverSkillsInput {
    pub cwd: PathBuf,
    pub user_home: PathBuf,
}

/// Resolves user home directory matching Python's expanduser in launch environment
pub fn resolve_antigravity_user_home(
    platform: &str,
    environment: &HashMap<String, String>,
) -> String {
    if platform == "win32" {
        if let Some(userprofile) = environment.get("USERPROFILE")
            && !userprofile.is_empty()
        {
            return userprofile.clone();
        }
        if let (Some(drive), Some(homepath)) =
            (environment.get("HOMEDRIVE"), environment.get("HOMEPATH"))
            && !drive.is_empty()
            && !homepath.is_empty()
        {
            return format!("{}{}", drive, homepath);
        }
        std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "C:\\Users\\default".to_string())
    } else {
        if let Some(home) = environment.get("HOME")
            && !home.is_empty()
        {
            return home.clone();
        }
        std::env::var("HOME").unwrap_or_else(|_| "/home/default".to_string())
    }
}

/// The native loader orders child paths with Go's URL.EscapedPath encoding.
pub fn skill_path_sort_key(entry: &str) -> String {
    let mut out = String::new();
    for byte in entry.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            b => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

pub fn parse_skill_frontmatter(
    contents: &str,
    file_name: &str,
) -> Option<(String, Option<String>)> {
    let start = contents.find("---")?;
    let after_start = &contents[start + 3..];
    let end = after_start.find("---")?;
    let yaml_str = after_start[..end].trim();

    let yaml_val: serde_yaml::Value = if yaml_str.is_empty() {
        serde_yaml::Value::Mapping(Default::default())
    } else {
        serde_yaml::from_str(yaml_str).ok()?
    };
    let empty_mapping = serde_yaml::Mapping::new();
    let mapping = match &yaml_val {
        serde_yaml::Value::Mapping(m) => m,
        serde_yaml::Value::Null => &empty_mapping,
        _ => return None,
    };

    let default_name = file_name.strip_suffix(".md").unwrap_or(file_name).to_string();
    let name_key = serde_yaml::Value::String("name".to_string());
    let raw_name = if let Some(n_val) = mapping.get(&name_key) {
        if n_val.is_null() {
            default_name.clone()
        } else if let Some(n_str) = n_val.as_str() {
            if n_str.is_empty() {
                default_name.clone()
            } else {
                n_str.to_string()
            }
        } else {
            return None;
        }
    } else {
        default_name.clone()
    };

    if raw_name.is_empty() || raw_name != raw_name.trim() {
        return None;
    }

    let desc_key = serde_yaml::Value::String("description".to_string());
    let description = if let Some(d_val) = mapping.get(&desc_key) {
        if d_val.is_null() {
            None
        } else if let Some(d_str) = d_val.as_str() {
            let trimmed = d_str.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        } else {
            return None;
        }
    } else {
        None
    };

    Some((raw_name, description))
}

fn read_skill(
    skill_path: &Path,
    budget: &mut ScanBudget,
) -> Result<Option<String>, AntigravitySkillsProbeError> {
    let metadata = match fs::metadata(skill_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(AntigravitySkillsProbeError::FilesystemError {
                path: skill_path.display().to_string(),
            })
        }
    };

    if !metadata.is_file() {
        return Ok(None);
    }

    let byte_limit = std::cmp::min(MAX_SKILL_BYTES, budget.remaining_bytes);
    if metadata.len() > byte_limit {
        return Err(AntigravitySkillsProbeError::ScanBudgetExhausted {
            path: skill_path.display().to_string(),
        });
    }

    let bytes = match fs::read(skill_path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(AntigravitySkillsProbeError::FilesystemError {
                path: skill_path.display().to_string(),
            })
        }
    };

    if bytes.len() as u64 > byte_limit {
        return Err(AntigravitySkillsProbeError::ScanBudgetExhausted {
            path: skill_path.display().to_string(),
        });
    }

    budget.remaining_bytes -= bytes.len() as u64;
    Ok(String::from_utf8(bytes).ok())
}

fn scan_directory(
    directory: &Path,
    scope: &str,
    scan_children: bool,
    budget: &mut ScanBudget,
    skills_by_name: &mut HashMap<String, ServerProviderSkill>,
) -> Result<(), AntigravitySkillsProbeError> {
    let metadata = match fs::metadata(directory) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };
    if !metadata.is_dir() {
        return Ok(());
    }

    let dir_entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => return Ok(()),
    };

    let mut entry_names = Vec::new();
    for entry in dir_entries.flatten() {
        if let Ok(name) = entry.file_name().into_string() {
            entry_names.push(name);
        }
    }

    if entry_names.len() > budget.remaining_entries {
        return Err(AntigravitySkillsProbeError::ScanBudgetExhausted {
            path: directory.display().to_string(),
        });
    }
    budget.remaining_entries -= entry_names.len();

    entry_names.sort();

    let skill_file_name = entry_names
        .iter()
        .find(|name| name.to_lowercase() == "skill.md")
        .cloned();

    if let Some(skill_file_name) = skill_file_name {
        if !skill_file_name.ends_with(".md") {
            return Ok(());
        }
        let skill_path = directory.join(&skill_file_name);
        if let Some(contents) = read_skill(&skill_path, budget)?
            && let Some((name, description)) =
                parse_skill_frontmatter(&contents, &skill_file_name)
            && !skills_by_name.contains_key(&name)
        {
            skills_by_name.insert(
                name.clone(),
                ServerProviderSkill {
                    name,
                    description,
                    path: skill_path.display().to_string(),
                    scope: scope.to_string(),
                    enabled: true,
                },
            );
        }
        return Ok(());
    }

    if scan_children {
        let mut sorted_children = entry_names;
        sorted_children.sort_by_key(|a| skill_path_sort_key(a));

        for entry in sorted_children {
            let child_dir = directory.join(entry);
            scan_directory(&child_dir, scope, false, budget, skills_by_name)?;
        }
    }

    Ok(())
}

/// Discovers Antigravity skills following native root precedence and scan budgets
pub fn discover_antigravity_skills(
    input: &DiscoverSkillsInput,
) -> Result<Vec<ServerProviderSkill>, AntigravitySkillsProbeError> {
    let roots = [
        (
            input.user_home.join(".gemini").join("config").join("skills"),
            "user",
        ),
        (input.cwd.join(".gemini").join("skills"), "project"),
        (
            input
                .user_home
                .join(".gemini")
                .join("antigravity-cli")
                .join("skills"),
            "user",
        ),
        (input.cwd.join(".agents").join("skills"), "project"),
        (input.cwd.join(".agent").join("skills"), "project"),
    ];

    let mut budget = ScanBudget {
        remaining_bytes: MAX_SCAN_BYTES,
        remaining_entries: MAX_SCAN_ENTRIES,
    };
    let mut skills_by_name = HashMap::new();

    for (root_dir, scope) in roots {
        scan_directory(&root_dir, scope, true, &mut budget, &mut skills_by_name)?;
    }

    let mut result: Vec<ServerProviderSkill> = skills_by_name.into_values().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}
