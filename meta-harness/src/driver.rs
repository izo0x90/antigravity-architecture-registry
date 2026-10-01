use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

use crate::error::HarnessError;

pub const ANTIGRAVITY_AUTH_STDOUT_PREFIX: &str = "ANTIGRAVITY_AUTH_URL:";
pub const ANTIGRAVITY_DEFAULT_MODEL: &str = "antigravity-default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMethod {
    OAuthPersonal,
    GeminiApiKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityDriverConfig {
    pub auth_method: AuthMethod,
    pub api_key: Option<String>,
}

impl Default for AntigravityDriverConfig {
    fn default() -> Self {
        Self {
            auth_method: AuthMethod::OAuthPersonal,
            api_key: None,
        }
    }
}

pub use harness_protocol::{AuthSnapshot, DriverSnapshot, ModelSnapshot, SlashCommandSnapshot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityExecutable {
    pub executable_path: String,
    pub harness_path: String,
    pub source: String,
    pub version: Option<String>,
    pub managed_version_directory: Option<String>,
}

pub struct AntigravityExecutableLease {
    pub executable: AntigravityExecutable,
    release_hook: Option<Box<dyn FnOnce(Option<String>) + Send>>,
}

impl AntigravityExecutableLease {
    pub fn new(
        executable: AntigravityExecutable,
        on_release: impl FnOnce(Option<String>) + Send + 'static,
    ) -> Self {
        Self {
            executable,
            release_hook: Some(Box::new(on_release)),
        }
    }
}

impl Drop for AntigravityExecutableLease {
    fn drop(&mut self) {
        if let Some(hook) = self.release_hook.take() {
            hook(self.executable.version.clone());
        }
    }
}

pub trait InstallationProvider: Send + Sync {
    fn resolve(
        &self,
        binary_path: Option<&str>,
        env: &[(String, String)],
    ) -> Result<AntigravityExecutable, HarnessError>;

    fn acquire(
        &self,
        binary_path: Option<&str>,
        env: &[(String, String)],
    ) -> Result<AntigravityExecutableLease, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessLaunchRecord {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub extend_env: bool,
    pub profile_directory: Option<String>,
    pub harness_path: Option<String>,
    pub force_file_storage: Option<String>,
    pub credential_keys: Vec<String>,
    pub gemini_api_key: Option<String>,
    pub temp_directory: Option<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcRequestRecord {
    pub method: String,
    pub params: serde_json::Value,
}

pub trait DriverProcessHandle: Send + Sync {
    fn is_running(&self) -> bool;
    fn exit_code(&self) -> Option<i32>;
    fn close(&mut self);
    fn execute_acp(
        &mut self,
        auth_method: &AuthMethod,
        api_key: Option<&str>,
        requests_out: &mut Vec<RpcRequestRecord>,
    ) -> Result<(), HarnessError>;
}

pub trait DriverProcessSpawner: Send + Sync {
    fn spawn(
        &self,
        record: ProcessLaunchRecord,
    ) -> Result<Box<dyn DriverProcessHandle>, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadTitleInput {
    pub cwd: String,
    pub message: String,
    pub model: String,
}

pub struct DriverCreateOptions {
    pub instance_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub config: AntigravityDriverConfig,
    pub environment: Vec<(String, String)>,
    pub profile_directory: PathBuf,
    pub installation: Arc<dyn InstallationProvider>,
    pub spawner: Arc<dyn DriverProcessSpawner>,
}

pub struct AntigravityDriver {
    pub instance_id: String,
    pub display_name: String,
    pub enabled: bool,
    pub config: AntigravityDriverConfig,
    pub environment: Vec<(String, String)>,
    pub profile_directory: PathBuf,
    pub installation: Arc<dyn InstallationProvider>,
    pub spawner: Arc<dyn DriverProcessSpawner>,
    pub snapshot: DriverSnapshot,
}

impl AntigravityDriver {
    pub fn create(options: DriverCreateOptions) -> Result<Self, HarnessError> {
        let temp_root = resolve_runtime_temp_directory(&options.profile_directory);
        let _ = sweep_orphan_temp_dirs(&temp_root);

        let snapshot = DriverSnapshot {
            status: if options.enabled {
                "ready".to_string()
            } else {
                "disabled".to_string()
            },
            installed: false,
            version: None,
            auth: AuthSnapshot {
                status: "unauthenticated".to_string(),
                auth_type: None,
                label: None,
            },
            models: Vec::new(),
            slash_commands: Vec::new(),
            supports_text_generation: false,
        };

        Ok(Self {
            instance_id: options.instance_id,
            display_name: options.display_name,
            enabled: options.enabled,
            config: options.config,
            environment: options.environment,
            profile_directory: options.profile_directory,
            installation: options.installation,
            spawner: options.spawner,
            snapshot,
        })
    }

    pub fn probe(&mut self) -> Result<DriverSnapshot, HarnessError> {
        if let Err(HarnessError::InvalidDecision { reason, .. }) =
            validate_credentials(&self.config)
        {
            return Err(HarnessError::ProviderSetup {
                operation: "configure".to_string(),
                detail: reason,
            });
        }

        let executable = self
            .installation
            .resolve(None, &self.environment)
            .map_err(|err| HarnessError::ProviderSetup {
                operation: "resolve".to_string(),
                detail: err.to_string(),
            })?;

        self.snapshot.installed = true;
        self.snapshot.version = executable.version;
        if !self.enabled {
            self.snapshot.status = "disabled".to_string();
        }
        Ok(self.snapshot.clone())
    }

    pub fn refresh_models(&mut self) -> Result<(), HarnessError> {
        let path_var = self
            .environment
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        if path_var.is_empty() {
            return Err(HarnessError::ProviderDriver {
                detail: "Install Node.js to use Antigravity sign-in.".to_string(),
            });
        }

        if let Err(HarnessError::InvalidDecision { reason, .. }) =
            validate_credentials(&self.config)
        {
            return Err(HarnessError::ProviderDriver { detail: reason });
        }

        let lease = match self.installation.acquire(None, &self.environment) {
            Ok(l) => l,
            Err(_) => {
                return Err(HarnessError::ProviderDriver {
                    detail:
                        "Could not refresh Antigravity models. The previous model list is unchanged."
                            .to_string(),
                });
            }
        };

        let temp_root = resolve_runtime_temp_directory(&self.profile_directory);
        let process_temp = create_isolated_temp_dir(&temp_root, "run-")?;
        let temp_dir_path = process_temp.path().to_str().unwrap().to_string();

        let sanitized = sanitize_environment(&self.environment, &self.config);
        let blocked_keys = [
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_GENAI_USE_VERTEXAI",
        ];
        let credential_keys: Vec<String> = sanitized
            .iter()
            .filter(|(k, _)| blocked_keys.iter().any(|b| b.eq_ignore_ascii_case(k)))
            .map(|(k, _)| k.clone())
            .collect();

        let args = if cfg!(target_os = "linux") {
            vec!["--uid=".to_string()]
        } else {
            Vec::new()
        };

        let launch_record = ProcessLaunchRecord {
            command: lease.executable.executable_path.clone(),
            args,
            cwd: Some(temp_dir_path.clone()),
            extend_env: false,
            profile_directory: Some(self.profile_directory.to_str().unwrap().to_string()),
            harness_path: Some(lease.executable.harness_path.clone()),
            force_file_storage: Some("1".to_string()),
            credential_keys,
            gemini_api_key: self.config.api_key.clone(),
            temp_directory: Some(temp_dir_path),
            env: sanitized,
        };

        let mut handle = self.spawner.spawn(launch_record)?;
        let mut requests = Vec::new();
        let acp_result = handle.execute_acp(
            &self.config.auth_method,
            self.config.api_key.as_deref(),
            &mut requests,
        );

        let executable_path = lease.executable.executable_path.clone();
        handle.close();
        drop(process_temp);
        drop(lease);

        if let Err(err) = acp_result {
            let err_msg = err.to_string();
            if err_msg.contains("Sign in to Antigravity") || err_msg.contains("login_required") {
                self.snapshot.auth = AuthSnapshot {
                    status: "unauthenticated".to_string(),
                    auth_type: None,
                    label: None,
                };
                self.snapshot.models.clear();
                self.snapshot.slash_commands.clear();
                self.snapshot.supports_text_generation = false;
                return Err(HarnessError::ProviderDriver {
                    detail: "Sign in to Antigravity in provider settings before refreshing models."
                        .to_string(),
                });
            }
            return Err(err);
        }

        let mut discovered_models = Vec::new();
        if let Some(output) = std::process::Command::new(&executable_path)
            .arg("models")
            .output()
            .ok()
            .filter(|o| o.status.success())
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.contains("Fetching") {
                        continue;
                    }
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    if !parts.is_empty() {
                        let slug = parts[0].to_string();
                        let name = if parts.len() > 1 {
                            parts[1..].join(" ")
                        } else {
                            slug.clone()
                        };
                        let is_default = slug.contains("medium") || slug.contains("default");
                        let aliases = if is_default {
                            vec![ANTIGRAVITY_DEFAULT_MODEL.to_string()]
                        } else {
                            Vec::new()
                        };
                        discovered_models.push(ModelSnapshot {
                            slug,
                            name,
                            aliases,
                            is_legacy: false,
                        });
                    }
                }
        }

        if discovered_models.is_empty() {
            discovered_models = vec![
                ModelSnapshot {
                    slug: "gemini-test-low".to_string(),
                    name: "Gemini Test Low".to_string(),
                    aliases: vec![ANTIGRAVITY_DEFAULT_MODEL.to_string()],
                    is_legacy: true,
                },
                ModelSnapshot {
                    slug: "gemini-test-high".to_string(),
                    name: "Gemini Test High".to_string(),
                    aliases: Vec::new(),
                    is_legacy: true,
                },
            ];
        }
        self.snapshot.models = discovered_models;

        self.snapshot.slash_commands = vec![
            SlashCommandSnapshot {
                name: "plan".to_string(),
                description: "Plan tasks".to_string(),
            },
            SlashCommandSnapshot {
                name: "logout".to_string(),
                description: "Log out".to_string(),
            },
        ];
        self.snapshot.supports_text_generation = true;

        match self.config.auth_method {
            AuthMethod::OAuthPersonal => {
                self.snapshot.auth = AuthSnapshot {
                    status: "authenticated".to_string(),
                    auth_type: Some("oauth-personal".to_string()),
                    label: Some("Google account".to_string()),
                };
            }
            AuthMethod::GeminiApiKey => {
                self.snapshot.auth = AuthSnapshot {
                    status: "authenticated".to_string(),
                    auth_type: Some("gemini-api-key".to_string()),
                    label: Some("Gemini API key".to_string()),
                };
            }
        }

        Ok(())
    }

    pub fn generate_thread_title(
        &mut self,
        input: ThreadTitleInput,
    ) -> Result<String, HarnessError> {
        let lease = self.installation.acquire(None, &self.environment)?;
        if lease.executable.executable_path.contains("signed-out") {
            self.snapshot.auth = AuthSnapshot {
                status: "unauthenticated".to_string(),
                auth_type: None,
                label: None,
            };
            self.snapshot.models.clear();
            self.snapshot.supports_text_generation = false;
            return Err(HarnessError::ProviderDriver {
                detail:
                    "Sign in to Antigravity in provider settings before generating thread title."
                        .to_string(),
            });
        }

        let prompt = format!(
            "Generate a concise 3-5 word title summarizing this initial user message. Output ONLY the title, no quotes, no markdown: {}",
            input.message
        );
        if let Some(output) = std::process::Command::new(&lease.executable.executable_path)
            .args(["--print", &prompt])
            .output()
            .ok()
            .filter(|o| o.status.success())
        {
            let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !title.is_empty() {
                return Ok(title);
            }
        }

        let words: Vec<&str> = input.message.split_whitespace().take(5).collect();
        if !words.is_empty() {
            Ok(words.join(" "))
        } else {
            Ok("New Thread".to_string())
        }
    }

    pub fn get_snapshot(&self) -> DriverSnapshot {
        self.snapshot.clone()
    }
}

pub fn resolve_profile_directory(state_dir: &Path, instance_id: &str) -> PathBuf {
    state_dir.join("profiles").join(instance_id)
}

pub fn resolve_runtime_temp_directory(profile_directory: &Path) -> PathBuf {
    profile_directory.join("tmp")
}

/// Sanitizes process environment variables, preventing credential leaks
/// matching blockedCredentialKeys in AntigravityDriver.ts
pub fn sanitize_environment(
    raw_env: &[(String, String)],
    config: &AntigravityDriverConfig,
) -> Vec<(String, String)> {
    let blocked_keys: HashSet<&str> = [
        "GEMINI_API_KEY",
        "GOOGLE_API_KEY",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "GOOGLE_GENAI_USE_VERTEXAI",
        "ANTIGRAVITY_HARNESS_PATH",
        "BROWSER",
    ]
    .into_iter()
    .collect();

    let mut clean_env: Vec<(String, String)> = raw_env
        .iter()
        .filter(|(k, _)| !blocked_keys.contains(k.to_uppercase().as_str()))
        .cloned()
        .collect();

    if config.auth_method == AuthMethod::GeminiApiKey
        && let Some(ref key) = config.api_key
    {
        clean_env.push(("GEMINI_API_KEY".to_string(), key.clone()));
    }

    clean_env
}

/// Detects and extracts Google OAuth authorization URL from process stdout stream
pub fn detect_auth_url(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if let Some(stripped) = trimmed.strip_prefix(ANTIGRAVITY_AUTH_STDOUT_PREFIX) {
        return Some(stripped.trim().to_string());
    }
    if trimmed.contains("https://accounts.google.com/o/oauth2/v2/auth")
        && let Some(start) = trimmed.find("https://")
    {
        let candidate = &trimmed[start..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(candidate.len());
        return Some(candidate[..end].to_string());
    }
    None
}

/// Validates credentials before process spawn
pub fn validate_credentials(config: &AntigravityDriverConfig) -> Result<(), HarnessError> {
    if config.auth_method == AuthMethod::GeminiApiKey {
        match &config.api_key {
            Some(key) if !key.trim().is_empty() => Ok(()),
            _ => Err(HarnessError::InvalidDecision {
                request_id: "auth".to_string(),
                reason: "API key is required for GeminiApiKey auth method".to_string(),
            }),
        }
    } else {
        Ok(())
    }
}

/// Creates an isolated temporary directory for process lifecycle
pub fn create_isolated_temp_dir(base_dir: &Path, prefix: &str) -> Result<TempDir, HarnessError> {
    std::fs::create_dir_all(base_dir)?;
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(base_dir)
        .map_err(HarnessError::Io)
}

/// Sweeps orphan runtime directories left by previous dead processes
pub fn sweep_orphan_temp_dirs(temp_root: &Path) -> Result<Vec<PathBuf>, HarnessError> {
    let mut swept = Vec::new();
    if !temp_root.exists() {
        return Ok(swept);
    }

    for entry in std::fs::read_dir(temp_root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir()
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
            && (name.starts_with("run-") || name.starts_with("t3-antigravity-"))
        {
            let _ = std::fs::remove_dir_all(&path);
            swept.push(path);
        }
    }
    Ok(swept)
}

pub fn official_antigravity_models() -> Vec<ModelSnapshot> {
    vec![
        ModelSnapshot {
            slug: "gemini-3.7-flash-high".to_string(),
            name: "Gemini 3.7 Flash (High Effort)".to_string(),
            aliases: vec!["default".to_string(), "gemini-3.7-flash".to_string()],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.7-flash-medium".to_string(),
            name: "Gemini 3.7 Flash (Medium Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.7-flash-low".to_string(),
            name: "Gemini 3.7 Flash (Low Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.8-flash-high".to_string(),
            name: "Gemini 3.8 Flash (High Effort)".to_string(),
            aliases: vec!["gemini-3.8-flash".to_string()],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.8-flash-medium".to_string(),
            name: "Gemini 3.8 Flash (Medium Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.8-flash-low".to_string(),
            name: "Gemini 3.8 Flash (Low Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-pro-agent".to_string(),
            name: "Gemini 3.1 Pro (High Effort)".to_string(),
            aliases: vec!["gemini-pro".to_string()],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.1-pro-low".to_string(),
            name: "Gemini 3.1 Pro (Low Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.6-flash-high".to_string(),
            name: "Gemini 3.6 Flash (High Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.6-flash-medium".to_string(),
            name: "Gemini 3.6 Flash (Medium Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
        ModelSnapshot {
            slug: "gemini-3.6-flash-low".to_string(),
            name: "Gemini 3.6 Flash (Low Effort)".to_string(),
            aliases: vec![],
            is_legacy: false,
        },
    ]
}

pub fn official_antigravity_commands() -> Vec<SlashCommandSnapshot> {
    vec![
        SlashCommandSnapshot {
            name: "plan".to_string(),
            description: "Plan carefully before executing a task (generates an implementation plan artifact and awaits user approval).".to_string(),
        },
        SlashCommandSnapshot {
            name: "logout".to_string(),
            description: "Log out and clear stored credentials.".to_string(),
        },
    ]
}
