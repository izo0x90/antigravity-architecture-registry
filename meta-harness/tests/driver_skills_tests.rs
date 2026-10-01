use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use meta_harness::skills::{
    discover_antigravity_skills, resolve_antigravity_user_home, AntigravitySkillsProbeError,
    DiscoverSkillsInput, ServerProviderSkill,
};

fn write_skill(directory: &Path, contents: &str) -> PathBuf {
    fs::create_dir_all(directory).expect("create dir");
    let skill_path = directory.join("SKILL.md");
    fs::write(&skill_path, contents).expect("write skill");
    skill_path
}

struct TestWorkspace {
    _temp: TempDir,
    cwd: PathBuf,
    user_home: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let temp = TempDir::new().expect("temp dir");
        let cwd = temp.path().join("workspace");
        let user_home = temp.path().join("home");
        fs::create_dir_all(&cwd).expect("create cwd");
        fs::create_dir_all(&user_home).expect("create home");
        Self {
            _temp: temp,
            cwd,
            user_home,
        }
    }

    fn input(&self) -> DiscoverSkillsInput {
        DiscoverSkillsInput {
            cwd: self.cwd.clone(),
            user_home: self.user_home.clone(),
        }
    }
}

/// Ported from AntigravitySkills.test.ts lines 32-67:
/// "does not read user skills from a nested project or from ~/.agents"
#[test]
fn test_does_not_read_user_skills_from_nested_project_or_from_dot_agents() {
    let ws = TestWorkspace::new();
    let nested_cwd = ws.user_home.join("AI").join("Projects").join("Something");
    fs::create_dir_all(&nested_cwd).expect("create nested");

    let skill_path = write_skill(
        &ws.user_home.join(".gemini").join("config").join("skills").join("review"),
        "---\nname: review\ndescription: Review changes.\n---\n",
    );
    write_skill(
        &ws.user_home.join(".agents").join("skills").join("ignored"),
        "---\nname: ignored\n---\n",
    );

    let nested_input = DiscoverSkillsInput {
        cwd: nested_cwd,
        user_home: ws.user_home.clone(),
    };

    let skills = discover_antigravity_skills(&nested_input).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "review".to_string(),
            description: Some("Review changes.".to_string()),
            path: skill_path.display().to_string(),
            scope: "user".to_string(),
            enabled: true,
        }]
    );

    // A project rooted at the home directory sees ~/.agents/skills as its own project scope
    let home_input = DiscoverSkillsInput {
        cwd: ws.user_home.clone(),
        user_home: ws.user_home.clone(),
    };
    let home_skills = discover_antigravity_skills(&home_input).expect("discover home");
    let mapped: Vec<(String, String)> = home_skills
        .into_iter()
        .map(|s| (s.name, s.scope))
        .collect();
    assert_eq!(
        mapped,
        vec![
            ("ignored".to_string(), "project".to_string()),
            ("review".to_string(), "user".to_string()),
        ]
    );
}

/// Ported from AntigravitySkills.test.ts lines 69-95:
/// "reads skill names, descriptions and paths from the current native roots"
#[test]
fn test_reads_skill_names_descriptions_and_paths_from_native_roots() {
    let ws = TestWorkspace::new();
    let roots = [
        (
            ws.user_home.join(".gemini").join("config").join("skills"),
            "user",
        ),
        (ws.cwd.join(".gemini").join("skills"), "project"),
        (
            ws.user_home
                .join(".gemini")
                .join("antigravity-cli")
                .join("skills"),
            "user",
        ),
        (ws.cwd.join(".agents").join("skills"), "project"),
    ];

    let mut expected = Vec::new();
    for (index, (dir, scope)) in roots.iter().enumerate() {
        let name = format!("review-{}", index);
        let description = format!("Review changes in root {}.", index);
        let skill_path = write_skill(
            &dir.join(&name),
            &format!("---\nname: {}\ndescription: {}\n---\n# Review\n", name, description),
        );
        expected.push(ServerProviderSkill {
            name,
            description: Some(description),
            path: skill_path.display().to_string(),
            scope: scope.to_string(),
            enabled: true,
        });
    }

    let mut skills = discover_antigravity_skills(&ws.input()).expect("discover");
    expected.sort_by(|a, b| a.name.cmp(&b.name));
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(skills, expected);
}

/// Ported from AntigravitySkills.test.ts lines 97-102:
/// "returns no skills when the native roots are missing"
#[test]
fn test_returns_no_skills_when_native_roots_are_missing() {
    let ws = TestWorkspace::new();
    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert!(skills.is_empty());
}

/// Ported from AntigravitySkills.test.ts lines 104-123:
/// "discovers skills from the legacy workspace root"
#[test]
fn test_discovers_skills_from_legacy_workspace_root() {
    let ws = TestWorkspace::new();
    let skill_path = write_skill(
        &ws.cwd.join(".agent").join("skills").join("review"),
        "---\nname: review\ndescription: Review changes.\n---\n",
    );

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "review".to_string(),
            description: Some("Review changes.".to_string()),
            path: skill_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );
}

/// Ported from AntigravitySkills.test.ts lines 125-151:
/// "uses native root order for duplicate names"
#[test]
fn test_uses_native_root_order_for_duplicate_names() {
    let ws = TestWorkspace::new();
    let roots = [
        ws.user_home.join(".gemini").join("config").join("skills"),
        ws.cwd.join(".gemini").join("skills"),
        ws.user_home
            .join(".gemini")
            .join("antigravity-cli")
            .join("skills"),
        ws.cwd.join(".agents").join("skills"),
        ws.cwd.join(".agent").join("skills"),
    ];

    for (index, root) in roots.iter().enumerate() {
        write_skill(
            &root.join(format!("copy-{}", index)),
            &format!("---\nname: review\ndescription: Copy {}.\n---\n", index),
        );
    }

    for (index, root) in roots.iter().enumerate() {
        let skills = discover_antigravity_skills(&ws.input()).expect("discover");
        assert_eq!(skills.len(), 1);
        let expected_path = root.join(format!("copy-{}", index)).join("SKILL.md");
        assert_eq!(skills[0].path, expected_path.display().to_string());
        fs::remove_dir_all(root).expect("remove root");
    }
}

/// Ported from AntigravitySkills.test.ts lines 153-169:
/// "loads a root skill without scanning its children"
#[test]
fn test_loads_root_skill_without_scanning_its_children() {
    let ws = TestWorkspace::new();
    let root = ws.cwd.join(".agents").join("skills");
    let skill_path = write_skill(&root, "---\nname: root-skill\n---\n");
    write_skill(&root.join("child"), "---\nname: child-skill\n---\n");

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "root-skill".to_string(),
            description: None,
            path: skill_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );

    // Invalid root frontmatter stops loading without scanning children
    fs::write(&skill_path, "---\nname: [invalid\n---\n").expect("write invalid");
    let skills_empty = discover_antigravity_skills(&ws.input()).expect("discover empty");
    assert!(skills_empty.is_empty());
}

/// Ported from AntigravitySkills.test.ts lines 171-194:
/// "uses the filename when valid frontmatter has no name"
#[test]
fn test_uses_the_filename_when_valid_frontmatter_has_no_name() {
    let ws = TestWorkspace::new();
    let skill_dir = ws.cwd.join(".agents").join("skills").join("not-the-name");
    let skill_path = write_skill(&skill_dir, "---\n---\n# Skill body\n");

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "SKILL".to_string(),
            description: None,
            path: skill_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );

    // Lowercase filename
    let lowercase_path = skill_dir.join("skill.md");
    fs::rename(&skill_path, &lowercase_path).expect("rename lowercase");
    let skills_lower = discover_antigravity_skills(&ws.input()).expect("discover lower");
    assert_eq!(
        skills_lower,
        vec![ServerProviderSkill {
            name: "skill".to_string(),
            description: None,
            path: lowercase_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );

    // Uppercase .MD extension is rejected
    let upper_ext_path = skill_dir.join("SKILL.MD");
    fs::rename(&lowercase_path, &upper_ext_path).expect("rename upper");
    let skills_rejected = discover_antigravity_skills(&ws.input()).expect("discover rejected");
    assert!(skills_rejected.is_empty());
}

/// Ported from AntigravitySkills.test.ts lines 196-209:
/// "accepts native metadata delimiters after leading text"
#[test]
fn test_accepts_native_metadata_delimiters_after_leading_text() {
    let ws = TestWorkspace::new();
    let skill_path = write_skill(
        &ws.cwd.join(".agents").join("skills").join("review"),
        "Leading text.---\nname: review\ndescription: null\n---Skill body.",
    );

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "review".to_string(),
            description: None,
            path: skill_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );
}

/// Ported from AntigravitySkills.test.ts lines 211-249:
/// "ignores invalid files and deeper directories but keeps native hidden skills"
#[test]
fn test_ignores_invalid_files_and_deeper_directories_but_keeps_native_hidden_skills() {
    let ws = TestWorkspace::new();
    let root = ws.cwd.join(".agents").join("skills");

    let invalid_skills = [
        ("plain", "# Missing frontmatter\n"),
        ("broken", "---\nname: [unclosed\n---\n"),
        ("scalar", "---\n42\n---\n"),
        ("wrong-type", "---\nname: invalid\ndescription: {}\n---\n"),
        ("blank-name", "---\nname: \" \"\n---\n"),
    ];

    for (name, contents) in invalid_skills {
        write_skill(&root.join(name), contents);
    }

    write_skill(&root.join("nested").join("deep"), "---\nname: too-deep\n---\n");
    write_skill(
        &ws.cwd.join(".claude").join("skills").join("wrong-provider"),
        "---\nname: wrong-provider\n---\n",
    );
    fs::create_dir_all(root.join(".not-a-skill")).expect("create not skill");
    fs::write(root.join("README.md"), "Not a skill.").expect("write readme");

    let skill_path = write_skill(
        &root.join(".native-hidden-skill"),
        "---\nname: native-name\ndescription: >\n  Review the code\n  and run tests.\n---\n",
    );

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "native-name".to_string(),
            description: Some("Review the code and run tests.".to_string()),
            path: skill_path.display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );
}

/// Ported from AntigravitySkills.test.ts lines 251-278:
/// "uses native URI order within a root and skips an invalid higher root"
#[test]
fn test_uses_native_uri_order_within_a_root_and_skips_invalid_higher_root() {
    let ws = TestWorkspace::new();
    let root = ws.cwd.join(".agents").join("skills");

    write_skill(
        &ws.user_home.join(".gemini").join("config").join("skills").join("review"),
        "---\nname: [invalid\n---\n",
    );

    let native_order = [" space-copy", "!-copy", "ø-copy", "a-copy"];
    for name in native_order {
        write_skill(&root.join(name), "---\nname: review\n---\n");
    }

    for name in native_order {
        let skills = discover_antigravity_skills(&ws.input()).expect("discover");
        assert_eq!(
            skills,
            vec![ServerProviderSkill {
                name: "review".to_string(),
                description: None,
                path: root.join(name).join("SKILL.md").display().to_string(),
                scope: "project".to_string(),
                enabled: true,
            }]
        );
        fs::remove_dir_all(root.join(name)).expect("remove copy");
    }
}

/// Ported from AntigravitySkills.test.ts lines 280-303:
/// "follows directory symlinks used to install shared skills"
#[cfg(unix)]
#[test]
fn test_follows_directory_symlinks_used_to_install_shared_skills() {
    let ws = TestWorkspace::new();
    let source_dir = ws.user_home.join("shared-review");
    write_skill(&source_dir, "---\nname: review\n---\n");

    let root = ws.cwd.join(".agents").join("skills");
    let linked_dir = root.join("review");
    fs::create_dir_all(&root).expect("create root");
    std::os::unix::fs::symlink(&source_dir, &linked_dir).expect("create symlink");

    let skills = discover_antigravity_skills(&ws.input()).expect("discover");
    assert_eq!(
        skills,
        vec![ServerProviderSkill {
            name: "review".to_string(),
            description: None,
            path: linked_dir.join("SKILL.md").display().to_string(),
            scope: "project".to_string(),
            enabled: true,
        }]
    );
}

/// Ported from AntigravitySkills.test.ts lines 305-321:
/// "rejects an oversized skill instead of returning an incomplete catalog"
#[test]
fn test_rejects_oversized_skill_instead_of_returning_incomplete_catalog() {
    let ws = TestWorkspace::new();
    let skill_path = write_skill(
        &ws.cwd.join(".agents").join("skills").join("oversized"),
        &format!(
            "---\nname: oversized\ndescription: Read a large skill.\n---\n{}",
            "x".repeat(1_000_000)
        ),
    );

    let result = discover_antigravity_skills(&ws.input());
    assert_eq!(
        result,
        Err(AntigravitySkillsProbeError::ScanBudgetExhausted {
            path: skill_path.display().to_string(),
        })
    );
}

/// Ported from AntigravitySkills.test.ts lines 323-343:
/// "bounds the total read size across skills"
#[test]
fn test_bounds_the_total_read_size_across_skills() {
    let ws = TestWorkspace::new();
    let root = ws.cwd.join(".agents").join("skills");
    for index in 0..9 {
        write_skill(
            &root.join(format!("large-{}", index)),
            &format!("---\nname: large-{}\n---\n{}", index, "x".repeat(900_000)),
        );
    }

    let result = discover_antigravity_skills(&ws.input());
    assert_eq!(
        result,
        Err(AntigravitySkillsProbeError::ScanBudgetExhausted {
            path: root.join("large-8").join("SKILL.md").display().to_string(),
        })
    );
}

/// Ported from AntigravitySkills.test.ts lines 345-360:
/// "resolves the home the agent expands ~ against"
#[test]
fn test_resolves_the_home_the_agent_expands_tilde_against() {
    let mut linux_env = HashMap::new();
    linux_env.insert("HOME".to_string(), "/home/user".to_string());
    linux_env.insert("USERPROFILE".to_string(), "C:\\Users\\user".to_string());
    assert_eq!(
        resolve_antigravity_user_home("linux", &linux_env),
        "/home/user"
    );

    let mut win_env = HashMap::new();
    win_env.insert("HOME".to_string(), "/home/user".to_string());
    win_env.insert("USERPROFILE".to_string(), "C:\\Users\\user".to_string());
    assert_eq!(
        resolve_antigravity_user_home("win32", &win_env),
        "C:\\Users\\user"
    );

    let mut win_homedrive = HashMap::new();
    win_homedrive.insert("HOMEDRIVE".to_string(), "D:".to_string());
    win_homedrive.insert("HOMEPATH".to_string(), "\\Users\\alice".to_string());
    assert_eq!(
        resolve_antigravity_user_home("win32", &win_homedrive),
        "D:\\Users\\alice"
    );

    let mut darwin_space = HashMap::new();
    darwin_space.insert("HOME".to_string(), "/Users/a b ".to_string());
    assert_eq!(
        resolve_antigravity_user_home("darwin", &darwin_space),
        "/Users/a b "
    );

    let mut darwin_empty = HashMap::new();
    darwin_empty.insert("HOME".to_string(), "".to_string());
    assert!(!resolve_antigravity_user_home("darwin", &darwin_empty).is_empty());
}
