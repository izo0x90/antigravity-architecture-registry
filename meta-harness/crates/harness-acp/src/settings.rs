//! What one ACP agent does its own way, as data (its harness pack's
//! `[provides.settings]`): its modes, what to tell it it can do, how it
//! reports who it's signed in as, where its skills and tool servers are
//! configured. Anything left out is the protocol's default.
//!
//! Paths and pointers are templates: `{home}`, `{project}`, and the
//! agent's own folders (`dirs`), each from an environment variable as the
//! agent will see it, else a default.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use harness_contracts::{ApprovalDecision, Launch, RuntimeMode, Scope};
use harness_core::skills::{FileRule, NameRule, OrderRule, SkillRoot, SkillRules};
use harness_core::tool_config::ToolConfigFile;
use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AcpSettings {
    /// The agent's mode for each of ours; one left out isn't offered.
    pub modes: Modes,
    /// Added to an approval option, by its decision.
    pub approval_warnings: BTreeMap<ApprovalDecision, String>,
    /// The agent may read and write the project's files through us.
    pub client_fs: bool,
    /// What we tell the agent we can do, besides files.
    pub advertise: Advertise,
    /// Added to the `_meta` of every session we open.
    pub session_meta: serde_json::Map<String, serde_json::Value>,
    /// Where `initialize`'s answer says the agent queues prompts sent while
    /// one runs (a JSON pointer; present and not false means it does).
    pub queueing: Option<String>,
    /// Where it says it takes steering (`_session/steering`).
    pub steering: Option<String>,
    /// How it reports who it's signed in as.
    pub sign_in: Option<SignInSettings>,
    /// Logins by the agent's own login command.
    pub login: LoginSettings,
    /// Questions it asks in forms.
    pub questions: QuestionSettings,
    /// Its own folders, by name (usable as `{name}` in templates).
    pub dirs: BTreeMap<String, DirSetting>,
    /// How its home (login, settings, sessions) is moved to a folder of the
    /// project's, when the project keeps it.
    pub home: Option<HomeSettings>,
    pub skills: Option<SkillSettings>,
    /// Its configuration files that list tool servers.
    pub tool_config: Vec<ToolConfigSetting>,
}

impl AcpSettings {
    /// Its settings from a profile's (none: all defaults).
    pub fn parse(value: &serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::Null => Ok(Self::default()),
            value => serde_json::from_value(value.clone()).map_err(|e| format!("its harness settings are wrong: {e}")),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Modes {
    pub approval_required: Option<String>,
    pub auto_accept_edits: Option<String>,
    pub auto: Option<String>,
    pub full_access: Option<String>,
    /// Planning first (our plan interaction mode).
    pub plan: Option<String>,
}

impl Modes {
    pub fn native(&self, mode: RuntimeMode) -> Option<&str> {
        match mode {
            RuntimeMode::ApprovalRequired => self.approval_required.as_deref(),
            RuntimeMode::AutoAcceptEdits => self.auto_accept_edits.as_deref(),
            RuntimeMode::Auto => self.auto.as_deref(),
            RuntimeMode::FullAccess => self.full_access.as_deref(),
        }
    }

    /// Ours for one of its modes: the most permissive of ours that maps to
    /// it (two of ours may share one of its).
    pub fn ours(&self, native: &str) -> Option<RuntimeMode> {
        self.offered().into_iter().rev().find(|m| self.native(*m) == Some(native))
    }

    /// Ours it has, least permissive first.
    pub fn offered(&self) -> Vec<RuntimeMode> {
        [RuntimeMode::ApprovalRequired, RuntimeMode::AutoAcceptEdits, RuntimeMode::Auto, RuntimeMode::FullAccess]
            .into_iter()
            .filter(|m| self.native(*m).is_some())
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Advertise {
    /// We run its login commands (ACP terminal auth).
    pub auth_terminal: bool,
    /// We show its questions as forms (ACP form elicitation).
    pub elicitation_form: bool,
    /// Added to our capabilities' `_meta`.
    pub meta: serde_json::Map<String, serde_json::Value>,
}

/// A notification the agent sends with who it's signed in as; the rest
/// are JSON pointers into it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignInSettings {
    pub notification: String,
    /// Where `initialize`'s answer says it sends them (it may not, then).
    pub declared: Option<String>,
    /// How long a check waits for the first one, in seconds.
    #[serde(default = "default_wait")]
    pub wait_s: u64,
    /// What it's signed in with (an account, a key), and the values of
    /// that which mean it's signed out.
    pub status: String,
    #[serde(default)]
    pub signed_out: Vec<String>,
    pub label: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub organization: Option<String>,
}

fn default_wait() -> u64 {
    6
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoginSettings {
    /// A pattern for the link its login prints (the first match is shown to
    /// open): its login command's on its output, a login it runs itself on
    /// its error output. Any `https://` link when not given.
    pub link: Option<String>,
    /// What the login page gives back to paste in, in words.
    pub expects: Option<String>,
    /// How long a login it runs itself waits for the browser, in seconds.
    pub within_s: Option<u64>,
    /// How long a login it runs itself is given to say its link, in
    /// seconds (30 when not given).
    pub wait_s: Option<u64>,
    /// Where it keeps its login (a template): there, it's signed in, for an
    /// agent that doesn't say so itself.
    pub kept: Option<String>,
    /// Signed out, it starts a login itself when a session opens (and says
    /// its link) instead of refusing: the opening is refused for it.
    pub on_session: bool,
}

impl LoginSettings {
    /// The pattern of its login's link.
    pub fn link_pattern(&self) -> Result<regex::Regex, String> {
        regex::Regex::new(self.link.as_deref().unwrap_or(r"https://\S+")).map_err(|e| format!("the login link pattern is wrong: {e}"))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuestionSettings {
    /// A field named a question's name plus this is that question's own
    /// answer box ("Other"), not a question of its own.
    pub custom_suffix: Option<String>,
    /// A permission request on a tool call whose id starts with this is a
    /// question (the harness asks its questions so): its options are the
    /// answers, and it's never answered for the user.
    pub permission_tool_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirSetting {
    /// The variable that sets it.
    pub env: String,
    /// Its template when the variable isn't set.
    pub default: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSettings {
    /// The variables that move it, each a template where `{kept}` is the
    /// folder it's kept in.
    pub env: BTreeMap<String, String>,
}

impl HomeSettings {
    /// The variables that keep it in `kept`.
    pub fn env_for(&self, kept: &Path) -> Vec<(String, String)> {
        self.env.iter().map(|(name, value)| (name.clone(), value.replace("{kept}", &kept.to_string_lossy()))).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillSettings {
    /// Where it looks, in order (the first skill of a name wins).
    pub roots: Vec<PlaceSetting>,
    #[serde(default)]
    pub name: NameRule,
    #[serde(default)]
    pub file: FileRule,
    #[serde(default)]
    pub order: OrderRule,
}

impl SkillSettings {
    pub fn rules(&self) -> SkillRules {
        SkillRules { name: self.name, file: self.file, order: self.order }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaceSetting {
    pub path: String,
    pub scope: Scope,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolConfigSetting {
    pub file: String,
    /// Where its servers are (a JSON pointer template).
    pub pointer: String,
    pub scope: Scope,
}

/// The places templates name, for one agent in one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    values: BTreeMap<String, String>,
}

impl Places {
    /// `{home}`, `{project}` and the agent's folders: each from its variable
    /// as `launch` gives it or leaves it (the server's own, unless removed),
    /// else its default.
    pub fn new(settings: &AcpSettings, project: &Path, launch: Option<&Launch>) -> Self {
        let env = |name: &str| -> Option<String> {
            if let Some(launch) = launch {
                if let Some((_, v)) = launch.env.iter().find(|(k, _)| k == name) {
                    return Some(v.clone());
                }
                if launch.env_remove.iter().any(|k| k == name) {
                    return None;
                }
            }
            std::env::var(name).ok().filter(|v| !v.is_empty())
        };
        let mut values = BTreeMap::from([
            ("home".to_owned(), env("HOME").unwrap_or_default()),
            ("project".to_owned(), project.display().to_string()),
        ]);
        // A folder's default may name `{home}` and `{project}`, not other folders.
        let base = Self { values: values.clone() };
        for (name, dir) in &settings.dirs {
            values.insert(name.clone(), env(&dir.env).unwrap_or_else(|| base.fill(&dir.default, |v| v.to_owned())));
        }
        Self { values }
    }

    pub fn path(&self, template: &str) -> PathBuf {
        PathBuf::from(self.fill(template, |v| v.to_owned()))
    }

    /// A JSON pointer: the places in it escaped as one segment each.
    pub fn pointer(&self, template: &str) -> String {
        self.fill(template, |v| v.replace('~', "~0").replace('/', "~1"))
    }

    fn fill(&self, template: &str, escape: impl Fn(&str) -> String) -> String {
        let mut out = template.to_owned();
        for (name, value) in &self.values {
            out = out.replace(&format!("{{{name}}}"), &escape(value));
        }
        out
    }

    pub fn skill_roots(&self, skills: &SkillSettings) -> Vec<SkillRoot> {
        skills.roots.iter().map(|r| SkillRoot { path: self.path(&r.path), scope: r.scope }).collect()
    }

    pub fn tool_files(&self, files: &[ToolConfigSetting]) -> Vec<ToolConfigFile> {
        files.iter().map(|f| ToolConfigFile { path: self.path(&f.file), pointer: self.pointer(&f.pointer), scope: f.scope }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_map_both_ways_and_the_most_permissive_wins_a_shared_one() {
        let modes = Modes { approval_required: Some("default".into()), auto: Some("yolo".into()), full_access: Some("yolo".into()), ..Modes::default() };
        assert_eq!(modes.native(RuntimeMode::Auto), Some("yolo"));
        assert_eq!(modes.native(RuntimeMode::AutoAcceptEdits), None);
        assert_eq!(modes.ours("yolo"), Some(RuntimeMode::FullAccess));
        assert_eq!(modes.offered(), [RuntimeMode::ApprovalRequired, RuntimeMode::Auto, RuntimeMode::FullAccess]);
    }

    #[test]
    fn places_come_from_the_agents_environment_else_their_defaults() {
        let settings: AcpSettings = toml_settings(
            r#"
            [dirs.config]
            env = "NAH_TEST_CONFIG_DIR"
            default = "{home}/.agent"
            "#,
        );
        let project = Path::new("/work/my proj");
        let mut launch = Launch { program: "a".into(), args: Vec::new(), env: Vec::new(), env_remove: vec!["NAH_TEST_CONFIG_DIR".into()] };
        let places = Places::new(&settings, project, Some(&launch));
        let home = std::env::var("HOME").unwrap_or_default();
        assert_eq!(places.path("{config}/skills"), PathBuf::from(format!("{home}/.agent/skills")));
        assert_eq!(places.pointer("/projects/{project}/mcpServers"), "/projects/~1work~1my proj/mcpServers");
        launch.env.push(("NAH_TEST_CONFIG_DIR".into(), "/cfg".into()));
        assert_eq!(Places::new(&settings, project, Some(&launch)).path("{config}/x"), PathBuf::from("/cfg/x"));
    }

    #[test]
    fn unknown_settings_are_refused() {
        let err = AcpSettings::parse(&serde_json::json!({ "mode": {} })).unwrap_err();
        assert!(err.contains("unknown field `mode`"), "{err}");
        assert_eq!(AcpSettings::parse(&serde_json::Value::Null), Ok(AcpSettings::default()));
    }

    fn toml_settings(text: &str) -> AcpSettings {
        let table: toml::Table = toml::from_str(text).unwrap();
        AcpSettings::parse(&serde_json::to_value(table).unwrap()).unwrap()
    }
}
