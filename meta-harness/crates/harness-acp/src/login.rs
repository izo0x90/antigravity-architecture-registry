//! A login the agent runs as a command of its own (ACP terminal auth): we
//! start it in the project, show the link it prints, and hand it what the
//! login page gave back on its input. It exits once signed in.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_contracts::Launch;
use harness_core::{HarnessError, Result};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::watch;

use crate::control::CommandLogin;

/// How long the command has to print its link, and to finish after it's
/// given the code.
const LINK_WAIT: Duration = Duration::from_secs(30);
const FINISH_WAIT: Duration = Duration::from_secs(120);

/// A link when the agent's settings don't say what its look like.
const ANY_LINK: &str = r"https://\S+";

/// A login command waiting for what the page gave back.
pub struct LoginRun {
    child: Child,
    stdin: Option<ChildStdin>,
    /// What it printed, both streams.
    output: Arc<Mutex<String>>,
}

/// What a started login shows: the link, if it printed one, and its last
/// words otherwise.
pub struct Started {
    pub run: LoginRun,
    pub link: Option<String>,
    /// It finished by itself (already signed in, or failed).
    pub exited: Option<bool>,
}

impl LoginRun {
    pub async fn start(launch: &Launch, login: &CommandLogin, cwd: &Path, link: Option<&str>) -> Result<Started> {
        let pattern = Regex::new(link.unwrap_or(ANY_LINK)).map_err(|e| HarnessError::InvalidConfig { reason: format!("the login link pattern is wrong: {e}") })?;
        let mut command = tokio::process::Command::new(&launch.program);
        command.args(&launch.args).args(&login.args).current_dir(cwd);
        for name in &launch.env_remove {
            command.env_remove(name);
        }
        command
            .envs(launch.env.iter().chain(&login.env).map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let output = Arc::new(Mutex::new(String::new()));
        let (found_tx, mut found) = watch::channel(None::<String>);
        let found_tx = Arc::new(found_tx);
        let streams: [Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>; 2] =
            [child.stdout.take().map(|s| Box::new(s) as _), child.stderr.take().map(|s| Box::new(s) as _)];
        for stream in streams.into_iter().flatten() {
            let (output, found_tx, pattern) = (output.clone(), found_tx.clone(), pattern.clone());
            tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(m) = pattern.find(&line) {
                        found_tx.send_if_modified(|f| f.is_none() && f.replace(m.as_str().to_owned()).is_none());
                    }
                    let mut out = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    out.push_str(&line);
                    out.push('\n');
                }
            });
        }
        let stdin = child.stdin.take();
        let mut run = Self { child, stdin, output };
        let link = tokio::select! {
            found = tokio::time::timeout(LINK_WAIT, found.wait_for(Option::is_some)) => found.ok().and_then(|f| f.ok().and_then(|l| l.clone())),
            status = run.child.wait() => return Ok(Started { exited: Some(status.is_ok_and(|s| s.success())), link: None, run }),
        };
        Ok(Started { run, link, exited: None })
    }

    /// Hands it what the login page gave back and waits for it to finish.
    pub async fn finish(mut self, response: &str) -> Result<()> {
        let mut stdin = self.stdin.take().ok_or_else(|| HarnessError::Provider { reason: "the login no longer takes input".into() })?;
        stdin.write_all(format!("{}\n", response.trim()).as_bytes()).await?;
        stdin.flush().await?;
        let status = tokio::time::timeout(FINISH_WAIT, self.child.wait())
            .await
            .map_err(|_| HarnessError::Provider { reason: "the login didn't finish".into() })??;
        if status.success() {
            Ok(())
        } else {
            Err(HarnessError::Provider { reason: format!("the login failed: {}", self.last_words()) })
        }
    }

    /// The last thing it printed.
    pub fn last_words(&self) -> String {
        let out = self.output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        out.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it said nothing").trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> (Launch, CommandLogin) {
        let launch = Launch { program: "sh".into(), args: vec!["-c".into(), script.into()], env: vec![("A".into(), "1".into())], env_remove: Vec::new() };
        (launch, CommandLogin { method: "login".into(), args: Vec::new(), env: vec![("B".into(), "2".into())] })
    }

    #[tokio::test]
    async fn the_link_is_shown_and_the_code_goes_to_its_input() {
        let dir = tempfile::tempdir().unwrap();
        let got = dir.path().join("code");
        let (launch, login) = sh(&format!(r#"echo "If the browser didn't open, visit: https://claude.ai/oauth?x=$A$B"; read code; echo "$code" > {}"#, got.display()));
        let started = LoginRun::start(&launch, &login, dir.path(), Some(r"https://\S+")).await.unwrap();
        assert_eq!(started.link.as_deref(), Some("https://claude.ai/oauth?x=12"));
        started.run.finish(" abc#state \n").await.unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap().trim(), "abc#state");
    }

    #[tokio::test]
    async fn a_failed_login_says_what_it_printed() {
        let (launch, login) = sh(r#"echo "visit https://x.example/a"; read code; echo "Invalid code" >&2; exit 1"#);
        let started = LoginRun::start(&launch, &login, Path::new("."), None).await.unwrap();
        let err = started.run.finish("nope").await.unwrap_err().to_string();
        assert!(err.contains("Invalid code"), "{err}");
    }

    #[tokio::test]
    async fn one_that_finishes_by_itself_says_how() {
        let (launch, login) = sh("echo already signed in");
        let started = LoginRun::start(&launch, &login, Path::new("."), None).await.unwrap();
        assert_eq!((started.exited, started.link), (Some(true), None));
    }
}
