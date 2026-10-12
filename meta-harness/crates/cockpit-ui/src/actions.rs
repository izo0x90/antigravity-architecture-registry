//! Action registry. Every user-invokable operation is an `Action`: a stable id, a
//! label, the parameter it needs, a typed command for the provider that made
//! it, and how it ranks (group, importance, when it applies). Actions carry no
//! keys; the keymap binds ids to keys. The leader menu, which-key HUD, key bar,
//! prefix popup, key sheet, palette, toolbars and touch entry are all generated
//! from the same list, and invoking any of them takes the same path.

use std::any::Any;
use std::rc::Rc;

use egui::RichText;

use crate::app::BufferKind;
use crate::keymap::{Chord, InputMode};
use crate::nav::Target;
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
    /// Typed text looked up as it's typed: the provider gets each query
    /// (`ActionProvider::search`) and lists what it found for it here.
    Search { hint: String, found: Vec<Choice>, waiting: bool },
}

impl Param {
    /// Choices as the palette lists them (toggles become On/Off).
    pub fn choices(&self) -> Vec<Choice> {
        match self {
            Self::Choice(choices) => choices.clone(),
            Self::Toggle { on } => vec![Choice::new("true", "On").current(*on), Choice::new("false", "Off").current(!*on)],
            Self::Search { found, .. } => found.clone(),
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

/// What kind of thing an action does (how the key sheet groups them).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    /// Moves the cursor within the view.
    Navigate,
    /// Takes the cursor's target somewhere else.
    GoTo,
    /// Does something to the target or the work.
    Act,
    /// Changes how the view shows things.
    View,
}

impl Group {
    pub const ALL: [Group; 4] = [Group::Navigate, Group::GoTo, Group::Act, Group::View];

    pub fn title(self) -> &'static str {
        match self {
            Self::Navigate => "Navigate",
            Self::GoTo => "Go to",
            Self::Act => "Act",
            Self::View => "View",
        }
    }
}

/// When an action applies, given the cursor's target and what the shell knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Always,
    /// The cursor is on something.
    Target,
    /// The target is (or is in) a component.
    Component,
    /// The target is code, or a component that may have some.
    Code,
    /// The cursor is on a line that names a component (a call, a type).
    Named,
    /// The view has a search to step through.
    Searched,
    /// There is somewhere to go back (or forward) to.
    Back,
    Forward,
    /// A mark is set.
    Marked,
    /// Enter (or Backspace) would do something here.
    Enter,
    Leave,
    /// A relational go-to that leads somewhere from the target (when its
    /// relations are known; until then, it may).
    Reaches(crate::nav::GoTo),
    /// Not now (offered so it can be found, and says why when run).
    Unavailable,
}

/// What `When` is judged against.
#[derive(Clone, Debug, Default)]
pub struct Situation {
    pub target: Option<Target>,
    pub searched: bool,
    pub can_back: bool,
    pub can_forward: bool,
    pub marked: bool,
    pub can_enter: bool,
    pub can_leave: bool,
    /// The relational go-tos that lead somewhere from the target, once its
    /// relations are known.
    pub reaches: Option<Vec<crate::nav::GoTo>>,
}

impl When {
    pub fn holds(self, s: &Situation) -> bool {
        match self {
            Self::Always => true,
            Self::Target => s.target.is_some(),
            // A line of code is in some component, even before the index says which.
            Self::Component => s.target.as_ref().is_some_and(|t| t.component().is_some() || matches!(t, Target::Line { .. })),
            Self::Code => s.target.as_ref().is_some_and(|t| t.code().is_some() || t.component().is_some()),
            Self::Named => matches!(&s.target, Some(Target::Line { refs, .. }) if !refs.is_empty()),
            Self::Searched => s.searched,
            Self::Back => s.can_back,
            Self::Forward => s.can_forward,
            Self::Marked => s.marked,
            Self::Enter => s.can_enter,
            Self::Leave => s.can_leave,
            Self::Reaches(op) => Self::Component.holds(s) && s.reaches.as_ref().is_none_or(|r| r.contains(&op)),
            Self::Unavailable => false,
        }
    }
}

/// Importance: shown first in the key bar, listed first in the key sheet.
pub const PRIMARY: u8 = 1;
pub const SECONDARY: u8 = 2;
pub const MINOR: u8 = 3;

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
    /// A glyph for compact controls (on-screen chips).
    pub icon: Option<&'static str>,
    pub group: Group,
    /// `PRIMARY`, `SECONDARY` or `MINOR`.
    pub importance: u8,
    pub when: When,
    /// Running it is a jump: where the cursor was goes in the jump list.
    pub jump: bool,
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
            icon: None,
            group: Group::Act,
            importance: SECONDARY,
            when: When::Always,
            jump: false,
            command: Rc::new(command),
        }
    }

    pub fn group(mut self, group: Group) -> Self {
        self.group = group;
        self
    }

    pub fn importance(mut self, importance: u8) -> Self {
        self.importance = importance;
        self
    }

    pub fn when(mut self, when: When) -> Self {
        self.when = when;
        self
    }

    pub fn jump(mut self) -> Self {
        self.jump = true;
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

    pub fn focused(mut self) -> Self {
        self.scope = Scope::Focused;
        self
    }

    pub fn alias(mut self, name: impl Into<String>) -> Self {
        self.aliases.push(name.into());
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    /// The glyph for compact controls: its own, else its toolbar's.
    pub fn glyph(&self) -> Option<&'static str> {
        self.icon.or(self.toolbar.as_ref().and_then(|t| t.icon))
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
    /// Direct key sequences in normal mode (most are one chord; `g c` is two).
    pub keys: Vec<Vec<Chord>>,
}

impl Bound {
    pub fn key_hint(&self) -> String {
        match &self.leader {
            Some(path) => {
                let keys: Vec<String> = path.iter().map(char::to_string).collect();
                format!("SPC {}", keys.join(" "))
            }
            None => self.keys.first().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" ")).unwrap_or_default(),
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
    /// A short note for the status line (why something didn't happen).
    pub note: &'a mut Option<String>,
    /// The shell's own actions (moves, go-tos, the jump list), for views
    /// that put them on chips or toolbars.
    pub shell: &'a [Action],
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

    /// What's typed for a `Param::Search` action, as it changes.
    fn search(&mut self, _action: &str, _query: &str) {}
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
/// action, and actions bound directly at this level. What belongs to the
/// focused view comes first.
pub fn leader_entries(bound: &[Bound], path: &[char], focused: Origin) -> Vec<LeaderEntry> {
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
    entries.sort_by_key(|e| (!is_primary(e, bound, path, focused), matches!(e, LeaderEntry::Action { .. }), e.key()));
    entries
}

/// Whether a leader entry belongs to the focused view (an action of its, or
/// a submenu holding one).
pub fn is_primary(entry: &LeaderEntry, bound: &[Bound], path: &[char], focused: Origin) -> bool {
    match entry {
        LeaderEntry::Action { index, .. } => bound[*index].origin == focused,
        LeaderEntry::Group { key } => {
            let mut prefix = path.to_vec();
            prefix.push(*key);
            bound.iter().any(|b| b.origin == focused && b.leader.as_ref().is_some_and(|l| l.starts_with(&prefix)))
        }
    }
}

/// What the user did with a toolbar control.
pub enum ToolbarUse {
    Run(Option<String>),
    /// Needs a parameter the toolbar can't take; open the palette at it.
    OpenPalette,
}

/// Narrowest a toolbar picker gets before its text is truncated.
const PICKER_MIN_WIDTH: f32 = 90.0;

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
            ui.checkbox(&mut value, RichText::new(text).size(crate::theme::look().text_body)).changed().then(|| ToolbarUse::Run(Some(value.to_string())))
        }
        (Param::Choice(choices), ToolbarStyle::Control) => {
            let caption = RichText::new(format!("{text}:")).strong().size(crate::theme::look().text_body);
            let shown = choices.iter().find(|c| c.current).map(|c| c.label.clone()).unwrap_or_else(|| "—".into());
            // Caption and picker stay on one row: wrap first if they won't fit, and
            // truncate the picker rather than let it push the slot wider.
            let measure = |ui: &egui::Ui, text: RichText| {
                egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button).size().x
            };
            let spacing = ui.spacing();
            let gap = spacing.item_spacing.x;
            let picker = (measure(ui, RichText::new(&shown)) + spacing.icon_width + spacing.icon_spacing + spacing.button_padding.x * 2.0).max(PICKER_MIN_WIDTH);
            if ui.available_width() < measure(ui, caption.clone()) + gap + picker {
                ui.end_row();
            }
            ui.label(caption);
            let mut picked = None;
            egui::ComboBox::from_id_salt(("toolbar", action.id.as_str()))
                .width(PICKER_MIN_WIDTH.min(ui.available_width()))
                .truncate()
                .selected_text(shown)
                .show_ui(ui, |ui| {
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
        let root = leader_entries(&all, &[], Origin::Shell);
        assert!(root.contains(&LeaderEntry::Group { key: 's' }));
        assert!(root.contains(&LeaderEntry::Action { key: '1', index: 0 }));
        assert!(!root.iter().any(|e| matches!(e, LeaderEntry::Action { index: 3, .. })));
        let session = leader_entries(&all, &['s'], Origin::Shell);
        assert_eq!(session, vec![LeaderEntry::Action { key: 'n', index: 1 }, LeaderEntry::Action { key: 'r', index: 2 }]);
    }

    #[test]
    fn the_focused_views_leader_entries_come_first() {
        let mut all = vec![bound("slot", "1"), bound("new", "sn"), bound("graph.view", "gv"), bound("graph.flip", "z")];
        all[2].origin = Origin::View(BufferKind::Graph);
        all[3].origin = Origin::View(BufferKind::Graph);
        let root = leader_entries(&all, &[], Origin::View(BufferKind::Graph));
        assert_eq!(root[..2], [LeaderEntry::Group { key: 'g' }, LeaderEntry::Action { key: 'z', index: 3 }], "the graph's own first: {root:?}");
        assert!(is_primary(&root[0], &all, &[], Origin::View(BufferKind::Graph)));
        assert!(!is_primary(&root[2], &all, &[], Origin::View(BufferKind::Graph)), "then the rest");
        let from_spec = leader_entries(&all, &[], Origin::View(BufferKind::Spec));
        assert_eq!(from_spec[0], LeaderEntry::Group { key: 'g' }, "nothing of the spec's here: the usual order");
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
}
