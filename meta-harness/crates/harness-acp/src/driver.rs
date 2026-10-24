//! The driver for every ACP agent: an instance per harness pack that names
//! it, configured by that pack's profile (its settings say how the agent
//! differs) and started by its launch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use harness_contracts::{DriverKind, HarnessEntry, HarnessLaunch, InstanceId};
use harness_core::{DriverMetadata, HarnessError, ProviderDriver, ProviderInstance, Result};

use crate::instance::{AcpInstance, InstanceParts};
use crate::launch::{AcpLauncher, ProcessLauncher};
use crate::profile::Profile;
use crate::settings::AcpSettings;

pub const DRIVER_KIND: &str = "acp";

pub struct AcpDriver {
    /// The project its agents work in.
    project: PathBuf,
}

impl AcpDriver {
    pub fn new(project: PathBuf) -> Self {
        Self { project }
    }
}

/// Makes a harness's home folder when it isn't there yet, for the server's
/// user only. One that's there is used as it is. In the project, no folder
/// on the way to it may be a link (it would take the login wherever it
/// points: into git, say).
fn make_home(project: &Path, dir: &Path) -> std::io::Result<()> {
    let linked = |at: &Path| std::fs::symlink_metadata(at).is_ok_and(|m| m.file_type().is_symlink());
    let mut on_the_way = dir.ancestors().take_while(|at| at.starts_with(project) && *at != project);
    if linked(dir) || on_the_way.any(linked) {
        return Err(std::io::Error::other("a link is on the way to it: a home is a folder of its own"));
    }
    // Kept in the project, nothing in it may lead out of it either.
    if dir.starts_with(project) && dir.exists() && links_in(dir)? {
        return Err(std::io::Error::other("there's a link in it: a home kept in the project holds only its own files"));
    }
    if dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[async_trait]
impl ProviderDriver for AcpDriver {
    fn metadata(&self) -> DriverMetadata {
        DriverMetadata { kind: DriverKind::new(DRIVER_KIND), display_name: "ACP agent".into(), supports_multiple_instances: true }
    }

    /// Its config is the harness itself (profile and launch).
    fn default_config(&self, harness: &HarnessEntry) -> serde_json::Value {
        serde_json::to_value(harness).unwrap_or_default()
    }

    async fn create(&self, id: InstanceId, config: serde_json::Value) -> Result<Arc<dyn ProviderInstance>> {
        let entry: HarnessEntry = serde_json::from_value(config).map_err(|e| HarnessError::InvalidConfig { reason: e.to_string() })?;
        let launch = match &entry.launch {
            HarnessLaunch::Ready { launch } => Ok(launch.clone()),
            HarnessLaunch::Unavailable { reason, fix } => Err(HarnessLaunch::describe_unavailable(reason, fix.as_deref())),
        };
        // Wrong settings: it can't be started until they're fixed.
        let (settings, launch) = match AcpSettings::parse(&entry.profile.settings) {
            Ok(settings) => (settings, launch),
            Err(why) => (AcpSettings::default(), Err(why)),
        };
        // Given what it always gets, and its home where the project keeps it.
        let home = entry.home.clone();
        let mut launch = launch.and_then(|launch| settings.launch_with(launch, home.as_deref()));
        if let (Ok(_), Some(dir)) = (&launch, &home) {
            let made = {
                let (project, dir) = (self.project.clone(), dir.clone());
                tokio::task::spawn_blocking(move || make_home(&project, &dir)).await
            };
            if let Err(why) = made.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string())) {
                launch = Err(format!("its home folder {}: {why}", dir.display()));
            }
        }
        let profile = Arc::new(Profile::new(settings, &self.project, launch.as_ref().ok()).keeping_home(home.clone()).keeping_out(entry.private.clone()));
        let launcher = launch.map(|launch| (Arc::new(ProcessLauncher::from(&launch)) as Arc<dyn AcpLauncher>, Some(launch)));
        let instance = AcpInstance::create(InstanceParts { id, harness: entry.profile, profile, launcher });
        let first = instance.clone();
        tokio::spawn(async move {
            if let Err(err) = first.refresh().await {
                tracing::warn!(target: "harness_acp", "first check failed: {err}");
            }
        });
        Ok(instance)
    }
}

/// Whether there's a link anywhere in `dir`.
fn links_in(dir: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() || (kind.is_dir() && links_in(&entry.path())?) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_is_made_for_the_servers_user_and_never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".nah/harness/antigravity");
        make_home(dir.path(), &home).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&home).unwrap().permissions().mode() & 0o777, 0o700);
            let linked = dir.path().join(".nah/harness/claude-code");
            std::fs::create_dir_all(dir.path().join("docs")).unwrap();
            std::os::unix::fs::symlink(dir.path().join("docs"), &linked).unwrap();
            assert!(make_home(dir.path(), &linked).is_err(), "a login would land where the link points");
            // A link above it, too.
            let other = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(other.path().join(".nah")).unwrap();
            std::os::unix::fs::symlink(dir.path().join("docs"), other.path().join(".nah/harness")).unwrap();
            assert!(make_home(other.path(), &other.path().join(".nah/harness/antigravity")).is_err());
            // A link inside it.
            std::os::unix::fs::symlink(dir.path().join("docs"), home.join("antigravity-acp")).unwrap();
            assert!(make_home(dir.path(), &home).is_err());
        }
    }
}
