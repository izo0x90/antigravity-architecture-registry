//! What changed in the project's files (git), for agents: the files a
//! comparison changes, one file's changes, the components they touch, and
//! the timeline. Reading only; reviewers read changes this way.

use std::fmt::Write;
use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::{ArchitectureService, ServiceError};
use harness_contracts::changes::{Comparison, FileStatus, Group, LineKind};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ToolSet;
use super::schema::tool;
use nah_access::Capability;

use super::ToolDef;
use crate::mcp::text::*;

pub(crate) struct Changes {
    pub(crate) arch: Arc<ArchitectureService>,
    pub(crate) changes: Option<Arc<nah_changes::Changes>>,
    pub(crate) timeline: Option<Arc<nah_timeline::Timeline>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChangesArgs {
    /// `from..to`; the uncommitted changes when left out.
    #[serde(default)]
    changed: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    context: Option<u32>,
    #[serde(default)]
    components: bool,
    #[serde(default)]
    timeline: bool,
}

fn failed(e: impl std::fmt::Display) -> ServiceError {
    ServiceError::InvalidOp(e.to_string())
}

fn status_word(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Modified => "modified",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed => "renamed",
        FileStatus::TypeChanged => "type changed",
    }
}

impl Changes {
    /// Its tools, as agents see them (in the list's order).
    pub(crate) fn schemas(_trees: &TreeConfig) -> Vec<ToolDef> {
        vec![tool(
            Capability::ViewRead,
            "changes",
            "What changed in the project's files (git; read only). By default the uncommitted changes against the last commit, staged, unstaged and untracked kept apart; or `changed` from..to between any two of HEAD, index (the staged files), files (as they are now), rev:<commit or branch>, snapshot:<id> (from the timeline), e.g. HEAD~3..HEAD or rev:main..files. Lists the files with lines added and removed; `path` gives one file's changes as a diff; `components` the components they touch (where the index is current); `timeline` when the files changed and which sessions were working, with each turn's changes as a comparison.",
            json!({ "type": "object", "properties": {
                "changed": { "type": "string", "description": "from..to; the uncommitted changes when left out" },
                "path": { "type": "string", "description": "One file (project-relative): its changes as a diff" },
                "context": { "type": "integer", "description": "With path: unchanged lines around each change (3)" },
                "components": { "type": "boolean", "description": "The components the changes touch, with lines added and removed" },
                "timeline": { "type": "boolean", "description": "When the files changed and who was working; each turn's changes" }
            } }),
        )]
    }

    fn comparison(range: Option<&str>) -> Result<Comparison, ServiceError> {
        range.map_or_else(|| Ok(Comparison::default()), |r| r.parse().map_err(failed))
    }

    async fn files(changes: Arc<nah_changes::Changes>, comparison: Comparison) -> Result<String, ServiceError> {
        let list = tokio::task::spawn_blocking(move || changes.list(&comparison, 0, nah_changes::PAGE)).await.map_err(failed)?.map_err(failed)?;
        let mut out = format!("{}: {} files changed\n", list.comparison, list.total);
        for f in &list.files {
            let group = if list.comparison.is_uncommitted() { format!("[{}] ", f.group.title().to_lowercase()) } else { String::new() };
            let from = f.old_path.as_ref().map(|o| format!(" (from {o})")).unwrap_or_default();
            let counts = if f.binary { "binary".to_owned() } else { format!("+{} -{}", f.added, f.removed) };
            let _ = writeln!(out, "{group}{} {}{from} {counts}", status_word(f.status), f.path);
        }
        if list.total > list.files.len() {
            let _ = writeln!(out, "… and {} more", list.total - list.files.len());
        }
        out.push_str("\nNext: `path` for a file's diff, `components` for what it touches.");
        Ok(out)
    }

    async fn file(changes: Arc<nah_changes::Changes>, comparison: Comparison, path: String, context: u32) -> Result<String, ServiceError> {
        let groups = if comparison.is_uncommitted() { vec![Group::Staged, Group::Unstaged, Group::Untracked] } else { vec![Group::Changes] };
        let read = tokio::task::spawn_blocking(move || groups.into_iter().map(|g| changes.file(&comparison, &path, g, context).map(|c| (g, c))).collect::<Result<Vec<_>, _>>())
            .await
            .map_err(failed)?
            .map_err(failed)?;
        let mut out = String::new();
        for (group, file) in read.iter().filter(|(_, f)| !f.hunks.is_empty() || f.binary) {
            let _ = writeln!(out, "{} ({}):", file.path, group.title().to_lowercase());
            if file.binary {
                out.push_str("binary\n");
            }
            for h in &file.hunks {
                let _ = writeln!(out, "@@ -{},{} +{},{} @@", h.old_start, h.old_lines, h.new_start, h.new_lines);
                for l in &h.lines {
                    let mark = match l.kind {
                        LineKind::Added => '+',
                        LineKind::Removed => '-',
                        LineKind::Context => ' ',
                    };
                    let _ = writeln!(out, "{mark}{}", l.text);
                }
            }
        }
        if out.is_empty() {
            out.push_str("No changes in that file.");
        }
        Ok(out)
    }

    async fn components(&self, changes: Arc<nah_changes::Changes>, comparison: Comparison) -> Result<String, ServiceError> {
        let arch = self.arch.clone();
        let list = tokio::task::spawn_blocking(move || crate::routes::changes::component_changes(&arch, &changes, &comparison)).await.map_err(failed)?.map_err(failed)?;
        let mut out = format!("{}: {} components touched\n", list.comparison, list.components.len());
        for c in &list.components {
            let own = if c.direct { "" } else { " (in its parts)" };
            let _ = writeln!(out, "{} ({}) +{} -{}{own}", c.component, c.file, c.added, c.removed);
        }
        if !list.out_of_date.is_empty() {
            let _ = writeln!(out, "Not mapped (changed since the index or the comparison read them): {}", list.out_of_date.join(", "));
        }
        out.push_str("\nNext: inspect a component, or view with `changed` to see them in context.");
        Ok(out)
    }

    async fn timeline(timeline: Arc<nah_timeline::Timeline>) -> Result<String, ServiceError> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        let read = tokio::task::spawn_blocking(move || timeline.read(now)).await.map_err(failed)?.map_err(failed)?;
        let ago = |ms: u64| format!("{} s ago", now.saturating_sub(ms) / 1000);
        let mut out = String::from("Windows (newest first):\n");
        for w in read.windows.iter().rev() {
            let to = w.to.as_ref().map_or_else(|| "files".to_owned(), |t| format!("snapshot:{t}"));
            let who = if w.working.is_empty() { "no session".to_owned() } else { w.working.iter().map(|x| format!("{} (turn {})", if x.slot.is_empty() { &x.thread } else { &x.slot }, x.turn)).collect::<Vec<_>>().join(", ") };
            let _ = writeln!(out, "snapshot:{}..{to} from {}: {} files; working: {who}", w.from, ago(w.started_ms), w.files);
        }
        out.push_str("Turns:\n");
        for t in &read.turns {
            let alongside = if t.alongside.is_empty() { String::new() } else { format!("; alongside: {}", t.alongside.iter().map(|x| x.turn.as_str()).collect::<Vec<_>>().join(", ")) };
            let _ = writeln!(out, "turn {} (thread {}): {} files, changed {}{alongside}", t.turn, t.thread, t.files, t.comparison.range());
        }
        out.push_str("\nNext: changes with `changed` set to a window or a turn to read what changed then.");
        Ok(out)
    }
}

#[async_trait::async_trait]
impl ToolSet for Changes {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        match name {
            "changes" => {
                let a: ChangesArgs = parse(args)?;
                let changes = self.changes.clone().ok_or_else(|| ServiceError::InvalidOp("the project isn't in a git repository: no changes to read".into()))?;
                if a.timeline {
                    let timeline = self.timeline.clone().ok_or_else(|| ServiceError::InvalidOp("no timeline here".into()))?;
                    return Self::timeline(timeline).await;
                }
                let comparison = Self::comparison(a.changed.as_deref())?;
                match (a.path, a.components) {
                    (Some(path), _) => Self::file(changes, comparison, path, a.context.unwrap_or(nah_changes::CONTEXT)).await,
                    (None, true) => self.components(changes, comparison).await,
                    (None, false) => Self::files(changes, comparison).await,
                }
            }
            other => Err(ServiceError::NotFound(format!("tool '{other}'"))),
        }
    }
}
