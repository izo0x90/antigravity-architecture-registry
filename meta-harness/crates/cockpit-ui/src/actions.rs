//! Action registry. Every user-invokable operation (global, per view type, or
//! contextual to what is on screen) is described as data; the leader menu, the
//! which-key HUD, the palette and the touch sheet are all generated from the same
//! list, and invoking any of them takes the same path. Views add their own actions
//! (including dynamic ones from the harness) without touching the input code.

/// Which part of the cockpit handles an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    App,
    Chat,
    Graph,
}

/// What an action needs before it can run.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    None,
    /// Pick one (the palette lists them, filterable).
    Choice(Vec<Choice>),
    /// Free text (opens ready for typing or dictation).
    Text { hint: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub detail: Option<String>,
    pub current: bool,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self { value: value.into(), label: label.into(), detail: None, current: false }
    }

    pub fn detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }

    pub fn current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub owner: Owner,
    pub label: String,
    /// Leader keys after Space: group keys, then the action's own key. Empty means
    /// palette-only.
    pub leader: Vec<char>,
    pub param: Param,
    pub detail: Option<String>,
}

impl Action {
    pub fn new(owner: Owner, id: impl Into<String>, label: impl Into<String>) -> Self {
        Self { id: id.into(), owner, label: label.into(), leader: Vec::new(), param: Param::None, detail: None }
    }

    pub fn keys(mut self, keys: &str) -> Self {
        self.leader = keys.chars().collect();
        self
    }

    pub fn param(mut self, param: Param) -> Self {
        self.param = param;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// A request to run an action, with its parameter value if it takes one.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub owner: Owner,
    pub id: String,
    pub value: Option<String>,
}

impl Invocation {
    pub fn of(action: &Action, value: Option<String>) -> Self {
        Self { owner: action.owner, id: action.id.clone(), value }
    }
}

/// Leader submenus, by key path.
const GROUPS: &[(&str, &str)] = &[
    ("b", "Buffer"),
    ("l", "Layout"),
    ("s", "Session"),
    ("m", "Model & config"),
    ("o", "Mode"),
    ("p", "MCP servers"),
    ("h", "Harness commands"),
    ("a", "Account"),
    ("r", "Requests"),
    ("g", "Graph"),
];

pub fn group_label(path: &[char]) -> Option<&'static str> {
    let key: String = path.iter().collect();
    GROUPS.iter().find(|(k, _)| *k == key).map(|(_, label)| *label)
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeaderEntry {
    Group { key: char, label: String },
    Action { key: char, index: usize },
}

impl LeaderEntry {
    pub fn key(&self) -> char {
        match self {
            Self::Group { key, .. } | Self::Action { key, .. } => *key,
        }
    }
}

/// What is reachable one key below `path`: submenus that contain at least one
/// action, and actions bound directly at this level.
pub fn leader_entries(actions: &[Action], path: &[char]) -> Vec<LeaderEntry> {
    let mut entries: Vec<LeaderEntry> = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        if action.leader.len() <= path.len() || !action.leader.starts_with(path) {
            continue;
        }
        let key = action.leader[path.len()];
        if action.leader.len() == path.len() + 1 {
            entries.retain(|e| e.key() != key);
            entries.push(LeaderEntry::Action { key, index });
        } else if !entries.iter().any(|e| e.key() == key) {
            let mut sub = path.to_vec();
            sub.push(key);
            let label = match group_label(&sub) {
                Some(label) => label.to_owned(),
                None => format!("{key}…"),
            };
            entries.push(LeaderEntry::Group { key, label });
        }
    }
    entries.sort_by_key(|e| (matches!(e, LeaderEntry::Action { .. }), e.key()));
    entries
}

/// Subsequence fuzzy match; higher is better, `None` means no match.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text = text.to_lowercase();
    let mut score = 0;
    let mut last = None;
    let mut chars = text.char_indices();
    for q in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let (pos, _) = chars.find(|(_, c)| *c == q)?;
        score += match last {
            Some(prev) if pos == prev + 1 => 3,
            _ => 1,
        };
        if pos == 0 {
            score += 2;
        }
        last = Some(pos);
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actions() -> Vec<Action> {
        vec![
            Action::new(Owner::App, "slot.1", "Focus slot 1").keys("1"),
            Action::new(Owner::Chat, "session.new", "New session").keys("sn"),
            Action::new(Owner::Chat, "session.resume", "Resume session").keys("sr"),
            Action::new(Owner::App, "palette", "Command palette"),
        ]
    }

    #[test]
    fn leader_tree_is_built_from_action_paths() {
        let all = actions();
        let root = leader_entries(&all, &[]);
        assert!(root.contains(&LeaderEntry::Group { key: 's', label: "Session".into() }));
        assert!(root.contains(&LeaderEntry::Action { key: '1', index: 0 }));
        assert!(!root.iter().any(|e| matches!(e, LeaderEntry::Action { index: 3, .. })), "palette-only actions stay off the leader");
        let session = leader_entries(&all, &['s']);
        assert_eq!(session, vec![LeaderEntry::Action { key: 'n', index: 1 }, LeaderEntry::Action { key: 'r', index: 2 }]);
    }

    #[test]
    fn fuzzy_prefers_contiguous_and_prefix_matches() {
        assert!(fuzzy_score("res", "Resume session") > fuzzy_score("res", "Close session (resets)"));
        assert_eq!(fuzzy_score("xyz", "Resume session"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
