//! An agent's process and what it starts stop together when the session
//! lets go (an emulated server's own harness included).

use std::time::{Duration, Instant};

use harness_acp::{AcpLauncher, ProcessLauncher};
use harness_contracts::{RuntimePolicy, SessionConfig, ThreadId};

fn alive(pid: &str) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists() && !std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().contains(") Z ")
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn what_the_agent_started_stops_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    // The agent starts a helper of its own, leaves a file in its temp
    // folder, then waits on its input.
    let temp_file = dir.path().join("temp.path");
    let launcher = ProcessLauncher { program: "sh".into(), args: vec!["-c".into(), format!("echo left > \"$TMPDIR/log\"; echo \"$TMPDIR\" > {}; sleep 300 & echo $! > {}; cat", temp_file.display(), pid_file.display())], env: Vec::new() };
    let config = SessionConfig { thread_id: ThreadId::new("t"), policy: RuntimePolicy::default(), model: None, resume: None, mcp_servers: Vec::new() };
    let transport = launcher.launch(&config).await.unwrap();
    let started = Instant::now();
    while !pid_file.exists() || std::fs::read_to_string(&pid_file).unwrap().trim().is_empty() {
        assert!(started.elapsed() < Duration::from_secs(10), "the helper started");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let helper = std::fs::read_to_string(&pid_file).unwrap().trim().to_owned();
    assert!(alive(&helper));
    let temp = std::path::PathBuf::from(std::fs::read_to_string(&temp_file).unwrap().trim());
    assert!(temp.join("log").exists() && temp.starts_with(std::env::temp_dir()), "its own temp folder: {}", temp.display());
    drop(transport);
    assert!(!temp.exists(), "removed with it");
    let started = Instant::now();
    while alive(&helper) {
        assert!(started.elapsed() < Duration::from_secs(10), "the helper {helper} outlived the agent");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
