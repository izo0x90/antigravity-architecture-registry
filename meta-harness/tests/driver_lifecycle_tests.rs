use meta_harness::driver::*;
use meta_harness::error::HarnessError;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::{tempdir, TempDir};

struct TestControls {
    selected: AntigravityExecutable,
    fail_resolution: bool,
    startup_delay: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcquisitionRecord {
    binary_path: Option<String>,
    path: Option<String>,
}

struct TestInstallationProvider {
    controls: Arc<Mutex<TestControls>>,
    acquisitions: Arc<Mutex<Vec<AcquisitionRecord>>>,
    releases: Arc<Mutex<Vec<Option<String>>>>,
}

impl InstallationProvider for TestInstallationProvider {
    fn resolve(
        &self,
        _binary_path: Option<&str>,
        _env: &[(String, String)],
    ) -> Result<AntigravityExecutable, HarnessError> {
        let controls = self.controls.lock().unwrap();
        if controls.fail_resolution {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".to_string(),
                detail: "Fixture resolution failed.".to_string(),
            });
        }
        Ok(controls.selected.clone())
    }

    fn acquire(
        &self,
        binary_path: Option<&str>,
        env: &[(String, String)],
    ) -> Result<AntigravityExecutableLease, HarnessError> {
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone());
        self.acquisitions.lock().unwrap().push(AcquisitionRecord {
            binary_path: binary_path.map(|s| s.to_string()),
            path,
        });

        let controls = self.controls.lock().unwrap();
        if let Some(delay) = controls.startup_delay {
            std::thread::sleep(delay);
        }
        if controls.fail_resolution {
            return Err(HarnessError::ProviderSetup {
                operation: "resolve".to_string(),
                detail: "Fixture resolution failed.".to_string(),
            });
        }
        let selected = controls.selected.clone();
        let releases = self.releases.clone();
        Ok(AntigravityExecutableLease::new(selected, move |ver| {
            releases.lock().unwrap().push(ver);
        }))
    }
}

struct TestProcessHandle {
    is_running: bool,
    exit_code: Option<i32>,
    login_required: bool,
    requests: Arc<Mutex<Vec<RpcRequestRecord>>>,
}

impl DriverProcessHandle for TestProcessHandle {
    fn is_running(&self) -> bool {
        self.is_running
    }

    fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    fn close(&mut self) {
        self.is_running = false;
        self.exit_code = Some(0);
    }

    fn execute_acp(
        &mut self,
        auth_method: &AuthMethod,
        _api_key: Option<&str>,
        requests_out: &mut Vec<RpcRequestRecord>,
    ) -> Result<(), HarnessError> {
        let mut reqs = self.requests.lock().unwrap();

        let req1 = RpcRequestRecord {
            method: "initialize".to_string(),
            params: serde_json::json!({ "protocolVersion": 1 }),
        };
        reqs.push(req1.clone());
        requests_out.push(req1);

        let method_id = match auth_method {
            AuthMethod::OAuthPersonal => "oauth-personal",
            AuthMethod::GeminiApiKey => "gemini-api-key",
        };
        let req2 = RpcRequestRecord {
            method: "authenticate".to_string(),
            params: serde_json::json!({ "methodId": method_id }),
        };
        reqs.push(req2.clone());
        requests_out.push(req2);

        if self.login_required {
            return Err(HarnessError::ProviderDriver {
                detail: "Sign in to Antigravity in provider settings before refreshing models."
                    .to_string(),
            });
        }

        let req3 = RpcRequestRecord {
            method: "session/new".to_string(),
            params: serde_json::json!({ "mcpServers": [] }),
        };
        reqs.push(req3.clone());
        requests_out.push(req3);

        Ok(())
    }
}

struct TestProcessSpawner {
    launches: Arc<Mutex<Vec<ProcessLaunchRecord>>>,
    requests: Arc<Mutex<Vec<RpcRequestRecord>>>,
}

impl DriverProcessSpawner for TestProcessSpawner {
    fn spawn(
        &self,
        record: ProcessLaunchRecord,
    ) -> Result<Box<dyn DriverProcessHandle>, HarnessError> {
        self.launches.lock().unwrap().push(record.clone());
        let login_required = record.command.contains("signed-out");
        Ok(Box::new(TestProcessHandle {
            is_running: true,
            exit_code: None,
            login_required,
            requests: self.requests.clone(),
        }))
    }
}

#[derive(Default)]
struct DriverHarnessOptions {
    config: Option<AntigravityDriverConfig>,
    enabled: bool,
    path_env: Option<String>,
}

struct DriverTestHarness {
    driver: AntigravityDriver,
    _root: TempDir,
    profile_directory: PathBuf,
    instance_path: String,
    first: AntigravityExecutable,
    second: AntigravityExecutable,
    signed_out: AntigravityExecutable,
    controls: Arc<Mutex<TestControls>>,
    acquisitions: Arc<Mutex<Vec<AcquisitionRecord>>>,
    releases: Arc<Mutex<Vec<Option<String>>>>,
    launches: Arc<Mutex<Vec<ProcessLaunchRecord>>>,
    requests: Arc<Mutex<Vec<RpcRequestRecord>>>,
}

fn make_harness(options: DriverHarnessOptions) -> DriverTestHarness {
    let root = tempdir().expect("tempdir");
    let root_path = root.path();
    let instance_id = "test-instance".to_string();
    let profile_directory = root_path.join("profile");
    let instance_path = options.path_env.unwrap_or_else(|| {
        format!("{}:/usr/bin:/bin", root_path.join("instance-bin").display())
    });

    let first = AntigravityExecutable {
        executable_path: root_path
            .join("runtime-one/agy_acp_server.par")
            .to_str()
            .unwrap()
            .to_string(),
        harness_path: root_path
            .join("runtime-one/localharness_external")
            .to_str()
            .unwrap()
            .to_string(),
        source: "managed".to_string(),
        version: Some("runtime 'one".to_string()),
        managed_version_directory: Some(
            root_path.join("runtime-one").to_str().unwrap().to_string(),
        ),
    };

    let second = AntigravityExecutable {
        executable_path: root_path
            .join("runtime-two/agy_acp_server.par")
            .to_str()
            .unwrap()
            .to_string(),
        harness_path: root_path
            .join("runtime-two/localharness_external")
            .to_str()
            .unwrap()
            .to_string(),
        source: "managed".to_string(),
        version: Some("runtime two".to_string()),
        managed_version_directory: Some(
            root_path.join("runtime-two").to_str().unwrap().to_string(),
        ),
    };

    let signed_out = AntigravityExecutable {
        executable_path: root_path
            .join("runtime-signed-out/agy_acp_server.par")
            .to_str()
            .unwrap()
            .to_string(),
        harness_path: root_path
            .join("runtime-signed-out/localharness_external")
            .to_str()
            .unwrap()
            .to_string(),
        source: "managed".to_string(),
        version: Some("runtime signed-out".to_string()),
        managed_version_directory: Some(
            root_path
                .join("runtime-signed-out")
                .to_str()
                .unwrap()
                .to_string(),
        ),
    };

    let controls = Arc::new(Mutex::new(TestControls {
        selected: first.clone(),
        fail_resolution: false,
        startup_delay: None,
    }));

    let acquisitions = Arc::new(Mutex::new(Vec::new()));
    let releases = Arc::new(Mutex::new(Vec::new()));
    let launches = Arc::new(Mutex::new(Vec::new()));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let installation = Arc::new(TestInstallationProvider {
        controls: controls.clone(),
        acquisitions: acquisitions.clone(),
        releases: releases.clone(),
    });

    let spawner = Arc::new(TestProcessSpawner {
        launches: launches.clone(),
        requests: requests.clone(),
    });

    let config = options.config.unwrap_or_default();
    let enabled = options.enabled;

    let environment = vec![
        ("PATH".to_string(), instance_path.clone()),
        ("T3_ACP_ANTIGRAVITY".to_string(), "1".to_string()),
        ("GEMINI_API_KEY".to_string(), "must-not-be-used".to_string()),
        ("google_api_key".to_string(), "must-not-be-used".to_string()),
        (
            "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
            "/must-not-be-used.json".to_string(),
        ),
        (
            "GOOGLE_GENAI_USE_VERTEXAI".to_string(),
            "true".to_string(),
        ),
        ("GEMINI_HOME".to_string(), "/must-not-be-used".to_string()),
        (
            "ANTIGRAVITY_HARNESS_PATH".to_string(),
            "/must-not-be-used".to_string(),
        ),
        ("BROWSER".to_string(), "must-not-run".to_string()),
    ];

    let driver = AntigravityDriver::create(DriverCreateOptions {
        instance_id,
        display_name: "Google test account".to_string(),
        enabled,
        config,
        environment,
        profile_directory: profile_directory.clone(),
        installation,
        spawner,
    })
    .expect("driver created");

    DriverTestHarness {
        driver,
        _root: root,
        profile_directory,
        instance_path,
        first,
        second,
        signed_out,
        controls,
        acquisitions,
        releases,
        launches,
        requests,
    }
}

/// Ported from AntigravityDriver.test.ts lines 264-277:
/// "preserves the Node install message when starting a standalone provider"
#[test]
fn test_preserves_the_node_install_message_when_starting_a_standalone_provider() {
    let mut h = make_harness(DriverHarnessOptions {
        path_env: Some("".to_string()),
        ..Default::default()
    });

    let err = h.driver.refresh_models().unwrap_err();
    assert!(err.to_string().contains("Install Node.js"));
    assert_eq!(h.launches.lock().unwrap().len(), 0);
}

/// Ported from AntigravityDriver.test.ts lines 279-288:
/// "does not launch a process for a disabled instance"
#[test]
fn test_does_not_launch_a_process_for_a_disabled_instance() {
    let mut h = make_harness(DriverHarnessOptions {
        enabled: false,
        ..Default::default()
    });

    let snapshot = h.driver.probe().unwrap();
    assert_eq!(snapshot.status, "disabled");
    assert_eq!(h.acquisitions.lock().unwrap().len(), 0);
    assert_eq!(h.launches.lock().unwrap().len(), 0);
    assert!(!h.profile_directory.exists());
}

/// Ported from AntigravityDriver.test.ts lines 290-308:
/// "refreshes models after slow process startup"
#[test]
fn test_refreshes_models_after_slow_process_startup() {
    let mut h = make_harness(Default::default());
    h.controls.lock().unwrap().startup_delay = Some(Duration::from_millis(20));

    h.driver.refresh_models().expect("refresh succeeds");
    let snapshot = h.driver.get_snapshot();
    assert_eq!(snapshot.auth.status, "authenticated");
    assert!(!snapshot.models.is_empty());
}

/// Ported from AntigravityDriver.test.ts lines 310-374:
/// "refreshes a disabled instance through the selected executable and personal Google ACP"
#[test]
fn test_refreshes_a_disabled_instance_through_the_selected_executable_and_personal_google_acp() {
    let mut h = make_harness(Default::default());

    h.driver.refresh_models().expect("refresh 1");
    let snapshot = h.driver.get_snapshot();
    assert_eq!(snapshot.status, "disabled");
    assert_eq!(snapshot.auth.status, "authenticated");
    let slugs: Vec<_> = snapshot.models.iter().map(|m| m.slug.as_str()).collect();
    assert_eq!(slugs, vec!["gemini-test-low", "gemini-test-high"]);
    assert!(snapshot.models[0]
        .aliases
        .contains(&ANTIGRAVITY_DEFAULT_MODEL.to_string()));
    assert!(snapshot.models.iter().all(|m| m.is_legacy));
    let commands: Vec<_> = snapshot
        .slash_commands
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(commands, vec!["plan", "logout"]);
    assert!(snapshot.supports_text_generation);

    h.controls.lock().unwrap().selected = h.second.clone();
    h.driver.refresh_models().expect("refresh 2");

    let acquisitions = h.acquisitions.lock().unwrap().clone();
    assert_eq!(acquisitions.len(), 2);
    assert_eq!(acquisitions[0].path, Some(h.instance_path.clone()));
    assert_eq!(acquisitions[1].path, Some(h.instance_path.clone()));

    let launches = h.launches.lock().unwrap().clone();
    assert_eq!(launches.len(), 2);
    assert_eq!(launches[0].command, h.first.executable_path);
    assert_eq!(launches[1].command, h.second.executable_path);
    assert_eq!(
        launches[0].harness_path,
        Some(h.first.harness_path.clone())
    );
    assert_eq!(
        launches[1].harness_path,
        Some(h.second.harness_path.clone())
    );

    for launch in &launches {
        assert_eq!(
            launch.profile_directory,
            Some(h.profile_directory.to_str().unwrap().to_string())
        );
        assert_eq!(launch.force_file_storage, Some("1".to_string()));
        assert!(!launch.extend_env);
        assert!(launch.credential_keys.is_empty());
    }

    let releases = h.releases.lock().unwrap().clone();
    assert_eq!(
        releases,
        vec![h.first.version.clone(), h.second.version.clone()]
    );

    let requests = h.requests.lock().unwrap().clone();
    let methods: Vec<_> = requests.iter().map(|r| r.method.as_str()).collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "authenticate",
            "session/new",
            "initialize",
            "authenticate",
            "session/new"
        ]
    );

    let auth_methods: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "authenticate")
        .map(|r| r.params["methodId"].as_str().unwrap())
        .collect();
    assert_eq!(auth_methods, vec!["oauth-personal", "oauth-personal"]);
}

/// Ported from AntigravityDriver.test.ts lines 376-401:
/// "authenticates with the configured API key method and labels the account by method"
#[test]
fn test_authenticates_with_the_configured_api_key_method_and_labels_the_account_by_method() {
    let mut h = make_harness(DriverHarnessOptions {
        config: Some(AntigravityDriverConfig {
            auth_method: AuthMethod::GeminiApiKey,
            api_key: Some("fixture-gemini-key".to_string()),
        }),
        ..Default::default()
    });

    h.driver.refresh_models().expect("refresh");
    let snapshot = h.driver.get_snapshot();
    assert_eq!(snapshot.auth.status, "authenticated");
    assert_eq!(
        snapshot.auth.auth_type.as_deref(),
        Some("gemini-api-key")
    );
    assert_eq!(snapshot.auth.label.as_deref(), Some("Gemini API key"));
    assert!(!snapshot.models.is_empty());

    let launches = h.launches.lock().unwrap().clone();
    assert_eq!(
        launches[0].gemini_api_key.as_deref(),
        Some("fixture-gemini-key")
    );

    let requests = h.requests.lock().unwrap().clone();
    let auth_methods: Vec<_> = requests
        .iter()
        .filter(|r| r.method == "authenticate")
        .map(|r| r.params["methodId"].as_str().unwrap())
        .collect();
    assert_eq!(auth_methods, vec!["gemini-api-key"]);
}

/// Ported from AntigravityDriver.test.ts lines 403-410:
/// "reports the missing credential before launching a process"
#[test]
fn test_reports_the_missing_credential_before_launching_a_process() {
    let mut h = make_harness(DriverHarnessOptions {
        config: Some(AntigravityDriverConfig {
            auth_method: AuthMethod::GeminiApiKey,
            api_key: None,
        }),
        ..Default::default()
    });

    let err = h.driver.refresh_models().unwrap_err();
    assert!(err.to_string().contains("API key"));
    assert_eq!(h.launches.lock().unwrap().len(), 0);
}

/// Ported from AntigravityDriver.test.ts lines 412-430:
/// "closes refresh processes and clears account metadata when Google sign-in is required"
#[test]
fn test_closes_refresh_processes_and_clears_account_metadata_when_google_sign_in_is_required() {
    let mut h = make_harness(Default::default());

    h.driver.refresh_models().expect("refresh 1");

    h.controls.lock().unwrap().selected = h.signed_out.clone();
    let err = h.driver.refresh_models().unwrap_err();
    assert!(err.to_string().contains("Sign in to Antigravity"));

    let snapshot = h.driver.get_snapshot();
    assert_eq!(snapshot.auth.status, "unauthenticated");
    assert!(snapshot.models.is_empty());
    assert!(snapshot.slash_commands.is_empty());
    assert!(!snapshot.supports_text_generation);

    assert_eq!(h.acquisitions.lock().unwrap().len(), 2);
    assert_eq!(
        h.releases.lock().unwrap().clone(),
        vec![h.first.version.clone(), h.signed_out.version.clone()]
    );
}

/// Ported from AntigravityDriver.test.ts lines 432-454:
/// "clears account metadata when a text helper needs Google sign-in"
#[test]
fn test_clears_account_metadata_when_a_text_helper_needs_google_sign_in() {
    let mut h = make_harness(Default::default());

    h.driver.refresh_models().expect("refresh 1");

    h.controls.lock().unwrap().selected = h.signed_out.clone();
    let err = h
        .driver
        .generate_thread_title(ThreadTitleInput {
            cwd: h.profile_directory.to_str().unwrap().to_string(),
            message: "Repair Google login".to_string(),
            model: "gemini-test-low".to_string(),
        })
        .unwrap_err();
    assert!(err.to_string().contains("Sign in to Antigravity"));

    let snapshot = h.driver.get_snapshot();
    assert_eq!(snapshot.auth.status, "unauthenticated");
    assert!(snapshot.models.is_empty());
    assert!(!snapshot.supports_text_generation);

    assert_eq!(
        h.releases.lock().unwrap().clone(),
        vec![h.first.version.clone(), h.signed_out.version.clone()]
    );
}

/// Ported from AntigravityDriver.test.ts lines 456-471:
/// "keeps the previous catalog when executable resolution fails"
#[test]
fn test_keeps_the_previous_catalog_when_executable_resolution_fails() {
    let mut h = make_harness(Default::default());

    h.driver.refresh_models().expect("refresh 1");
    let before = h.driver.get_snapshot();

    h.controls.lock().unwrap().fail_resolution = true;
    let err = h.driver.refresh_models().unwrap_err();
    assert!(err.to_string().contains("previous model list is unchanged"));

    let after = h.driver.get_snapshot();
    assert_eq!(after.models, before.models);
    assert_eq!(after.auth, before.auth);

    assert_eq!(h.acquisitions.lock().unwrap().len(), 2);
    assert_eq!(
        h.releases.lock().unwrap().clone(),
        vec![h.first.version.clone()]
    );
}

/// Ported from AntigravityDriver.test.ts lines 526-535:
/// "probes through installation resolution without launching a process"
#[test]
fn test_probes_through_installation_resolution_without_launching_a_process() {
    let mut h = make_harness(DriverHarnessOptions {
        enabled: true,
        ..Default::default()
    });

    let snapshot = h.driver.probe().unwrap();
    assert!(snapshot.installed);
    assert_eq!(snapshot.version, h.first.version);
    assert_eq!(h.launches.lock().unwrap().len(), 0);
    assert_eq!(h.acquisitions.lock().unwrap().len(), 0);
}

/// Ported from AntigravityDriver.test.ts lines 196-207 and 351:
/// "blockedCredentialKeys: GEMINI_API_KEY, GOOGLE_API_KEY, GOOGLE_APPLICATION_CREDENTIALS, GOOGLE_GENAI_USE_VERTEXAI"
#[test]
fn test_blocks_sensitive_credentials_from_environment() {
    let raw_env = vec![
        ("PATH".to_string(), "/usr/bin:/bin".to_string()),
        ("T3_ACP_ANTIGRAVITY".to_string(), "1".to_string()),
        ("GEMINI_API_KEY".to_string(), "leaked-secret".to_string()),
        (
            "google_api_key".to_string(),
            "leaked-secret-lower".to_string(),
        ),
        (
            "GOOGLE_APPLICATION_CREDENTIALS".to_string(),
            "/secret.json".to_string(),
        ),
        (
            "GOOGLE_GENAI_USE_VERTEXAI".to_string(),
            "true".to_string(),
        ),
        (
            "ANTIGRAVITY_HARNESS_PATH".to_string(),
            "/harness".to_string(),
        ),
        ("BROWSER".to_string(), "firefox".to_string()),
    ];

    let config = AntigravityDriverConfig {
        auth_method: AuthMethod::OAuthPersonal,
        api_key: None,
    };

    let sanitized = sanitize_environment(&raw_env, &config);

    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("GEMINI_API_KEY")));
    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("GOOGLE_API_KEY")));
    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("GOOGLE_APPLICATION_CREDENTIALS")));
    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("GOOGLE_GENAI_USE_VERTEXAI")));
    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("ANTIGRAVITY_HARNESS_PATH")));
    assert!(!sanitized
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("BROWSER")));

    assert!(sanitized
        .iter()
        .any(|(k, v)| k == "PATH" && v == "/usr/bin:/bin"));
    assert!(sanitized
        .iter()
        .any(|(k, v)| k == "T3_ACP_ANTIGRAVITY" && v == "1"));
}

/// Ported from AntigravityDriver.test.ts lines 88-96 and 412-430:
/// "reports hidden login requests as sign-in required when Google sign-in is required"
#[test]
fn test_detects_google_oauth_stdout_prompt() {
    let sample_oauth_url = "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&redirect_uri=http%3A%2F%2F127.0.0.1%3A51234%2F&state=fixture-state";

    let prefixed_line = format!("{}{}", ANTIGRAVITY_AUTH_STDOUT_PREFIX, sample_oauth_url);
    let detected_1 = detect_auth_url(&prefixed_line);
    assert_eq!(detected_1.as_deref(), Some(sample_oauth_url));

    let embedded_line = format!("Please authenticate: '{}'", sample_oauth_url);
    let detected_2 = detect_auth_url(&embedded_line);
    assert_eq!(detected_2.as_deref(), Some(sample_oauth_url));

    let normal_line = "{\"jsonrpc\":\"2.0\",\"method\":\"session/new\"}";
    assert_eq!(detect_auth_url(normal_line), None);
}

/// Ported from AntigravityDriver.test.ts lines 376-410:
/// "authenticates with configured API key method" and "reports missing credential before launch"
#[test]
fn test_validates_and_injects_gemini_api_key() {
    let invalid_config = AntigravityDriverConfig {
        auth_method: AuthMethod::GeminiApiKey,
        api_key: None,
    };
    let validation_result = validate_credentials(&invalid_config);
    assert!(validation_result.is_err(), "Must report missing API key");

    let valid_config = AntigravityDriverConfig {
        auth_method: AuthMethod::GeminiApiKey,
        api_key: Some("fixture-gemini-key".to_string()),
    };
    assert!(validate_credentials(&valid_config).is_ok());

    let raw_env = vec![("PATH".to_string(), "/usr/bin".to_string())];
    let sanitized = sanitize_environment(&raw_env, &valid_config);

    assert!(sanitized
        .iter()
        .any(|(k, v)| k == "GEMINI_API_KEY" && v == "fixture-gemini-key"));
}

/// Ported from AntigravityDriver.test.ts lines 473-491:
/// "gives each process its own temp directory and removes it when the process closes"
#[test]
fn test_process_temp_directory_isolation_and_cleanup() {
    let base = tempdir().expect("base tempdir");
    let base_path = base.path();

    let temp1 = create_isolated_temp_dir(base_path, "run-instance1-").expect("temp1 created");
    let temp2 = create_isolated_temp_dir(base_path, "run-instance2-").expect("temp2 created");

    let path1 = temp1.path().to_path_buf();
    let path2 = temp2.path().to_path_buf();

    assert_ne!(path1, path2);
    assert_eq!(path1.parent(), Some(base_path));
    assert_eq!(path2.parent(), Some(base_path));
    assert!(path1.exists());
    assert!(path2.exists());

    drop(temp1);
    assert!(!path1.exists(), "temp1 should be removed on close/drop");
    assert!(path2.exists(), "temp2 must still exist independently");

    drop(temp2);
    assert!(!path2.exists(), "temp2 should be removed on close/drop");
}

/// Ported from AntigravityDriver.test.ts lines 493-524:
/// "removes runtime temp directories left by a previous server on create"
#[test]
fn test_sweeps_orphan_runtime_directories() {
    let base = tempdir().expect("base tempdir");
    let base_path = base.path();

    let orphan1 = base_path.join("run-orphan-1");
    let orphan2 = base_path.join("t3-antigravity-stale");
    let non_orphan = base_path.join("user-unrelated-dir");

    fs::create_dir(&orphan1).expect("orphan1 created");
    fs::create_dir(&orphan2).expect("orphan2 created");
    fs::create_dir(&non_orphan).expect("non_orphan created");

    fs::write(orphan1.join("stale.bin"), "stale data").expect("write stale file");

    let swept = sweep_orphan_temp_dirs(base_path).expect("sweep succeeded");

    assert_eq!(swept.len(), 2);
    assert!(!orphan1.exists(), "orphan1 should be purged");
    assert!(!orphan2.exists(), "orphan2 should be purged");
    assert!(
        non_orphan.exists(),
        "non-orphan directory must NOT be swept"
    );
}
