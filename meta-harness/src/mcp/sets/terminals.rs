//! The user's terminals, for agents: which there are, what one shows, and
//! (only for a role granted `shell.drive`) typing into one.

use std::sync::Arc;

use arch_graph::tree::TreeConfig;
use arch_service::ServiceError;
use harness_contracts::{Mods, TermKey, TerminalId, TerminalInput, TerminalStatus};
use serde_json::{Value, json};

use super::schema::tool;
use super::{ToolDef, ToolSet};
use nah_access::Capability;

pub(crate) struct Terminals {
    /// None: the server runs no shells.
    pub(crate) terminals: Option<Arc<crate::terminals::Terminals>>,
}

/// How much scrollback `terminal_read` gives at most, and how many of the
/// last commands it names.
const MOST_BACK: u64 = 2000;
const LAST_COMMANDS: usize = 10;

impl Terminals {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        vec![
            tool(
                Capability::ViewRead,
                "terminals",
                "The user's terminals (shells the server runs): each one's id, title, whether its shell runs, its folder and size.",
                json!({ "type": "object", "properties": {} }),
            ),
            tool(
                Capability::ViewRead,
                "terminal_read",
                "What a terminal shows now, as text: its screen, with `back` lines of scrollback above it (default 0, at most 2000; `numbered`: each row with its line's number, as `tag` names lines), and the last commands its shell ran (each with its number in this start of the shell, its exit code and how long it ran; bash and zsh say). With `command` (a number), that command and its output instead (its last 2000 lines at most).",
                json!({ "type": "object", "properties": { "terminal": { "type": "string" }, "back": { "type": "integer", "minimum": 0 }, "numbered": { "type": "boolean" }, "command": { "type": "integer", "minimum": 1 } }, "required": ["terminal"] }),
            ),
            tool(
                Capability::ShellDrive,
                "terminal_type",
                "Type into one of the user's terminals: `text` as typed, then `keys` in order, space separated (enter, tab, escape, backspace, up, down, left, right, home, end, pageup, pagedown, delete, f1-f12; modifiers joined by +: ctrl+c, alt+b). Read it again after: output takes a moment.",
                json!({ "type": "object", "properties": { "terminal": { "type": "string" }, "text": { "type": "string" }, "keys": { "type": "string" } }, "required": ["terminal"] }),
            ),
        ]
    }

    fn terminals(&self) -> Result<&Arc<crate::terminals::Terminals>, ServiceError> {
        self.terminals
            .as_ref()
            .ok_or_else(|| ServiceError::InvalidOp("this server runs no shells (it was started with --no-shells)".into()))
    }
}

/// One key by its name (`enter`, `ctrl+c`, `alt+b`, `f5`).
fn key(name: &str) -> Result<TerminalInput, String> {
    let mut mods = Mods::default();
    let mut parts: Vec<&str> = name.split('+').collect();
    let last = parts.pop().filter(|k| !k.is_empty()).ok_or_else(|| format!("{name:?}: no key"))?;
    for m in parts {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" | "meta" => mods.alt = true,
            "shift" => mods.shift = true,
            other => return Err(format!("{name:?}: {other:?} isn't a modifier (ctrl, alt, shift)")),
        }
    }
    let lower = last.to_ascii_lowercase();
    let key = match lower.as_str() {
        "enter" | "return" => TermKey::Enter,
        "tab" => TermKey::Tab,
        "backspace" => TermKey::Backspace,
        "escape" | "esc" => TermKey::Escape,
        "up" => TermKey::Up,
        "down" => TermKey::Down,
        "left" => TermKey::Left,
        "right" => TermKey::Right,
        "home" => TermKey::Home,
        "end" => TermKey::End,
        "pageup" => TermKey::PageUp,
        "pagedown" => TermKey::PageDown,
        "insert" => TermKey::Insert,
        "delete" => TermKey::Delete,
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => {
            TermKey::F(f[1..].parse().unwrap_or(1))
        }
        _ if last.chars().count() == 1 => TermKey::Char(last.chars().next().unwrap_or(' ')),
        _ => return Err(format!("{name:?} isn't a key this knows")),
    };
    Ok(TerminalInput::Key { key, mods })
}

#[async_trait::async_trait]
impl ToolSet for Terminals {
    fn tools(&self, trees: &TreeConfig) -> Vec<ToolDef> {
        Self::schemas(trees)
    }

    async fn call(&self, name: &str, args: Value, _caller: &crate::mcp::McpCaller) -> Result<String, ServiceError> {
        let terminals = self.terminals()?;
        let id = || {
            args["terminal"]
                .as_str()
                .map(TerminalId::new)
                .ok_or_else(|| ServiceError::InvalidOp("`terminal` (its id, from `terminals`) is needed".into()))
        };
        let missing = |e: crate::terminals::TerminalError| ServiceError::InvalidOp(e.to_string());
        match name {
            "terminals" => {
                let list = terminals.list();
                if list.is_empty() {
                    return Ok("No terminals open (the user opens them in the cockpit).".into());
                }
                Ok(list
                    .iter()
                    .map(|t| {
                        let status = match t.status {
                            TerminalStatus::Running => "running".to_owned(),
                            TerminalStatus::Ended { code: Some(code) } => format!("ended ({code})"),
                            TerminalStatus::Ended { code: None } => "ended".to_owned(),
                        };
                        format!("{} · {} · {status} · {} ({}×{})", t.id, t.title, t.folder, t.size.cols, t.size.rows)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "terminal_read" => {
                let open = terminals.get(&id()?).map_err(missing)?;
                // Reading takes the terminal's lock (while its output is fed): off the async threads.
                tokio::task::spawn_blocking(move || read(&open, &args)).await.map_err(|e| ServiceError::InvalidOp(e.to_string()))?
            }
            "terminal_type" => {
                let open = terminals.get(&id()?).map_err(missing)?;
                let mut inputs = Vec::new();
                if let Some(text) = args["text"].as_str().filter(|t| !t.is_empty()) {
                    inputs.push(TerminalInput::Text { text: text.to_owned() });
                }
                for name in args["keys"].as_str().unwrap_or_default().split_whitespace() {
                    inputs.push(key(name).map_err(ServiceError::InvalidOp)?);
                }
                if inputs.is_empty() {
                    return Err(ServiceError::InvalidOp("nothing to type: give `text`, `keys`, or both".into()));
                }
                tokio::task::spawn_blocking(move || inputs.iter().try_for_each(|i| open.input(i)))
                    .await
                    .map_err(|e| ServiceError::InvalidOp(e.to_string()))?
                    .map_err(missing)?;
                Ok("Typed. terminal_read shows what it did (output takes a moment).".into())
            }
            _ => Err(ServiceError::NotFound(format!("tool '{name}'"))),
        }
    }
}

/// `terminal_read` of `open`: its screen (with scrollback), or one
/// command's output; its last commands.
fn read(open: &crate::terminals::Open, args: &Value) -> Result<String, ServiceError> {
    let commands = open.commands();
    if let Some(number) = args["command"].as_u64() {
        let no_such =
            || ServiceError::InvalidOp(format!("there's no command {number} (kept while its lines are: terminal_read lists them)"));
        let command = commands.iter().find(|c| c.id == number).ok_or_else(no_such)?;
        let (output, left_out) = open.command_output(number, MOST_BACK).ok_or_else(|| {
            ServiceError::InvalidOp(format!("command {number}'s output can't be read while a program has the whole screen"))
        })?;
        let cut = if left_out > 0 { format!("({left_out} lines before these left out)\n") } else { String::new() };
        return Ok(format!("{}\n{cut}{output}", crate::terminals::in_words(command)));
    }
    let back = args["back"].as_u64().unwrap_or(0).min(MOST_BACK) as u32;
    let text = if args["numbered"].as_bool().unwrap_or(false) {
        open.numbered(back).into_iter().map(|(n, row)| format!("{n}\t{row}")).collect::<Vec<_>>().join("\n")
    } else {
        open.text(back)
    };
    let mut out = if text.is_empty() { "(nothing on its screen)".to_owned() } else { text };
    let ran: Vec<String> =
        commands.iter().rev().filter(|c| c.output.is_some()).take(LAST_COMMANDS).map(crate::terminals::in_words).collect();
    if !ran.is_empty() {
        out.push_str(&format!(
            "\n\nIts last commands (newest first; `command` reads one's output; tag one as `terminal {} command <number>`):\n",
            open.info().id
        ));
        out.push_str(&ran.join("\n"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_named_as_the_tool_says() {
        let k = |key, ctrl, alt| TerminalInput::Key { key, mods: Mods { ctrl, alt, shift: false } };
        assert_eq!(key("enter"), Ok(k(TermKey::Enter, false, false)));
        assert_eq!(key("ctrl+c"), Ok(k(TermKey::Char('c'), true, false)));
        assert_eq!(key("alt+b"), Ok(k(TermKey::Char('b'), false, true)));
        assert_eq!(key("F5"), Ok(k(TermKey::F(5), false, false)));
        assert!(key("hyper+x").is_err() && key("sparkle").is_err());
    }
}
