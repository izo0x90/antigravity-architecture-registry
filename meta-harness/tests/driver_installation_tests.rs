use std::collections::HashMap;
use std::fs;
use std::path::Path;

use meta_harness::error::HarnessError;
use meta_harness::installation::{
    executable_names_for_platform, host_platform, write_release, AntigravityInstallation,
    AntigravityInstallationOptions,
};

const SERVER_CONTENTS: &str = "antigravity runtime\n";
const HARNESS_CONTENTS: &str = "local harness\n";
const PREVIOUS_VERSION: &str = "fixture-old";
const PREVIOUS_RELEASE_ID: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";

fn make_harness(
    base_dir: &Path,
    platform: Option<&str>,
    path_env: Option<&str>,
    previous: bool,
    supported: bool,
) -> AntigravityInstallation {
    let plat = platform.unwrap_or(host_platform());
    let (exec_name, harness_name) = executable_names_for_platform(plat);

    let mut env = HashMap::new();
    if let Some(p) = path_env {
        env.insert("PATH".to_string(), p.to_string());
    }

    let managed_directory = base_dir
        .join("tools")
        .join("antigravity-acp")
        .join(format!("{plat}-x64"));

    if previous {
        write_release(meta_harness::installation::MockReleaseSpec {
            managed_directory: &managed_directory,
            release_id: PREVIOUS_RELEASE_ID,
            version: PREVIOUS_VERSION,
            executable_name: &exec_name,
            harness_name: &harness_name,
            server_contents: SERVER_CONTENTS,
            harness_contents: HARNESS_CONTENTS,
            active: true,
        })
        .expect("write_release should succeed");
    }

    let options = AntigravityInstallationOptions {
        base_dir: base_dir.to_path_buf(),
        platform: Some(plat.to_string()),
        arch: Some("x64".to_string()),
        environment: env,
        supported,
    };

    AntigravityInstallation::new(options).expect("AntigravityInstallation::new should succeed")
}

async fn expect_previous_release(installation: &AntigravityInstallation) {
    let resolved = installation
        .resolve(None, None)
        .expect("should resolve managed release");
    assert_eq!(resolved.version.as_deref(), Some(PREVIOUS_VERSION));
    let expected_managed_dir = installation
        .managed_directory
        .join("versions")
        .join(PREVIOUS_RELEASE_ID);
    assert_eq!(
        resolved.managed_version_directory.as_deref(),
        Some(expected_managed_dir.to_str().unwrap())
    );
    assert_eq!(
        fs::read_to_string(&resolved.executable_path).expect("read executable"),
        SERVER_CONTENTS
    );
    assert_eq!(
        fs::read_to_string(&resolved.harness_path).expect("read harness"),
        HARNESS_CONTENTS
    );
    assert_eq!(
        installation.state().await.installed_version.as_deref(),
        Some(PREVIOUS_VERSION)
    );
}

#[tokio::test]
async fn test_honors_explicit_paths_and_reports_invalid_overrides_without_falling_back() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();

    let platform = host_platform();
    let (exec_name, harness_name) = executable_names_for_platform(platform);

    let external_directory = base_dir.join("external");
    let external_executable = external_directory.join(&exec_name);
    let external_harness = external_directory.join(&harness_name);

    fs::create_dir_all(&external_directory).unwrap();
    fs::write(&external_executable, "external server").unwrap();
    fs::write(&external_harness, "external harness").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&external_executable, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&external_harness, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let external_dir_str = external_directory.to_str().unwrap();
    let installation = make_harness(base_dir, Some(platform), Some(external_dir_str), true, true);

    expect_previous_release(&installation).await;

    let mut custom_env = HashMap::new();
    custom_env.insert("PATH".to_string(), external_dir_str.to_string());

    let resolved_managed = installation
        .resolve(None, Some(&custom_env))
        .expect("should resolve managed");
    assert_eq!(resolved_managed.source, "managed");
    assert_eq!(resolved_managed.version.as_deref(), Some(PREVIOUS_VERSION));

    let resolved_override = installation
        .resolve(Some(external_executable.to_str().unwrap()), None)
        .expect("should resolve override");
    assert_eq!(
        fs::canonicalize(&resolved_override.executable_path).unwrap(),
        fs::canonicalize(&external_executable).unwrap()
    );
    assert_eq!(resolved_override.source, "override");
    assert!(resolved_override.managed_version_directory.is_none());

    let resolved_by_name = installation
        .resolve(Some(&exec_name), Some(&custom_env))
        .expect("should resolve by name");
    assert_eq!(resolved_by_name.source, "override");

    fs::remove_file(&external_harness).unwrap();

    let err_missing_harness = installation
        .resolve(Some(external_executable.to_str().unwrap()), None)
        .unwrap_err();
    match err_missing_harness {
        HarnessError::ProviderSetup { operation, .. } => assert_eq!(operation, "resolve"),
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    let missing_path = base_dir.join("missing");
    let err_missing_exec = installation
        .resolve(Some(missing_path.to_str().unwrap()), None)
        .unwrap_err();
    match err_missing_exec {
        HarnessError::ProviderSetup { operation, .. } => assert_eq!(operation, "resolve"),
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    expect_previous_release(&installation).await;

    fs::write(&external_harness, "external harness").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&external_harness, fs::Permissions::from_mode(0o755)).unwrap();
    }

    installation.remove(&[]).await.unwrap();

    let resolved_from_path = installation
        .resolve(None, Some(&custom_env))
        .expect("should resolve from path after remove");
    assert_eq!(resolved_from_path.source, "path");
    assert_eq!(
        fs::canonicalize(&resolved_from_path.executable_path).unwrap(),
        fs::canonicalize(&external_executable).unwrap()
    );

    let isolated = make_harness(base_dir, Some(platform), None, false, true);

    let err_isolated = isolated.resolve(None, None).unwrap_err();
    match err_isolated {
        HarnessError::ProviderSetup { operation, .. } => assert_eq!(operation, "resolve"),
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    let resolved_isolated_path = isolated
        .resolve(None, Some(&custom_env))
        .expect("should resolve with env PATH");
    assert_eq!(resolved_isolated_path.source, "path");
    assert_eq!(
        fs::canonicalize(&resolved_isolated_path.executable_path).unwrap(),
        fs::canonicalize(&external_executable).unwrap()
    );

    let resolved_isolated_override = isolated
        .resolve(Some(&exec_name), Some(&custom_env))
        .expect("should resolve override with env PATH");
    assert_eq!(resolved_isolated_override.source, "override");
    assert_eq!(
        fs::canonicalize(&resolved_isolated_override.executable_path).unwrap(),
        fs::canonicalize(&external_executable).unwrap()
    );
}

#[tokio::test]
async fn test_keeps_leased_releases_available_while_new_sessions_resolve() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();

    let installation = make_harness(base_dir, None, None, true, true);

    let lease = installation
        .acquire(None, None)
        .expect("acquire should succeed");

    let remove_err = installation.remove(&[]).await.unwrap_err();
    match remove_err {
        HarnessError::ProviderSetup { operation, detail } => {
            assert_eq!(operation, "remove");
            assert!(detail.contains("Stop Antigravity sessions"));
        }
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    drop(lease);

    installation.remove(&[]).await.unwrap();
    assert!(!installation.managed_directory.exists());
}

#[tokio::test]
async fn test_removes_an_incomplete_active_release_before_reinstalling_without_a_path_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();

    let platform = host_platform();
    let (exec_name, harness_name) = executable_names_for_platform(platform);

    let installation = make_harness(base_dir, Some(platform), None, true, true);
    let previous = installation.resolve(None, None).unwrap();

    fs::remove_file(&previous.harness_path).unwrap();

    let external_directory = base_dir.join("external");
    fs::create_dir_all(&external_directory).unwrap();
    let external_exec = external_directory.join(&exec_name);
    let external_harness = external_directory.join(&harness_name);
    fs::write(&external_exec, "external server").unwrap();
    fs::write(&external_harness, "external harness").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&external_exec, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&external_harness, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let restarted = make_harness(
        base_dir,
        Some(platform),
        Some(external_directory.to_str().unwrap()),
        false,
        true,
    );

    let state = restarted.state().await;
    assert_eq!(state.phase, "failed");
    assert!(state.installed_version.is_none());
    assert!(state.can_remove);

    let resolve_err = restarted.resolve(None, None).unwrap_err();
    match resolve_err {
        HarnessError::ProviderSetup { operation, .. } => assert_eq!(operation, "resolve"),
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    restarted.remove(&[]).await.unwrap();

    let idle_state = restarted.state().await;
    assert_eq!(idle_state.phase, "idle");
    assert!(!idle_state.can_remove);
    assert!(!installation.managed_directory.exists());
}

#[tokio::test]
async fn test_blocks_removal_of_custom_managed_paths_and_leaves_external_executables_and_profiles_intact()
{
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();

    let platform = host_platform();
    let (exec_name, harness_name) = executable_names_for_platform(platform);

    let installation = make_harness(base_dir, Some(platform), None, true, true);
    let managed = installation.resolve(None, None).unwrap();

    let external_directory = base_dir.join("external");
    let profile_directory = base_dir.join("providers").join("antigravity").join("profile");
    fs::create_dir_all(&external_directory).unwrap();
    fs::create_dir_all(&profile_directory).unwrap();

    let external_exec = external_directory.join(&exec_name);
    let external_harness = external_directory.join(&harness_name);
    let profile_path = profile_directory.join("preferences.json");

    fs::write(&external_exec, "external server").unwrap();
    fs::write(&external_harness, "external harness").unwrap();
    fs::write(&profile_path, "{}").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&external_exec, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&external_harness, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let remove_blocked = installation
        .remove(std::slice::from_ref(&managed.executable_path))
        .await
        .unwrap_err();
    match remove_blocked {
        HarnessError::ProviderSetup { operation, detail } => {
            assert_eq!(operation, "remove");
            assert!(detail.contains("custom path inside this managed runtime"));
        }
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    expect_previous_release(&installation).await;

    installation
        .remove(&[external_exec.to_str().unwrap().to_string()])
        .await
        .unwrap();

    let state = installation.state().await;
    assert_eq!(state.phase, "idle");
    assert!(state.operation_id.is_none());
    assert!(state.installed_version.is_none());

    assert_eq!(
        fs::read_to_string(&external_exec).unwrap(),
        "external server"
    );
    assert_eq!(
        fs::read_to_string(&external_harness).unwrap(),
        "external harness"
    );
    assert_eq!(fs::read_to_string(&profile_path).unwrap(), "{}");
}

#[tokio::test]
async fn test_reports_unsupported_hosts_without_downloading_or_changing_state() {
    let temp = tempfile::tempdir().unwrap();
    let base_dir = temp.path();

    let installation = make_harness(base_dir, Some("darwin"), None, false, false);

    let start_err = installation.start().await.unwrap_err();
    match start_err {
        HarnessError::ProviderSetup { operation, detail } => {
            assert_eq!(operation, "start");
            assert!(detail.contains("Google does not publish an Antigravity runtime"));
        }
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    let resolve_err = installation.resolve(None, None).unwrap_err();
    match resolve_err {
        HarnessError::ProviderSetup { operation, detail } => {
            assert_eq!(operation, "resolve");
            assert!(detail.contains("Google does not publish an Antigravity runtime"));
        }
        other => panic!("expected ProviderSetup, got {other:?}"),
    }

    let state = installation.state().await;
    assert_eq!(state.phase, "idle");
    assert!(state.operation_id.is_none());
}
