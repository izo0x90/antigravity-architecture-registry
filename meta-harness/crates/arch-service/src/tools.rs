//! Validation tools: project linters, type checkers and test runners, defined
//! as data and run against components. A tool runs on a component when the
//! component binds it (its spec's `validation`, with its own targets and
//! optionally its own args) or when the tool applies to the component's type.
//!
//! Args may use placeholders: `{targets}`, `{files}`, `{symbols}` (an arg that
//! is exactly one of these expands to one arg per value; inside a longer arg
//! the values are joined with spaces), `{root}` and `{component}`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use arch_graph::{Component, ComponentId, Finding, FindingCode, Report, Target, TypeSystem};
use serde::{Deserialize, Serialize};

use crate::ServiceError;

fn default_timeout() -> u64 {
    300
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDef {
    #[serde(default)]
    pub description: String,
    /// The program to run (looked up on PATH).
    pub executable: String,
    #[serde(default)]
    pub default_args: Vec<String>,
    /// Types this tool runs on without being bound (by lineage, so a
    /// specialization of a listed type counts). Empty: only where bound.
    #[serde(default)]
    pub applies_to: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

/// How logic-step checks (shell snippets in specs) run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogicChecks {
    /// The program and leading arguments; the snippet comes last.
    pub shell: Vec<String>,
    pub timeout_secs: u64,
}

impl Default for LogicChecks {
    fn default() -> Self {
        Self { shell: vec!["sh".into(), "-c".into()], timeout_secs: 60 }
    }
}

/// The project's validation tools by name. There are no built-in tools: each
/// project declares its own (`.nah/tools.toml`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCatalog {
    #[serde(default)]
    pub tools: BTreeMap<String, ToolDef>,
    #[serde(default)]
    pub logic_checks: LogicChecks,
}

impl ToolCatalog {
    pub fn from_toml(text: &str) -> Result<Self, ServiceError> {
        let catalog: Self = toml::from_str(text).map_err(|e| ServiceError::InvalidOp(format!("tools: {e}")))?;
        if catalog.logic_checks.shell.is_empty() {
            return Err(ServiceError::InvalidOp("tools: logic_checks.shell needs a program".into()));
        }
        for (name, tool) in &catalog.tools {
            if tool.executable.trim().is_empty() {
                return Err(ServiceError::InvalidOp(format!("tool '{name}' has no executable")));
            }
        }
        Ok(catalog)
    }

    /// Adds or replaces tools by name.
    pub fn extend_with(&mut self, other: ToolCatalog) {
        self.tools.extend(other.tools);
        self.logic_checks = other.logic_checks;
    }

    pub fn get(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// stdout then stderr.
    pub output: String,
}

/// Runs one tool process. Injectable so tests and other hosts can stand in.
pub trait ToolRunner: Send + Sync {
    fn run(&self, cwd: &Path, program: &str, args: &[String], timeout: Duration) -> Result<ToolOutput, String>;
}

/// Runs tools as child processes in the repo root, killing them at the timeout.
pub struct ProcessRunner;

impl ToolRunner for ProcessRunner {
    fn run(&self, cwd: &Path, program: &str, args: &[String], timeout: Duration) -> Result<ToolOutput, String> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("can't start {program}: {e}"))?;
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let mut bytes = Vec::new();
                    let _ = pipe.read_to_end(&mut bytes);
                    text = String::from_utf8_lossy(&bytes).into_owned();
                }
                text
            })
        };
        let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
        let started = Instant::now();
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break Some(status),
                None if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let mut output = stdout.join().unwrap_or_default();
        output.push_str(&stderr.join().unwrap_or_default());
        match status {
            Some(status) => Ok(ToolOutput { success: status.success(), exit_code: status.code(), output }),
            None => Ok(ToolOutput { success: false, exit_code: None, output: format!("{output}\n(killed after {}s)", timeout.as_secs()) }),
        }
    }
}

/// One planned run of a tool for a component.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub tool: String,
    pub component: ComponentId,
    pub program: String,
    pub args: Vec<String>,
    pub timeout_secs: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRun {
    #[serde(flatten)]
    pub invocation: Invocation,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The end of the output (the part that usually says what's wrong).
    pub output: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsReport {
    pub runs: Vec<ToolRun>,
    pub report: Report,
}

const OUTPUT_TAIL: usize = 4000;

fn tail(text: &str) -> String {
    let text = text.trim_end();
    match text.char_indices().rev().nth(OUTPUT_TAIL) {
        Some((cut, _)) => format!("…{}", &text[cut..]),
        None => text.to_owned(),
    }
}

fn expand(args: &[String], values: &[(&str, Vec<String>)]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        if let Some((_, list)) = values.iter().find(|(key, _)| arg == key) {
            out.extend(list.iter().cloned());
            continue;
        }
        let mut arg = arg.clone();
        for (key, list) in values {
            arg = arg.replace(key, &list.join(" "));
        }
        out.push(arg);
    }
    out
}

/// The tool runs for `component` (as merged with its plan, so it carries its
/// real anchor). Bound tools that aren't defined are findings.
pub fn invocations(catalog: &ToolCatalog, types: &TypeSystem, component: &Component, root: &Path) -> (Vec<Invocation>, Report) {
    let mut report = Report::default();
    let mut runs = Vec::new();
    let file = component.file().map(|f| f.as_str().to_owned());
    let symbol = component.anchor.as_ref().map(|a| a.symbol.clone()).or(component.proposed.as_ref().map(|p| p.symbol.clone()));
    let mut plan_run = |name: &str, tool: &ToolDef, targets: Vec<String>, args: &[String]| {
        let values = [
            ("{targets}", targets),
            ("{files}", file.iter().cloned().collect()),
            ("{symbols}", symbol.iter().cloned().collect()),
            ("{root}", vec![root.to_string_lossy().into_owned()]),
            ("{component}", vec![component.id.to_string()]),
        ];
        runs.push(Invocation {
            tool: name.to_owned(),
            component: component.id.clone(),
            program: tool.executable.clone(),
            args: expand(args, &values),
            timeout_secs: tool.timeout_secs,
        });
    };
    let bound = component.spec.as_ref().map(|s| s.validation.as_slice()).unwrap_or_default();
    for binding in bound {
        let Some(tool) = catalog.get(&binding.tool) else {
            let known: Vec<&str> = catalog.tools.keys().map(String::as_str).collect();
            report.push(
                Finding::error(
                    FindingCode::UnknownTool,
                    Target::Component { id: component.id.clone() },
                    format!("validation tool '{}' isn't defined", binding.tool),
                )
                .suggest(format!(
                    "define it in .nah/tools.toml or use one of: {}",
                    if known.is_empty() { "(none defined)".to_owned() } else { known.join(", ") }
                )),
            );
            continue;
        };
        let targets = if binding.targets.is_empty() { file.iter().cloned().collect() } else { binding.targets.clone() };
        plan_run(&binding.tool, tool, targets, binding.args.as_deref().unwrap_or(&tool.default_args));
    }
    let lineage = types.resolve(&component.type_name).map(|t| t.lineage).unwrap_or_default();
    for (name, tool) in &catalog.tools {
        let applies = tool.applies_to.iter().any(|t| lineage.contains(t));
        if applies && !bound.iter().any(|b| &b.tool == name) && file.is_some() {
            plan_run(name, tool, file.iter().cloned().collect(), &tool.default_args);
        }
    }
    (runs, report)
}

/// Runs the invocations; each failure is an error on its component.
pub fn run(invocations: Vec<Invocation>, runner: &dyn ToolRunner, root: &Path) -> ToolsReport {
    let mut out = ToolsReport::default();
    for invocation in invocations {
        let target = Target::Component { id: invocation.component.clone() };
        let command = format!("{} {}", invocation.program, invocation.args.join(" "));
        let result = runner.run(root, &invocation.program, &invocation.args, Duration::from_secs(invocation.timeout_secs));
        let (passed, exit_code, output) = match result {
            Ok(o) => (o.success, o.exit_code, tail(&o.output)),
            Err(e) => (false, None, e),
        };
        if !passed {
            let status = exit_code.map(|c| format!("exit {c}")).unwrap_or_else(|| "didn't finish".to_owned());
            out.report.push(
                Finding::error(
                    FindingCode::ToolFailed,
                    target,
                    format!("{} failed ({status}) on {}: `{command}`\n{output}", invocation.tool, invocation.component),
                )
                .suggest("fix what the tool reports, then verify again"),
            );
        }
        out.runs.push(ToolRun { invocation, passed, exit_code, output });
    }
    out
}

/// Runs every logic step's check for `components`: one finding per check,
/// passed (info) or failed (error, with the output).
pub fn run_logic_checks<'a>(
    components: impl Iterator<Item = &'a Component>,
    config: &LogicChecks,
    runner: &dyn ToolRunner,
    root: &Path,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some((program, lead)) = config.shell.split_first() else { return out };
    for c in components {
        for step in c.spec.iter().flat_map(|s| &s.logic_steps) {
            let Some(check) = &step.check else { continue };
            let mut args = lead.to_vec();
            args.push(check.clone());
            let target = Target::Component { id: c.id.clone() };
            let label = format!("logic step {} '{}' check", step.sequence, step.name);
            out.push(match runner.run(root, program, &args, Duration::from_secs(config.timeout_secs)) {
                Ok(o) if o.success => Finding::info(FindingCode::LogicCheckPassed, target, format!("{label} passed")),
                Ok(o) => Finding::error(FindingCode::LogicCheckFailed, target, format!("{label} failed: {}", o.output.trim())),
                Err(e) => Finding::error(FindingCode::LogicCheckFailed, target, format!("{label} couldn't run: {e}")),
            });
        }
    }
    out
}
