//! Action registry. Every user-invokable operation is an `Action`: a stable id, a
//! label, the parameter it needs and a typed command for the provider that made
//! it. Actions carry no keys; the keymap binds ids to keys. The leader menu,
//! which-key HUD, palette, toolbars and touch entry are all generated from the
//! same list, and invoking any of them takes the same path.

use std::any::Any;
use std::rc::Rc;

use egui::RichText;

use crate::app::BufferKind;
use crate::keymap::{Chord, InputMode};
use crate::net::NetClient;

/// What an action needs before it can run.
#[derive(Clone, Debug, PartialEq)]
pub enum Param {
    None,
    /// Pick one (the palette lists them, filterable).
    Choice(Vec<Choice>),
    /// On or off; runs with "true" or "false".
    Toggle { on: bool },
    /// Free text (opens ready for typing or dictation).
    Text { hint: String },
}

impl Param {
    /// Choices as the palette lists them (toggles become On/Off).
    pub fn choices(&self) -> Vec<Choice> {
        match self {
            Self::Choice(choices) => choices.clone(),
            Self::Toggle { on } => vec![Choice::new("true", "On").current(*on), Choice::new("false", "Off").current(!*on)],
            Self::None | Self::Text { .. } => Vec::new(),
        }
    }
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

/// Where an action is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Wherever focus is.
    Global,
    /// Only while its view has the focused slot.
    Focused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarStyle {
    /// A button in the view's action row.
    Button,
    /// A picker or toggle in the view's controls row.
    Control,
}

/// How an action shows up in its view's toolbar.
#[derive(Clone, Debug, PartialEq)]
pub struct Toolbar {
    pub caption: String,
    pub icon: Option<&'static str>,
    pub style: ToolbarStyle,
}

#[derive(Clone)]
pub struct Action {
    /// Stable name the keymap binds to.
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub param: Param,
    pub scope: Scope,
    /// Other keymap names for this action, most specific first.
    pub aliases: Vec<String>,
    pub toolbar: Option<Toolbar>,
    command: Rc<dyn Any>,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action").field("id", &self.id).field("param", &self.param).finish_non_exhaustive()
    }
}

impl Action {
    pub fn new<C: 'static>(id: impl Into<String>, label: impl Into<String>, command: C) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            param: Param::None,
            scope: Scope::Global,
            aliases: Vec::new(),
            toolbar: None,
            command: Rc::new(command),
        }
    }

    pub fn param(mut self, param: Param) -> Self {
        self.param = param;
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn focused(mut self) -> Self {
        self.scope = Scope::Focused;
        self
    }

    pub fn alias(mut self, name: impl Into<String>) -> Self {
        self.aliases.push(name.into());
        self
    }

    pub fn toolbar(mut self, style: ToolbarStyle, caption: impl Into<String>, icon: Option<&'static str>) -> Self {
        self.toolbar = Some(Toolbar { caption: caption.into(), icon, style });
        self
    }

    /// The provider's own command, if this action is one of its kind.
    pub fn command<C: 'static>(&self) -> Option<&C> {
        self.command.downcast_ref()
    }
}

/// Who runs an action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The app shell (slots, layout, overlays).
    Shell,
    View(BufferKind),
}

/// An action with its resolved keys.
#[derive(Clone, Debug)]
pub struct Bound {
    pub action: Action,
    pub origin: Origin,
    /// Keys after the leader.
    pub leader: Option<Vec<char>>,
    /// Direct keys in normal mode.
    pub keys: Vec<Chord>,
}

impl Bound {
    pub fn key_hint(&self) -> String {
        match &self.leader {
            Some(path) => {
                let keys: Vec<String> = path.iter().map(char::to_string).collect();
                format!("SPC {}", keys.join(" "))
            }
            None => self.keys.first().map(Chord::label).unwrap_or_default(),
        }
    }
}

/// A request to run an action, with its parameter value if it takes one.
#[derive(Clone, Debug)]
pub struct Invocation {
    pub action: Action,
    pub origin: Origin,
    pub value: Option<String>,
}

/// What a running action may touch besides its provider.
pub struct RunCx<'a> {
    pub net: &'a NetClient,
    pub ctx: &'a egui::Context,
    pub input_mode: &'a mut InputMode,
    /// The focused slot.
    pub slot: usize,
}

/// A part of the UI that offers actions: what it can do right now, and running
/// its own (the command is the provider's own type).
pub trait ActionProvider {
    fn actions(&self) -> Vec<Action>;

    fn run(&mut self, action: &Action, value: Option<String>, cx: &mut RunCx<'_>);

    /// An action id to open in the palette (e.g. once its choices have loaded).
    fn take_palette_request(&mut self) -> Option<String> {
        None
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeaderEntry {
    Group { key: char },
    Action { key: char, index: usize },
}

impl LeaderEntry {
    pub fn key(&self) -> char {
        match self {
            Self::Group { key } | Self::Action { key, .. } => *key,
        }
    }
}

/// What is reachable one key below `path`: submenus that contain at least one
/// action, and actions bound directly at this level.
pub fn leader_entries(bound: &[Bound], path: &[char]) -> Vec<LeaderEntry> {
    let mut entries: Vec<LeaderEntry> = Vec::new();
    for (index, b) in bound.iter().enumerate() {
        let Some(leader) = &b.leader else { continue };
        if leader.len() <= path.len() || !leader.starts_with(path) {
            continue;
        }
        let key = leader[path.len()];
        if leader.len() == path.len() + 1 {
            entries.retain(|e| e.key() != key);
            entries.push(LeaderEntry::Action { key, index });
        } else if !entries.iter().any(|e| e.key() == key) {
            entries.push(LeaderEntry::Group { key });
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

/// What the user did with a toolbar control.
pub enum ToolbarUse {
    Run(Option<String>),
    /// Needs a parameter the toolbar can't take; open the palette at it.
    OpenPalette,
}

/// Draws an action as a toolbar control.
pub fn toolbar_control(ui: &mut egui::Ui, action: &Action) -> Option<ToolbarUse> {
    let toolbar = action.toolbar.as_ref()?;
    let text = match toolbar.icon {
        Some(icon) if toolbar.caption.is_empty() => icon.to_owned(),
        Some(icon) => format!("{icon} {}", toolbar.caption),
        None => toolbar.caption.clone(),
    };
    match (&action.param, toolbar.style) {
        (Param::Toggle { on }, ToolbarStyle::Control) => {
            let mut value = *on;
            ui.checkbox(&mut value, RichText::new(text).size(11.0)).changed().then(|| ToolbarUse::Run(Some(value.to_string())))
        }
        (Param::Choice(choices), ToolbarStyle::Control) => {
            ui.label(RichText::new(format!("{text}:")).strong().size(11.0));
            let shown = choices.iter().find(|c| c.current).map(|c| c.label.clone()).unwrap_or_else(|| "—".into());
            let mut picked = None;
            egui::ComboBox::from_id_salt(("toolbar", action.id.as_str())).width(130.0).selected_text(shown).show_ui(ui, |ui| {
                for choice in choices {
                    let item = ui.selectable_label(choice.current, &choice.label);
                    let item = match &choice.detail {
                        Some(d) => item.on_hover_text(d),
                        None => item,
                    };
                    if item.clicked() && !choice.current {
                        picked = Some(choice.value.clone());
                    }
                }
            });
            picked.map(|v| ToolbarUse::Run(Some(v)))
        }
        (Param::None, _) => ui.button(text).on_hover_text(&action.label).clicked().then_some(ToolbarUse::Run(None)),
        _ => ui.button(text).on_hover_text(&action.label).clicked().then_some(ToolbarUse::OpenPalette),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(id: &str, leader: &str) -> Bound {
        Bound {
            action: Action::new(id, id, ()),
            origin: Origin::Shell,
            leader: (!leader.is_empty()).then(|| leader.chars().collect()),
            keys: Vec::new(),
        }
    }

    #[test]
    fn leader_tree_is_built_from_resolved_paths() {
        let all = vec![bound("slot", "1"), bound("new", "sn"), bound("resume", "sr"), bound("palette-only", "")];
        let root = leader_entries(&all, &[]);
        assert!(root.contains(&LeaderEntry::Group { key: 's' }));
        assert!(root.contains(&LeaderEntry::Action { key: '1', index: 0 }));
        assert!(!root.iter().any(|e| matches!(e, LeaderEntry::Action { index: 3, .. })));
        let session = leader_entries(&all, &['s']);
        assert_eq!(session, vec![LeaderEntry::Action { key: 'n', index: 1 }, LeaderEntry::Action { key: 'r', index: 2 }]);
    }

    #[test]
    fn commands_downcast_to_their_provider_type() {
        #[derive(Debug, PartialEq)]
        enum Mine {
            Go,
        }
        let action = Action::new("x", "X", Mine::Go);
        assert_eq!(action.command::<Mine>(), Some(&Mine::Go));
        assert_eq!(action.command::<u8>(), None);
    }

    #[test]
    fn fuzzy_prefers_contiguous_and_prefix_matches() {
        assert!(fuzzy_score("res", "Resume session") > fuzzy_score("res", "Close session (resets)"));
        assert_eq!(fuzzy_score("xyz", "Resume session"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
