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

/// The server connection: how the event socket comes back after a drop.
#[derive(Clone, Debug, Deserialize)]
pub struct NetUi {
    pub reconnect_min_ms: u64,
    pub reconnect_max_ms: u64,
}

impl Default for NetUi {
    fn default() -> Self {
        Self { reconnect_min_ms: 500, reconnect_max_ms: 10_000 }
    }
}

/// Links between views.
#[derive(Clone, Debug, Deserialize)]
pub struct LinksUi {
    /// Follow mode: how long the code cursor rests before the graph follows it.
    pub follow_debounce_ms: u64,
}

impl Default for LinksUi {
    fn default() -> Self {
        Self { follow_debounce_ms: 250 }
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
    pub net: NetUi,
    pub links: LinksUi,
    pub code: CodeUi,
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
}
