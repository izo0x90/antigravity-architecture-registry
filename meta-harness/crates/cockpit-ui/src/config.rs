//! UI configuration: branding, keymap and theme. The defaults ship in the binary
//! (`config/default.toml`); a user file served by the server is merged over them.

use serde::Deserialize;

use crate::graph_config::GraphUi;
use crate::graph_style::GraphStyle;
use crate::keymap::KeymapConfig;
use crate::theme::{Look, Theme};

pub(crate) const DEFAULTS: &str = include_str!("../config/default.toml");

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Brand {
    pub name: String,
    pub tagline: String,
}

/// Who is at the cockpit: the author of their comments, reviews and approvals.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct User {
    pub name: String,
}

/// The server connection: how the event socket comes back after a drop,
/// and how soon the layout is saved after it changes.
#[derive(Clone, Debug, Deserialize)]
pub struct NetUi {
    pub reconnect_min_ms: u64,
    pub reconnect_max_ms: u64,
    pub save_after_ms: u64,
}

impl Default for NetUi {
    fn default() -> Self {
        Self { reconnect_min_ms: 500, reconnect_max_ms: 10_000, save_after_ms: 500 }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UiConfig {
    pub brand: Brand,
    #[serde(default)]
    pub user: User,
    pub keymap: KeymapConfig,
    pub theme: Theme,
    pub look: Look,
    pub graph_style: GraphStyle,
    pub graph: GraphUi,
    /// Touch gestures' thresholds (every view's).
    pub touch: crate::touch::TouchConfig,
    pub net: NetUi,
    pub code: CodeUi,
    pub panels: PanelsUi,
    pub changes: ChangesUi,
    pub chat: ChatUi,
    /// The faces text is drawn in.
    pub fonts: FontsUi,
    /// Every state's word, icon and colour.
    #[serde(default)]
    pub status: crate::status::Statuses,
}

/// Tables merge key by key; any other value replaces the base.
fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base_table)), toml::Value::Table(over_table)) => merge(base_table, over_table),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// The panels: how many there may be, and where they show the buttons of
/// what you're on (until changed in the cockpit, which is kept).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PanelsUi {
    pub max: usize,
    pub item_buttons: crate::hosted::ItemButtons,
}

impl Default for PanelsUi {
    fn default() -> Self {
        Self { max: 12, item_buttons: crate::hosted::ItemButtons::default() }
    }
}

/// The faces text is drawn in: font packs the UI server has installed, by
/// name (`meta-harness pack install jetbrains-mono`); empty: egui's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct FontsUi {
    /// Code, terminals, keys: the monospace face.
    pub mono: String,
    /// Everything else.
    pub text: String,
}

/// The changes view.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChangesUi {
    /// Unchanged lines shown around each change, and how many more a
    /// "more context" adds.
    pub context: u32,
    pub more_context: u32,
    /// Up to this many files, and this many changed lines in all, open by
    /// themselves (the diff run together); more stay closed until opened.
    pub open_up_to_files: usize,
    pub open_up_to_lines: u32,
    /// Seconds between asking for the list again while it's shown (the
    /// index and staging change outside the files too).
    pub refresh_s: f64,
    /// Auto layout: side by side from this panel width (points).
    pub side_by_side_from: f32,
    /// The code view shows deleted lines in place (`z d` toggles them).
    pub deleted_in_code: bool,
}

impl Default for ChangesUi {
    fn default() -> Self {
        Self { context: 3, more_context: 10, open_up_to_files: 30, open_up_to_lines: 600, refresh_s: 5.0, side_by_side_from: 900.0, deleted_in_code: true }
    }
}

/// The chat: how much a chat shows and what shows the agent at work, till
/// it's picked there; how long after its last update it's said to be
/// receiving still, and quiet too long.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChatUi {
    #[serde(deserialize_with = "named")]
    pub shows: Verbosity,
    #[serde(deserialize_with = "named")]
    pub indicator: crate::indicator::Kind,
    pub receiving_s: f64,
    pub quiet_after_s: f64,
}

impl Default for ChatUi {
    fn default() -> Self {
        Self { shows: Verbosity::default(), indicator: crate::indicator::Kind::default(), receiving_s: 1.5, quiet_after_s: 60.0 }
    }
}

/// A setting's choices, each by its name (in the config, in what's kept)
/// and in words for the screen.
pub trait Named: Sized + Copy + PartialEq + 'static {
    /// What a choice is ("verbosity").
    const WHAT: &'static str;
    const ALL: &'static [Self];
    fn name(self) -> &'static str;
    fn words(self) -> &'static str;
    fn detail(self) -> &'static str;

    fn named(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.name() == name)
    }
}

/// One of a setting's choices, by name.
pub fn named<'de, D: serde::Deserializer<'de>, T: Named>(to: D) -> Result<T, D::Error> {
    let name = String::deserialize(to)?;
    T::named(&name).ok_or_else(|| serde::de::Error::custom(format!("unknown {} `{name}` (there are: {})", T::WHAT, T::ALL.iter().map(|c| c.name()).collect::<Vec<_>>().join(", "))))
}

/// How much of what the agent does a chat shows: messages and what
/// needs you (everything between two messages one folded line), the calls
/// folded into runs, or every call and its output open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    #[default]
    Normal,
    Everything,
}

impl Named for Verbosity {
    const WHAT: &'static str = "verbosity";
    const ALL: &'static [Self] = &[Self::Quiet, Self::Normal, Self::Everything];

    fn name(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Normal => "normal",
            Self::Everything => "everything",
        }
    }

    fn words(self) -> &'static str {
        match self {
            Self::Quiet => "Quiet",
            Self::Normal => "Normal",
            Self::Everything => "Everything",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Quiet => "Messages and what needs you: what it did between them is one folded line",
            Self::Normal => "Calls folded into runs, each a line; their output folded",
            Self::Everything => "Every call and its output, open",
        }
    }
}

/// The code view.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct CodeUi {
    /// Syntax colours: a highlight capture (`keyword`, `function.method`)
    /// or its first part (`function`) -> a theme color.
    pub highlight: std::collections::BTreeMap<String, String>,
}

impl CodeUi {
    /// The theme token for a capture name, by its most specific known part.
    pub fn token(&self, capture: &str) -> Option<&str> {
        let mut name = capture;
        loop {
            if let Some(t) = self.highlight.get(name) {
                return Some(t);
            }
            name = &name[..name.rfind('.')?];
        }
    }
}

impl UiConfig {
    /// The defaults with a user file (TOML) merged over them.
    pub fn with_overrides(user: &str) -> Result<Self, String> {
        let mut table: toml::Table = DEFAULTS.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let user: toml::Table = user.parse().map_err(|e: toml::de::Error| format!("ui config: {e}"))?;
        merge(&mut table, user);
        // A named preset (`preset = "classic"`) lays its own sections over the rest.
        if let Some(name) = table.get("preset").and_then(|p| p.as_str()).filter(|p| !p.is_empty()).map(str::to_owned) {
            let preset = table.get("presets").and_then(|p| p.get(&name)).and_then(|p| p.as_table()).cloned().ok_or_else(|| format!("ui config: no preset `{name}` in [presets]"))?;
            merge(&mut table, preset);
        }
        let config: Self = toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| format!("ui config: {e}"))?;
        config.graph_style.check(&config.theme).map_err(|e| format!("ui config: {e}"))?;
        crate::status::check(&config.status, &config.theme).map_err(|e| format!("ui config: {e}"))?;
        config.graph.check().map_err(|e| format!("ui config: {e}"))?;
        config.touch.check().map_err(|e| format!("ui config: {e}"))?;
        config.keymap.check_gestures().map_err(|e| format!("ui config: {e}"))?;
        if let Some((capture, token)) = config.code.highlight.iter().find(|(_, t)| config.theme.token(t).is_none()) {
            return Err(format!("ui config: code.highlight.{capture}: '{token}' is not a theme color"));
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse() {
        let config = UiConfig::with_overrides("").unwrap();
        assert_eq!(config.brand.name, "NAH");
        assert!(config.keymap.bind.contains_key("session.new"));
    }

    #[test]
    fn user_file_overrides_single_bindings_and_keeps_the_rest() {
        let config = UiConfig::with_overrides(
            r#"
            brand.name = "mine"
            [keymap.bind]
            "session.new" = { leader = "x n" }
            "#,
        )
        .unwrap();
        assert_eq!(config.brand.name, "mine");
        assert_eq!(config.brand.tagline, "not a harness");
        let new = config.keymap.bind["session.new"].leader.clone();
        assert_eq!(new, Some(crate::keymap::LeaderPath::try_from("x n".to_owned()).unwrap()));
        assert!(config.keymap.bind.contains_key("session.resume"));
    }

    #[test]
    fn theme_colors_come_from_config() {
        let config = UiConfig::with_overrides("[theme]\nwarn = \"#010203\"").unwrap();
        assert_eq!(config.theme.warn, egui::Color32::from_rgb(1, 2, 3));
        assert_eq!(config.theme.ok, egui::Color32::from_rgb(0x4a, 0xde, 0x80));
        assert!(UiConfig::with_overrides("[theme]\nwarn = \"orange\"").is_err());
    }

    #[test]
    fn a_bad_user_file_is_reported() {
        assert!(UiConfig::with_overrides("[keymap.bind]\n\"x\" = { keys = [\"hyper+q\"] }").is_err());
    }

    /// A setting's choice is read by its name; one there's no such is
    /// refused, the names there are said.
    #[test]
    fn the_chats_choices_are_read_by_name() {
        let config = UiConfig::with_overrides("[chat]\nshows = \"quiet\"").unwrap();
        assert_eq!(config.chat.shows, Verbosity::Quiet);
        let refused = UiConfig::with_overrides("[chat]\nindicator = \"lemmings\"").unwrap_err();
        assert!(refused.contains("unknown indicator `lemmings` (there are: heartbeat, none)"), "{refused}");
    }
}
