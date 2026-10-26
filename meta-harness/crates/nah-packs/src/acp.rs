//! The `acp` check: a harness started by its launch is sent ACP
//! `initialize` (newline-delimited JSON-RPC on stdio), and its version is
//! the one the answer's `agentInfo` gives.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use crate::engine::HarnessLaunch;

/// What the harness answered `initialize` with: its version, or why not.
pub fn version(launch: &HarnessLaunch, limit: Duration) -> Result<String, String> {
    // A temp folder of its own, removed after: what it leaves there doesn't
    // pile up with every check.
    let temp = tempfile::Builder::new().prefix("nah-check-").tempdir().map_err(|e| format!("a temp folder for the check: {e}"))?;
    let mut command = Command::new(&launch.program);
    for name in &launch.env_remove {
        command.env_remove(name);
    }
    let mut child = command
        .args(&launch.args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .envs(["TMPDIR", "TMP", "TEMP"].map(|k| (k, temp.path())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", launch.program))?;
    let request =
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": 1, "clientCapabilities": {}}});
    let sent = child.stdin.as_mut().map(|stdin| writeln!(stdin, "{request}").and_then(|()| stdin.flush()));
    let stdout = child.stdout.take();
    // What it says on stderr, the last lines kept (for why it failed); read
    // as it comes, never waited for (what it started may hold it open).
    let said: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<String>>> = Default::default();
    if let Some(err) = child.stderr.take() {
        let said = said.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let mut last = said.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                last.push_back(line);
                if last.len() > 5 {
                    last.pop_front();
                }
            }
        });
    }
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Some(stdout) = stdout else { return };
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if message.get("id") == Some(&serde_json::json!(1)) {
                let _ = tx.send(message);
                return;
            }
        }
    });
    let answer = match sent {
        Some(Ok(())) => rx.recv_timeout(limit).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => format!("no answer to ACP initialize within {} s", limit.as_secs()),
            mpsc::RecvTimeoutError::Disconnected => "it ended before answering ACP initialize".to_owned(),
        }),
        Some(Err(e)) => Err(format!("couldn't send ACP initialize: {e}")),
        None => Err("no stdin to send ACP initialize on".to_owned()),
    };
    // Closing stdin lets it end; it's stopped either way.
    drop(child.stdin.take());
    let ended = child.try_wait().ok().flatten();
    let _ = child.kill();
    let _ = child.wait();
    // A moment for its last words to arrive.
    std::thread::sleep(Duration::from_millis(100));
    let said = said.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().cloned().collect::<Vec<_>>().join(" | ");
    let answer = answer.map_err(|why| {
        let status = ended.map(|s| format!(" ({s})")).unwrap_or_default();
        if said.is_empty() { format!("{why}{status}") } else { format!("{why}{status}: {said}") }
    })?;
    if let Some(error) = answer.get("error") {
        return Err(format!("ACP initialize failed: {error}"));
    }
    answer
        .pointer("/result/agentInfo/version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("its ACP initialize answer has no agentInfo.version: {answer}"))
}

/// The version in what the harness answered, by the pack's pattern (all of
/// it when there's none).
pub fn read_version(answered: &str, pattern: Option<&str>) -> Result<String, String> {
    let Some(pattern) = pattern else { return Ok(answered.to_owned()) };
    let re = regex::Regex::new(pattern).map_err(|e| e.to_string())?;
    re.captures(answered)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_owned())
        .ok_or_else(|| format!("its version '{answered}' doesn't match {pattern}"))
}
