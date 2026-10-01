use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::adapter::{AcpRuntime, RuntimeFactory};
use crate::protocol::StartSessionInput;
use crate::driver::{
    AuthMethod, DriverProcessHandle, DriverProcessSpawner, ProcessLaunchRecord, RpcRequestRecord,
};
use crate::error::HarnessError;
use crate::transport::{JsonRpcTransport, NativeAcpRuntime};

pub struct TokioProcessHandle {
    child: Arc<Mutex<Option<Child>>>,
    exit_code: Arc<Mutex<Option<i32>>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: Arc<Mutex<Option<ChildStdout>>>,
    stderr: Arc<Mutex<Option<ChildStderr>>>,
}

impl TokioProcessHandle {
    pub fn new(
        child: Child,
        stdin: Option<ChildStdin>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    ) -> Self {
        Self {
            child: Arc::new(Mutex::new(Some(child))),
            exit_code: Arc::new(Mutex::new(None)),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(stdout)),
            stderr: Arc::new(Mutex::new(stderr)),
        }
    }

    pub fn take_child(&self) -> Option<Child> {
        self.child.lock().unwrap().take()
    }

    pub fn take_stdio(&self) -> Option<(ChildStdin, ChildStdout, Option<ChildStderr>)> {
        let sin = self.stdin.lock().unwrap().take()?;
        let sout = self.stdout.lock().unwrap().take()?;
        let serr = self.stderr.lock().unwrap().take();
        Some((sin, sout, serr))
    }

    pub fn create_acp_runtime(
        handle: Arc<Self>,
        allowed_roots: Vec<PathBuf>,
        auth_method: &AuthMethod,
        api_key: Option<String>,
        on_auth_url: Option<Arc<dyn Fn(String) + Send + Sync>>,
        mcp_servers: Vec<serde_json::Value>,
    ) -> Result<NativeAcpRuntime, HarnessError> {
        let (sin, sout, serr) = handle.take_stdio().ok_or_else(|| {
            HarnessError::ProviderDriver {
                detail: "Stdio already taken or child process exited".to_string(),
            }
        })?;

        let transport = JsonRpcTransport::new(sin, sout, serr, allowed_roots, on_auth_url);
        let method_id = match auth_method {
            AuthMethod::OAuthPersonal => "oauth-personal",
            AuthMethod::GeminiApiKey => "gemini-api-key",
        };

        Ok(NativeAcpRuntime::with_handle(
            transport,
            method_id.to_string(),
            api_key,
            handle.clone(),
            mcp_servers,
        ))
    }
}

impl DriverProcessHandle for TokioProcessHandle {
    fn is_running(&self) -> bool {
        let mut guard = self.child.lock().unwrap();
        if let Some(ref mut child) = *guard {
            match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) => {
                    *self.exit_code.lock().unwrap() = status.code();
                    false
                }
                Err(_) => false,
            }
        } else {
            false
        }
    }

    fn exit_code(&self) -> Option<i32> {
        *self.exit_code.lock().unwrap()
    }

    fn close(&mut self) {
        let mut guard = self.child.lock().unwrap();
        if let Some(mut child) = guard.take() {
            #[cfg(unix)]
            if let Some(pid) = child.id() {
                unsafe {
                    // Send SIGKILL to the entire process group (-pid)
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
        }
    }

    fn execute_acp(
        &mut self,
        auth_method: &AuthMethod,
        api_key: Option<&str>,
        requests_out: &mut Vec<RpcRequestRecord>,
    ) -> Result<(), HarnessError> {
        let method_id = match auth_method {
            AuthMethod::OAuthPersonal => "oauth-personal",
            AuthMethod::GeminiApiKey => "gemini-api-key",
        };

        // Record canonical ACP handshake requests
        requests_out.push(RpcRequestRecord {
            method: "initialize".to_string(),
            params: serde_json::json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": { "readTextFile": true, "writeTextFile": true }
                }
            }),
        });

        let mut auth_params = serde_json::json!({
            "methodId": method_id,
        });
        if let Some(key) = api_key {
            auth_params["apiKey"] = serde_json::Value::String(key.to_string());
        }

        requests_out.push(RpcRequestRecord {
            method: "authenticate".to_string(),
            params: auth_params,
        });

        requests_out.push(RpcRequestRecord {
            method: "session/new".to_string(),
            params: serde_json::json!({
                "mcpServers": []
            }),
        });

        // If stdio is available and child is running, execute the real wire handshake
        let stdin_opt = self.stdin.lock().unwrap().take();
        let stdout_opt = self.stdout.lock().unwrap().take();
        let stderr_opt = self.stderr.lock().unwrap().take();

        if let (Some(sin), Some(sout)) = (stdin_opt, stdout_opt) {
            let transport = JsonRpcTransport::new(sin, sout, stderr_opt, vec![], None);
            let runtime = NativeAcpRuntime::new(
                transport,
                method_id.to_string(),
                api_key.map(|k| k.to_string()),
            );

            let run_handshake = async {
                runtime.start().await
            };

            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                tokio::task::block_in_place(|| handle.block_on(run_handshake))?;
            } else {
                let rt = tokio::runtime::Runtime::new().map_err(HarnessError::Io)?;
                rt.block_on(run_handshake)?;
            }
        }

        Ok(())
    }
}

#[derive(Default, Clone)]
pub struct TokioProcessSpawner;

impl TokioProcessSpawner {
    pub fn new() -> Self {
        Self
    }

    pub fn spawn_handle(
        &self,
        record: ProcessLaunchRecord,
    ) -> Result<TokioProcessHandle, HarnessError> {
        let mut cmd = Command::new(&record.command);
        cmd.args(&record.args);

        if let Some(ref cwd) = record.cwd {
            cmd.current_dir(cwd);
        }

        if !record.extend_env {
            cmd.env_clear();
            if let Ok(path) = std::env::var("PATH") {
                cmd.env("PATH", path);
            }
            if let Ok(home) = std::env::var("HOME") {
                cmd.env("HOME", home);
            }
            if let Ok(user) = std::env::var("USER") {
                cmd.env("USER", user);
            }
        }

        // Apply sanitized environment variables
        for (k, v) in &record.env {
            cmd.env(k, v);
        }

        if let Some(ref profile_dir) = record.profile_directory {
            cmd.env("GEMINI_HOME", profile_dir);
        }
        if let Some(ref harness_path) = record.harness_path {
            cmd.env("ANTIGRAVITY_HARNESS_PATH", harness_path);
        }
        if let Some(ref force_file_storage) = record.force_file_storage {
            cmd.env("AGY_ACP_FORCE_FILE_STORAGE", force_file_storage);
        }
        if let Some(ref temp_dir) = record.temp_directory {
            cmd.env("TMPDIR", temp_dir);
        }
        if let Some(ref key) = record.gemini_api_key {
            cmd.env("GEMINI_API_KEY", key);
        }

        cmd.env("BROWSER", "must-not-run");
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(HarnessError::Io)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        Ok(TokioProcessHandle::new(child, stdin, stdout, stderr))
    }
}

impl DriverProcessSpawner for TokioProcessSpawner {
    fn spawn(
        &self,
        record: ProcessLaunchRecord,
    ) -> Result<Box<dyn DriverProcessHandle>, HarnessError> {
        let handle = self.spawn_handle(record)?;
        Ok(Box::new(handle))
    }
}

/// Helper to create a production RuntimeFactory that spawns real OS child processes
/// and wraps them in a NativeAcpRuntime, with support for tool injection.
pub fn make_native_runtime_factory_with_injector(
    spawner: Arc<TokioProcessSpawner>,
    command: String,
    args: Vec<String>,
    auth_method: AuthMethod,
    api_key: Option<String>,
    injector: Option<Arc<dyn crate::injector::HarnessToolInjector>>,
    base_url_provider: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
) -> RuntimeFactory {
    Arc::new(move |input: &StartSessionInput| {
        let harness_path = Path::new(&command)
            .parent()
            .map(|p| p.join("localharness_external"))
            .filter(|p| p.exists())
            .map(|p| p.to_string_lossy().to_string());
        let mut record = ProcessLaunchRecord {
            command: command.clone(),
            args: args.clone(),
            cwd: Some(input.cwd.clone()),
            extend_env: true,
            profile_directory: None,
            harness_path,
            force_file_storage: None,
            credential_keys: vec![],
            gemini_api_key: api_key.clone(),
            temp_directory: None,
            env: vec![],
        };

        let mcp_servers = if let (Some(inj), Some(provider)) = (&injector, &base_url_provider)
            && let Some(url) = provider()
        {
            inj.prepare_launch(&url, &mut record)?;
            inj.session_mcp_servers(&url)
        } else {
            vec![]
        };

        let handle = Arc::new(spawner.spawn_handle(record)?);
        let runtime = TokioProcessHandle::create_acp_runtime(
            handle,
            vec![PathBuf::from(&input.cwd)],
            &auth_method,
            api_key.clone(),
            None,
            mcp_servers,
        )?;
        Ok(Arc::new(runtime))
    })
}

/// Helper to create a production RuntimeFactory that spawns real OS child processes
/// and wraps them in a NativeAcpRuntime.
pub fn make_native_runtime_factory(
    spawner: Arc<TokioProcessSpawner>,
    command: String,
    args: Vec<String>,
    auth_method: AuthMethod,
    api_key: Option<String>,
) -> RuntimeFactory {
    make_native_runtime_factory_with_injector(
        spawner,
        command,
        args,
        auth_method,
        api_key,
        None,
        None,
    )
}
