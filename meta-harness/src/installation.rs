use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};

pub use crate::driver::{AntigravityExecutable, AntigravityExecutableLease, InstallationProvider};
use crate::error::HarnessError;

pub const RECORD_MAX_BYTES: u64 = 8 * 1024;
pub const RELEASE_RECORD: &str = ".install-complete.json";
pub const DEFAULT_DRIVER: &str = "antigravity";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInstallState {
    pub driver: String,
    pub operation_id: Option<String>,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub version: Option<String>,
    pub installed_version: Option<String>,
    pub can_remove: bool,
    pub message: Option<String>,
}

impl Default for ProviderInstallState {
    fn default() -> Self {
        Self {
            driver: DEFAULT_DRIVER.to_string(),
            operation_id: None,
            phase: "idle".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            version: None,
            installed_version: None,
            can_remove: false,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseFileRecord {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledReleaseRecord {
    pub release_id: String,
    pub version: String,
    pub executable: ReleaseFileRecord,
    pub harness: ReleaseFileRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActiveReleaseRecord {
    pub release_id: String,
}

#[derive(Clone)]
pub struct AntigravityInstallationOptions {
    pub base_dir: PathBuf,
    pub platform: Option<String>,
    pub arch: Option<String>,
    pub environment: HashMap<String, String>,
    pub supported: bool,
}

impl Default for AntigravityInstallationOptions {
    fn default() -> Self {
        Self {
            base_dir: PathBuf::new(),
            platform: None,
            arch: None,
            environment: HashMap::new(),
            supported: true,
        }
    }
}

pub struct AntigravityInstallation {
    pub managed_directory: PathBuf,
    pub versions_directory: PathBuf,
    pub active_path: PathBuf,
    pub platform: String,
    pub arch: String,
    pub executable_name: String,
    pub harness_name: String,
    pub supported: bool,
    state: Arc<RwLock<ProviderInstallState>>,
    state_tx: broadcast::Sender<ProviderInstallState>,
    leases: Arc<Mutex<HashMap<String, usize>>>,
    environment: HashMap<String, String>,
}

pub fn executable_names_for_platform(platform: &str) -> (String, String) {
    if platform == "win32" {
        ("agy_acp_server.exe".into(), "localharness_external.exe".into())
    } else {
        ("agy_acp_server.par".into(), "localharness_external".into())
    }
}

pub fn host_platform() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "win32"
    }
    #[cfg(target_os = "macos")]
    {
        "darwin"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        "unknown"
    }
}

pub fn host_arch() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "x64"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "arm64"
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        "unknown"
    }
}

impl AntigravityInstallation {
    pub fn new(options: AntigravityInstallationOptions) -> Result<Self, HarnessError> {
        let platform = options
            .platform
            .unwrap_or_else(|| host_platform().to_string());
        let arch = options
            .arch
            .unwrap_or_else(|| host_arch().to_string());
        let (executable_name, harness_name) = executable_names_for_platform(&platform);

        let managed_directory = options
            .base_dir
            .join("tools")
            .join("antigravity-acp")
            .join(format!("{platform}-{arch}"));
        let versions_directory = managed_directory.join("versions");
        let active_path = managed_directory.join("active.json");

        let can_remove = managed_directory.exists();
        let (state_tx, _) = broadcast::channel(32);

        let mut initial_state = ProviderInstallState {
            driver: DEFAULT_DRIVER.to_string(),
            operation_id: None,
            phase: "idle".to_string(),
            downloaded_bytes: 0,
            total_bytes: None,
            version: None,
            installed_version: None,
            can_remove,
            message: None,
        };

        if active_path.exists() {
            match Self::read_active_release(&active_path) {
                Ok(active) => {
                    match Self::read_completed_release(
                        &versions_directory,
                        &active.release_id,
                        &executable_name,
                        &harness_name,
                        &platform,
                    ) {
                        Ok(installed) => {
                            initial_state.installed_version = installed.version;
                        }
                        Err(_) => {
                            initial_state.phase = "failed".to_string();
                            initial_state.message = Some(
                                "The managed Antigravity runtime is incomplete. Remove it and reinstall."
                                    .to_string(),
                            );
                        }
                    }
                }
                Err(_) => {
                    initial_state.phase = "failed".to_string();
                    initial_state.message = Some(
                        "The managed Antigravity runtime is incomplete. Remove it and reinstall."
                            .to_string(),
                    );
                }
            }
        }

        let state = Arc::new(RwLock::new(initial_state));
        let leases = Arc::new(Mutex::new(HashMap::new()));

        Ok(Self {
            managed_directory,
            versions_directory,
            active_path,
            platform,
            arch,
            executable_name,
            harness_name,
            supported: options.supported,
            state,
            state_tx,
            leases,
            environment: options.environment,
        })
    }

    pub async fn state(&self) -> ProviderInstallState {
        self.state.read().await.clone()
    }

    pub fn changes(&self) -> broadcast::Receiver<ProviderInstallState> {
        self.state_tx.subscribe()
    }

    pub async fn start(&self) -> Result<ProviderInstallState, HarnessError> {
        if !self.supported {
            return Err(HarnessError::ProviderSetup {
                operation: "start".into(),
                detail: format!(
                    "Google does not publish an Antigravity runtime for {}-{}. Use a supported remote environment or a custom executable.",
                    self.platform, self.arch
                ),
            });
        }
        Ok(self.state.read().await.clone())
    }

    pub fn resolve(
        &self,
        binary_path: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<AntigravityExecutable, HarnessError> {
        let default_env = HashMap::new();
        let effective_env = env.unwrap_or(if !self.environment.is_empty() {
            &self.environment
        } else {
            &default_env
        });
        self.resolve_internal(binary_path, effective_env)
    }

    pub fn acquire(
        &self,
        binary_path: Option<&str>,
        env: Option<&HashMap<String, String>>,
    ) -> Result<AntigravityExecutableLease, HarnessError> {
        let default_env = HashMap::new();
        let effective_env = env.unwrap_or(if !self.environment.is_empty() {
            &self.environment
        } else {
            &default_env
        });
        self.acquire_internal(binary_path, effective_env)
    }

    fn read_active_release(active_path: &Path) -> Result<ActiveReleaseRecord, HarnessError> {
        let meta = fs::metadata(active_path)?;
        if !meta.is_file() || meta.len() > RECORD_MAX_BYTES {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed runtime record is invalid. Reinstall Antigravity.".into(),
            });
        }
        let contents = fs::read_to_string(active_path)?;
        let record: ActiveReleaseRecord = serde_json::from_str(&contents)?;
        if record.release_id.len() != 64
            || !record.release_id.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed runtime record is invalid. Reinstall Antigravity.".into(),
            });
        }
        Ok(record)
    }

    fn is_executable_file(path: &Path, expected_bytes: Option<u64>, platform: &str) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        if !meta.is_file() {
            return false;
        }
        if let Some(expected) = expected_bytes
            && meta.len() != expected
        {
            return false;
        }
        if platform == "win32" {
            true
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                (meta.permissions().mode() & 0o111) != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
    }

    fn read_completed_release(
        versions_directory: &Path,
        release_id: &str,
        executable_name: &str,
        harness_name: &str,
        platform: &str,
    ) -> Result<AntigravityExecutable, HarnessError> {
        let dir = versions_directory.join(release_id);
        let record_path = dir.join(RELEASE_RECORD);
        let meta = fs::metadata(&record_path).map_err(|_| HarnessError::ProviderSetup {
            operation: "resolve".into(),
            detail: "The managed Antigravity runtime is incomplete. Reinstall it.".into(),
        })?;
        if !meta.is_file() || meta.len() > RECORD_MAX_BYTES {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed runtime record is invalid. Reinstall Antigravity.".into(),
            });
        }
        let contents =
            fs::read_to_string(&record_path).map_err(|_| HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed Antigravity runtime is incomplete. Reinstall it.".into(),
            })?;
        let record: InstalledReleaseRecord =
            serde_json::from_str(&contents).map_err(|_| HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed Antigravity runtime is incomplete. Reinstall it.".into(),
            })?;

        if record.release_id != release_id
            || record.executable.name != executable_name
            || record.harness.name != harness_name
            || record.executable.bytes == 0
            || record.harness.bytes == 0
            || record.version.trim().is_empty()
        {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed Antigravity runtime is incomplete. Reinstall it.".into(),
            });
        }

        let exec_path = dir.join(executable_name);
        let harness_path = dir.join(harness_name);

        if !Self::is_executable_file(&exec_path, Some(record.executable.bytes), platform)
            || !Self::is_executable_file(&harness_path, Some(record.harness.bytes), platform)
        {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: "The managed Antigravity runtime is incomplete. Reinstall it.".into(),
            });
        }

        Ok(AntigravityExecutable {
            executable_path: exec_path.to_string_lossy().to_string(),
            harness_path: harness_path.to_string_lossy().to_string(),
            source: "managed".to_string(),
            version: Some(record.version),
            managed_version_directory: Some(dir.to_string_lossy().to_string()),
        })
    }

    fn check_managed_version_membership(
        &self,
        directory: &Path,
    ) -> Option<AntigravityExecutable> {
        let real_versions = fs::canonicalize(&self.versions_directory).ok()?;
        let parent = directory.parent()?;
        if parent != real_versions {
            return None;
        }
        let file_name = directory.file_name()?.to_str()?;
        if file_name.len() != 64 || !file_name.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let installed = Self::read_completed_release(
            &self.versions_directory,
            file_name,
            &self.executable_name,
            &self.harness_name,
            &self.platform,
        )
        .ok()?;
        Some(installed)
    }

    fn resolve_external_candidate(
        &self,
        candidate: &Path,
        source: &str,
    ) -> Option<AntigravityExecutable> {
        if !Self::is_executable_file(candidate, None, &self.platform) {
            return None;
        }
        let canonical_exec = fs::canonicalize(candidate).ok()?;
        let directory = canonical_exec.parent()?;
        let harness = directory.join(&self.harness_name);
        if !Self::is_executable_file(&harness, None, &self.platform) {
            return None;
        }
        let canonical_harness = fs::canonicalize(&harness).unwrap_or(harness);

        if let Some(installed) = self.check_managed_version_membership(directory) {
            return Some(AntigravityExecutable {
                executable_path: canonical_exec.to_string_lossy().to_string(),
                harness_path: canonical_harness.to_string_lossy().to_string(),
                source: source.to_string(),
                version: installed.version,
                managed_version_directory: installed.managed_version_directory,
            });
        }

        Some(AntigravityExecutable {
            executable_path: canonical_exec.to_string_lossy().to_string(),
            harness_path: canonical_harness.to_string_lossy().to_string(),
            source: source.to_string(),
            version: None,
            managed_version_directory: None,
        })
    }

    fn path_candidates(&self, binary: &str, env: &HashMap<String, String>) -> Vec<PathBuf> {
        let path_val = if self.platform == "win32" {
            env.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
                .map(|(_, v)| v.as_str())
                .unwrap_or("")
        } else {
            env.get("PATH").map(|s| s.as_str()).unwrap_or("")
        };
        let sep = if self.platform == "win32" { ';' } else { ':' };
        path_val
            .split(sep)
            .map(|s| s.trim().trim_matches('"'))
            .filter(|s| !s.is_empty())
            .map(|dir| Path::new(dir).join(binary))
            .collect()
    }

    pub fn resolve_internal(
        &self,
        binary_path: Option<&str>,
        env: &HashMap<String, String>,
    ) -> Result<AntigravityExecutable, HarnessError> {
        if !self.supported {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail: format!(
                    "Google does not publish an Antigravity runtime for {}-{}. Use a supported environment or a custom executable.",
                    self.platform, self.arch
                ),
            });
        }

        let trimmed = binary_path.map(|s| s.trim()).filter(|s| !s.is_empty());

        if let Some(override_path) = trimmed {
            let candidates: Vec<PathBuf> = if Path::new(override_path).is_absolute()
                || override_path.contains('/')
                || override_path.contains('\\')
            {
                vec![PathBuf::from(override_path)]
            } else {
                self.path_candidates(override_path, env)
            };

            for candidate in candidates {
                if let Some(selected) = self.resolve_external_candidate(&candidate, "override") {
                    return Ok(selected);
                }
            }
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".into(),
                detail:
                    "The custom Antigravity executable or its localharness_external sibling is missing or not executable."
                        .into(),
            });
        }

        if self.active_path.exists() {
            let active = Self::read_active_release(&self.active_path)?;
            return Self::read_completed_release(
                &self.versions_directory,
                &active.release_id,
                &self.executable_name,
                &self.harness_name,
                &self.platform,
            );
        }

        for candidate in self.path_candidates(&self.executable_name, env) {
            if let Some(selected) = self.resolve_external_candidate(&candidate, "path") {
                return Ok(selected);
            }
        }

        Err(HarnessError::ProviderSetup {
            operation: "resolve".into(),
            detail:
                "Antigravity is not installed. Install it in this environment or set a custom executable path."
                    .into(),
        })
    }

    pub fn acquire_internal(
        &self,
        binary_path: Option<&str>,
        env: &HashMap<String, String>,
    ) -> Result<AntigravityExecutableLease, HarnessError> {
        let executable = self.resolve_internal(binary_path, env)?;
        let managed_dir = executable.managed_version_directory.clone();

        if let Some(ref dir) = managed_dir {
            let mut leases = self.leases.lock().unwrap();
            *leases.entry(dir.clone()).or_insert(0) += 1;
        }

        let leases_clone = Arc::clone(&self.leases);
        let managed_dir_clone = managed_dir.clone();

        Ok(AntigravityExecutableLease::new(executable, move |_| {
            if let Some(dir) = managed_dir_clone {
                let mut leases = leases_clone.lock().unwrap();
                if let Some(count) = leases.get_mut(&dir) {
                    if *count > 1 {
                        *count -= 1;
                    } else {
                        leases.remove(&dir);
                    }
                }
            }
        }))
    }

    pub async fn remove(&self, protected_binary_paths: &[String]) -> Result<(), HarnessError> {
        let current = self.state.read().await.clone();
        if current.phase == "downloading"
            || current.phase == "extracting"
            || current.phase == "verifying"
        {
            return Err(HarnessError::ProviderSetup {
                operation: "remove".into(),
                detail:
                    "Stop Antigravity sessions and sign-in flows before removing its managed runtime."
                        .into(),
            });
        }

        {
            let leases = self.leases.lock().unwrap();
            if !leases.is_empty() {
                return Err(HarnessError::ProviderSetup {
                    operation: "remove".into(),
                    detail:
                        "Stop Antigravity sessions and sign-in flows before removing its managed runtime."
                            .into(),
                });
            }
        }

        if self.managed_directory.exists() {
            if let Ok(real_managed) = fs::canonicalize(&self.managed_directory) {
                for bin_path in protected_binary_paths {
                    let trimmed = bin_path.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(selected) = self.resolve_internal(Some(trimmed), &self.environment)
                        && selected.managed_version_directory.is_some()
                    {
                        return Err(HarnessError::ProviderSetup {
                            operation: "remove".into(),
                            detail:
                                "A provider instance has a custom path inside this managed runtime. Clear that path before removing it."
                                    .into(),
                        });
                    }
                    let candidate = fs::canonicalize(trimmed).unwrap_or_else(|_| PathBuf::from(trimmed));
                    if candidate.starts_with(&real_managed) {
                        return Err(HarnessError::ProviderSetup {
                            operation: "remove".into(),
                            detail:
                                "A provider instance has a custom path inside this managed runtime. Clear that path before removing it."
                                    .into(),
                        });
                    }
                }
            }
            fs::remove_dir_all(&self.managed_directory).map_err(|_| HarnessError::ProviderSetup {
                operation: "remove".into(),
                detail: "Could not remove the managed Antigravity runtime. Check for open processes and try again.".into(),
            })?;
        }

        {
            let mut s = self.state.write().await;
            s.operation_id = None;
            s.phase = "idle".to_string();
            s.downloaded_bytes = 0;
            s.installed_version = None;
            s.can_remove = false;
            s.message = None;
            let _ = self.state_tx.send(s.clone());
        }

        Ok(())
    }
}

impl InstallationProvider for AntigravityInstallation {
    fn resolve(
        &self,
        binary_path: Option<&str>,
        env: &[(String, String)],
    ) -> Result<AntigravityExecutable, HarnessError> {
        let env_map: HashMap<String, String> = env.iter().cloned().collect();
        self.resolve_internal(binary_path, &env_map)
    }

    fn acquire(
        &self,
        binary_path: Option<&str>,
        env: &[(String, String)],
    ) -> Result<AntigravityExecutableLease, HarnessError> {
        let env_map: HashMap<String, String> = env.iter().cloned().collect();
        self.acquire_internal(binary_path, &env_map)
    }
}

pub struct MockReleaseSpec<'a> {
    pub managed_directory: &'a Path,
    pub release_id: &'a str,
    pub version: &'a str,
    pub executable_name: &'a str,
    pub harness_name: &'a str,
    pub server_contents: &'a str,
    pub harness_contents: &'a str,
    pub active: bool,
}

pub fn write_release(spec: MockReleaseSpec<'_>) -> Result<(), HarnessError> {
    let directory = spec
        .managed_directory
        .join("versions")
        .join(spec.release_id);
    fs::create_dir_all(&directory)?;

    let exec_path = directory.join(spec.executable_name);
    let harness_path = directory.join(spec.harness_name);

    fs::write(&exec_path, spec.server_contents)?;
    fs::write(&harness_path, spec.harness_contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&exec_path, fs::Permissions::from_mode(0o755));
        let _ = fs::set_permissions(&harness_path, fs::Permissions::from_mode(0o755));
    }

    let record = InstalledReleaseRecord {
        release_id: spec.release_id.to_string(),
        version: spec.version.to_string(),
        executable: ReleaseFileRecord {
            name: spec.executable_name.to_string(),
            bytes: spec.server_contents.len() as u64,
        },
        harness: ReleaseFileRecord {
            name: spec.harness_name.to_string(),
            bytes: spec.harness_contents.len() as u64,
        },
    };
    fs::write(
        directory.join(RELEASE_RECORD),
        serde_json::to_string(&record)?,
    )?;

    if spec.active {
        let active_rec = ActiveReleaseRecord {
            release_id: spec.release_id.to_string(),
        };
        fs::write(
            spec.managed_directory.join("active.json"),
            serde_json::to_string(&active_rec)?,
        )?;
    }

    Ok(())
}
