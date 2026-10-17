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

/// How much scrollback `terminal_read` gives at most.
const MOST_BACK: u64 = 2000;

impl Terminals {
    pub(crate) fn schemas(trees: &TreeConfig) -> Vec<ToolDef> {
        let _ = trees;
        vec![
            tool(Capability::ViewRead, "terminals", "The user's terminals (shells the server runs): each one's id, title, whether its shell runs, its folder and size.", json!({ "type": "object", "properties": {} })),
            tool(
                Capability::ViewRead,
                "terminal_read",
                "What a terminal shows now, as text: its screen, with `back` lines of scrollback above it (default 0, at most 2000).",
                json!({ "type": "object", "properties": { "terminal": { "type": "string" }, "back": { "type": "integer", "minimum": 0 } }, "required": ["terminal"] }),
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
        self.terminals.as_ref().ok_or_else(|| ServiceError::InvalidOp("this server runs no shells (it was started with --no-shells)".into()))
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
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => TermKey::F(f[1..].parse().unwrap_or(1)),
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
        let id = || args["terminal"].as_str().map(TerminalId::new).ok_or_else(|| ServiceError::InvalidOp("`terminal` (its id, from `terminals`) is needed".into()));
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
                let back = args["back"].as_u64().unwrap_or(0).min(MOST_BACK) as u32;
                let text = open.text(back);
                Ok(if text.is_empty() { "(nothing on its screen)".into() } else { text })
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
