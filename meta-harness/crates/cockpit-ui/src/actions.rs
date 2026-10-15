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

/// Where an action sits relative to where you are, closest first. Every
/// bound action has one (worked out once a frame, with its keys); every
/// surface reads it: the key bar and the leader menu put the closest first,
/// the `?` sheet has a section for each, and each host draws its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// What you're on: the cursor's item, or the box being typed in.
    On,
    /// The focused panel's own, and the moves and go-tos on its cursor.
    Panel,
    /// What holds the panel: the quick look, the column.
    Host,
    /// The rest: columns, servers, the palette, the bell.
    Everywhere,
}

/// Where an action's control shows. Hosts draw both; views draw none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    /// The panel's top line: what applies to the whole view.
    Header,
    /// Under the item the cursor is on, while it's on it.
    Item,
    /// Under the box being typed in, while it's typed in (Send…).
    Typing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// A button.
    Button,
    /// A setting: a picker (choices) or a switch (on/off).
    Setting,
    /// One of a list to pick from (a question's answers): a full-width row.
    Choice,
}

/// How a control or a status reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Muted,
    Accent,
    Info,
    Ok,
    Warn,
    Error,
}

impl Tone {
    pub fn color(self) -> egui::Color32 {
        let c = crate::theme::colors();
        match self {
            Self::Plain => c.text,
            Self::Muted => c.muted,
            Self::Accent => c.accent,
            Self::Info => c.info,
            Self::Ok => c.ok,
            Self::Warn => c.warn,
            Self::Error => c.error,
        }
    }
}

/// An action's control: where it shows and how it looks.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub at: At,
    pub style: Style,
    pub caption: String,
    pub icon: Option<&'static str>,
    /// Its colour, when it says something (allow, deny).
    pub tone: Option<Tone>,
    /// More on hover, after what it does and its key.
    pub hover: Option<String>,
    /// At the far end of its row (the top bar's columns and tools).
    pub end: bool,
}

impl Control {
    pub fn new(at: At, style: Style, caption: impl Into<String>, icon: Option<&'static str>) -> Self {
        Self { at, style, caption: caption.into(), icon, tone: None, hover: None, end: false }
    }

    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = Some(tone);
        self
    }

    pub fn hover(mut self, hover: impl Into<String>) -> Self {
        self.hover = Some(hover.into());
        self
    }

    pub fn at_end(mut self) -> Self {
        self.end = true;
        self
    }
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
    pub control: Option<Control>,
    /// A glyph for compact controls (on-screen chips).
    pub icon: Option<&'static str>,
    pub group: Group,
    /// `PRIMARY`, `SECONDARY` or `MINOR`.
    pub importance: u8,
    pub when: When,
    /// Running it is a jump: where the cursor was goes in the jump list.
    pub jump: bool,
    /// Where it sits, when its maker says (a shell action on what you're on
    /// is the panel's; a quick look's or a column's own, its host's). A
    /// view's own are its panel's; the shell's otherwise everywhere's.
    pub layer: Option<Layer>,
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
            control: None,
            icon: None,
            group: Group::Act,
            importance: SECONDARY,
            when: When::Always,
            jump: false,
            layer: None,
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

    pub fn layer(mut self, layer: Layer) -> Self {
        self.layer = Some(layer);
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

    /// The glyph for compact controls: its own, else its control's.
    pub fn glyph(&self) -> Option<&'static str> {
        self.icon.or(self.control.as_ref().and_then(|c| c.icon))
    }

    /// Shown on the panel's top line.
    pub fn header(mut self, style: Style, caption: impl Into<String>, icon: Option<&'static str>) -> Self {
        self.control = Some(Control::new(At::Header, style, caption, icon));
        self
    }

    /// Shown under the cursor's item, while the cursor is on it.
    pub fn on_item(mut self, style: Style, caption: impl Into<String>, icon: Option<&'static str>) -> Self {
        self.control = Some(Control::new(At::Item, style, caption, icon));
        self
    }

    /// Shown under the box being typed in, while it's typed in.
    pub fn on_typing(mut self, style: Style, caption: impl Into<String>, icon: Option<&'static str>) -> Self {
        self.control = Some(Control::new(At::Typing, style, caption, icon));
        self
    }

    /// Where its control shows, if it has one.
    pub fn at(&self) -> Option<At> {
        self.control.as_ref().map(|c| c.at)
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
    /// Touch gestures that run it (`"swipe_left"`).
    pub gestures: Vec<String>,
    /// Where it sits relative to where you are.
    pub layer: Layer,
}

impl Bound {
    /// The quickest way to run it, as keys are written: its direct key,
    /// else its leader path (empty with neither). What every control and
    /// the key bar show.
    pub fn key_hint(&self) -> String {
        match self.keys.first() {
            Some(seq) => seq.iter().map(Chord::label).collect::<Vec<_>>().join(" "),
            None => self.leader.as_ref().map(|path| crate::keymap::local().leader_hint(path)).unwrap_or_default(),
        }
    }

    /// Every way to run it: its keys, its leader path, its gestures (what
    /// the `?` sheet lists).
    pub fn ways(&self) -> Vec<String> {
        let keys = self.keys.iter().map(|seq| seq.iter().map(Chord::label).collect::<Vec<_>>().join(" "));
        let leader = self.leader.iter().map(|path| crate::keymap::local().leader_hint(path));
        let gestures = self.gestures.iter().map(|g| g.replace('_', " "));
        keys.chain(leader).chain(gestures).collect()
    }

    /// Whether `gesture` (`"swipe_left"`) runs it.
    pub fn runs_on(&self, gesture: &str) -> bool {
        self.gestures.iter().any(|g| g == gesture)
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
    /// An action a control asked to run (its id, and its value if it has
    /// one): the app runs it once the frame is drawn, the way its key would.
    pub asked: &'a mut Option<(String, Option<String>)>,
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
/// action, and actions bound directly at this level. What's closest to where
/// you are comes first.
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
    entries.sort_by_key(|e| (closest(e, bound, path), matches!(e, LeaderEntry::Action { .. }), e.key()));
    entries
}

/// The closest layer a leader entry reaches (its action's, or the closest
/// of a submenu's).
pub fn closest(entry: &LeaderEntry, bound: &[Bound], path: &[char]) -> Layer {
    match entry {
        LeaderEntry::Action { index, .. } => bound[*index].layer,
        LeaderEntry::Group { key } => {
            let mut prefix = path.to_vec();
            prefix.push(*key);
            bound.iter().filter(|b| b.leader.as_ref().is_some_and(|l| l.starts_with(&prefix))).map(|b| b.layer).min().unwrap_or(Layer::Everywhere)
        }
    }
}

/// Whether a leader entry is about where you are (not only everywhere's).
pub fn is_primary(entry: &LeaderEntry, bound: &[Bound], path: &[char]) -> bool {
    closest(entry, bound, path) < Layer::Everywhere
}

/// What the user did with a control.
enum ControlUse {
    Run(Option<String>),
    /// Needs a parameter the toolbar can't take; open the palette at it.
    OpenPalette,
}

/// Draws a bound action as its control: its caption and the key that runs
/// it (the binding's own, the one key handling uses; on hover when there's
/// no room); used, it's asked for (`RunCx::asked`). Whether it was used.
/// Only hosts call this (and `controls`): views draw no controls.
pub fn control(ui: &mut egui::Ui, bound: &Bound, cx: &mut RunCx<'_>) -> bool {
    let action = &bound.action;
    let Some(used) = draw_control(ui, bound) else { return false };
    let value = match used {
        ControlUse::Run(value) => value,
        // Asked without its value: the palette opens at it.
        ControlUse::OpenPalette => None,
    };
    *cx.asked = Some((action.id.clone(), value));
    true
}

/// Draws every bound action of `bound` that has a control, in order.
pub fn controls<'x>(ui: &mut egui::Ui, bound: impl IntoIterator<Item = &'x Bound>, cx: &mut RunCx<'_>) {
    for b in bound.into_iter().filter(|b| b.action.control.is_some()) {
        control(ui, b, cx);
    }
}

/// Narrowest room left in a row for a control to also show its key.
const KEY_SHOWN_ABOVE: f32 = 140.0;

/// Narrowest a picker gets before its text is truncated.
const PICKER_MIN_WIDTH: f32 = 90.0;

/// Draws a bound action's control; hovering it says what it does and its
/// key, and the key shows beside its caption when there's room.
fn draw_control(ui: &mut egui::Ui, bound: &Bound) -> Option<ControlUse> {
    let action = &bound.action;
    let toolbar = action.control.as_ref()?;
    let key = bound.key_hint();
    let mut tip = if key.is_empty() { action.label.clone() } else { format!("{} ({key})", action.label) };
    if let Some(more) = &toolbar.hover {
        tip = format!("{tip}\n{more}");
    }
    let text = match toolbar.icon {
        Some(icon) if toolbar.caption.is_empty() => icon.to_owned(),
        Some(icon) => format!("{icon} {}", toolbar.caption),
        None if toolbar.caption.is_empty() => action.label.clone(),
        None => toolbar.caption.clone(),
    };
    let shows_key = !key.is_empty() && ui.available_width() > KEY_SHOWN_ABOVE;
    let tinted = |text: String| -> egui::WidgetText {
        let look = crate::theme::look();
        let style = ui.style();
        let mut job = egui::text::LayoutJob::default();
        let mut caption = RichText::new(text).size(look.text_body);
        if let Some(tone) = toolbar.tone {
            caption = caption.color(tone.color());
        }
        caption.append_to(&mut job, style, egui::FontSelection::Default, egui::Align::Center);
        if shows_key {
            RichText::new(format!("  {key}")).monospace().size(look.text_small).color(crate::theme::colors().key).append_to(&mut job, style, egui::FontSelection::Default, egui::Align::Center);
        }
        job.into()
    };
    match (&action.param, toolbar.style) {
        (Param::Toggle { on }, Style::Setting) => {
            let mut value = *on;
            ui.checkbox(&mut value, tinted(text)).on_hover_text(tip).changed().then(|| ControlUse::Run(Some(value.to_string())))
        }
        (Param::Choice(choices), Style::Setting) => {
            let caption = tinted(format!("{text}:"));
            let shown = choices.iter().find(|c| c.current).map(|c| c.label.clone()).unwrap_or_else(|| "—".into());
            // Caption and picker stay on one row: wrap first if they won't fit, and
            // truncate the picker rather than let it push the slot wider.
            let measure = |ui: &egui::Ui, text: egui::WidgetText| text.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Button).size().x;
            let spacing = ui.spacing();
            let gap = spacing.item_spacing.x;
            let picker = (measure(ui, RichText::new(&shown).into()) + spacing.icon_width + spacing.icon_spacing + spacing.button_padding.x * 2.0).max(PICKER_MIN_WIDTH);
            if ui.available_width() < measure(ui, caption.clone()) + gap + picker {
                ui.end_row();
            }
            ui.label(caption).on_hover_text(&tip);
            let mut picked = None;
            egui::ComboBox::from_id_salt(("control", action.id.as_str()))
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
            picked.map(|v| ControlUse::Run(Some(v)))
        }
        (Param::None, Style::Choice) => {
            let row = egui::Button::selectable(false, tinted(format!("{} {text}", egui_phosphor::regular::CARET_RIGHT))).min_size(egui::vec2(ui.available_width(), 0.0));
            ui.add(row).on_hover_text(tip).clicked().then_some(ControlUse::Run(None))
        }
        (Param::None, _) => ui.button(tinted(text)).on_hover_text(tip).clicked().then_some(ControlUse::Run(None)),
        _ => ui.button(tinted(text)).on_hover_text(tip).clicked().then_some(ControlUse::OpenPalette),
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
            gestures: Vec::new(),
            layer: Layer::Everywhere,
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
    fn leader_entries_closest_to_where_you_are_come_first() {
        let mut all = vec![bound("slot", "1"), bound("new", "sn"), bound("graph.view", "gv"), bound("graph.flip", "z")];
        (all[2].layer, all[3].layer) = (Layer::Panel, Layer::Panel);
        let root = leader_entries(&all, &[]);
        assert_eq!(root[..2], [LeaderEntry::Group { key: 'g' }, LeaderEntry::Action { key: 'z', index: 3 }], "the panel's own first: {root:?}");
        assert!(is_primary(&root[0], &all, &[]));
        assert!(!is_primary(&root[2], &all, &[]), "then everywhere's");
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
