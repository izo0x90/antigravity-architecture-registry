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

use crate::control::CommandLogin;

/// How long a login has to finish once it's given what the page gave back.
pub(crate) const FINISH_WAIT: Duration = Duration::from_secs(120);
/// How long its output may still take to be read once it's exited.
const READ_OUT_WAIT: Duration = Duration::from_secs(2);

/// A login command waiting for what the page gave back.
pub struct LoginRun {
    child: Child,
    stdin: Option<ChildStdin>,
    /// What it printed, both streams, and the tasks reading them.
    output: Arc<Mutex<String>>,
    readers: Vec<tokio::task::JoinHandle<()>>,
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
    /// Starts it; its link is the first match of `pattern` in what it
    /// prints, within `wait`.
    pub async fn start(launch: &Launch, login: &CommandLogin, cwd: &Path, pattern: Option<Regex>, wait: Duration) -> Result<Started> {
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
        // What it prints, both streams, line by line: its link is looked
        // for there as an agent's is in its error output.
        let (says_to, said) = tokio::sync::broadcast::channel(crate::launch::SAID_HELD);
        let streams: [Option<Box<dyn tokio::io::AsyncRead + Send + Unpin>>; 2] =
            [child.stdout.take().map(|s| Box::new(s) as _), child.stderr.take().map(|s| Box::new(s) as _)];
        let mut readers = Vec::new();
        for stream in streams.into_iter().flatten() {
            let (output, says_to) = (output.clone(), says_to.clone());
            readers.push(tokio::spawn(async move {
                let mut lines = tokio::io::BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let _ = says_to.send(line.clone());
                    let mut out = output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    out.push_str(&line);
                    out.push('\n');
                }
            }));
        }
        drop(says_to);
        let stdin = child.stdin.take();
        let mut run = Self { child, stdin, output, readers };
        // The link it printed, or how it ended (it finished by itself).
        let first = tokio::select! {
            found = tokio::time::timeout(wait, crate::launch::says(said, pattern)) => Ok(found.ok().flatten()),
            status = run.child.wait() => Err(status.is_ok_and(|s| s.success())),
        };
        // Its output closed with no link: as it exits.
        let first = match first {
            Ok(None) => match tokio::time::timeout(READ_OUT_WAIT, run.child.wait()).await {
                Ok(status) => Err(status.is_ok_and(|s| s.success())),
                Err(_) => Ok(None),
            },
            first => first,
        };
        match first {
            Ok(link) => Ok(Started { run, link, exited: None }),
            Err(ok) => {
                run.read_out().await;
                Ok(Started { run, link: None, exited: Some(ok) })
            }
        }
    }

    /// Hands it what the login page gave back and waits for it to finish.
    pub async fn finish(mut self, response: &str) -> Result<()> {
        let mut stdin = self.stdin.take().ok_or_else(|| HarnessError::Provider { reason: "the sign-in no longer takes input".into() })?;
        stdin.write_all(format!("{}\n", response.trim()).as_bytes()).await?;
        stdin.flush().await?;
        let status = tokio::time::timeout(FINISH_WAIT, self.child.wait())
            .await
            .map_err(|_| HarnessError::Provider { reason: "the sign-in didn't finish".into() })??;
        self.read_out().await;
        if status.success() {
            Ok(())
        } else {
            Err(HarnessError::Provider { reason: format!("the sign-in failed: {}", self.last_words()) })
        }
    }

    /// Once it's exited: what it printed, read to the end (the streams can
    /// still hold its last words when it exits). A stream something else
    /// keeps open isn't waited on for long.
    async fn read_out(&mut self) {
        for reader in std::mem::take(&mut self.readers) {
            let _ = tokio::time::timeout(READ_OUT_WAIT, reader).await;
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

    const WAIT: Duration = Duration::from_secs(30);


    fn sh(script: &str) -> (Launch, CommandLogin) {
        let launch = Launch { program: "sh".into(), args: vec!["-c".into(), script.into()], env: vec![("A".into(), "1".into())], env_remove: Vec::new() };
        (launch, CommandLogin { method: "login".into(), args: Vec::new(), env: vec![("B".into(), "2".into())] })
    }

    #[tokio::test]
    async fn the_link_is_shown_and_the_code_goes_to_its_input() {
        let dir = tempfile::tempdir().unwrap();
        let got = dir.path().join("code");
        let (launch, login) = sh(&format!(r#"echo "If the browser didn't open, visit: https://claude.ai/oauth?x=$A$B"; read code; echo "$code" > {}"#, got.display()));
        let started = LoginRun::start(&launch, &login, dir.path(), None, WAIT).await.unwrap();
        assert_eq!(started.link.as_deref(), Some("https://claude.ai/oauth?x=12"));
        started.run.finish(" abc#state \n").await.unwrap();
        assert_eq!(std::fs::read_to_string(got).unwrap().trim(), "abc#state");
    }

    #[tokio::test]
    async fn a_failed_login_says_what_it_printed() {
        let (launch, login) = sh(r#"echo "visit https://x.example/a"; read code; echo "Invalid code" >&2; exit 1"#);
        let started = LoginRun::start(&launch, &login, Path::new("."), None, WAIT).await.unwrap();
        let err = started.run.finish("nope").await.unwrap_err().to_string();
        assert!(err.contains("Invalid code"), "{err}");
    }

    #[tokio::test]
    async fn one_that_finishes_by_itself_says_how() {
        let (launch, login) = sh("echo already signed in");
        let started = LoginRun::start(&launch, &login, Path::new("."), None, WAIT).await.unwrap();
        assert_eq!((started.exited, started.link), (Some(true), None));
    }
}
