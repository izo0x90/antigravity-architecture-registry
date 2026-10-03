//! UI configuration: branding, keymap and theme. The defaults ship in the binary
//! (`config/default.toml`); a user file served by the server is merged over them.

use serde::Deserialize;

use crate::graph_style::GraphStyle;
use crate::keymap::KeymapConfig;
use crate::theme::Theme;

const DEFAULTS: &str = include_str!("../config/default.toml");

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

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UiConfig {
    pub brand: Brand,
    #[serde(default)]
    pub user: User,
    pub keymap: KeymapConfig,
    pub theme: Theme,
    pub graph_style: GraphStyle,
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

impl UiConfig {
    /// The defaults with a user file (TOML) merged over them.
    pub fn with_overrides(user: &str) -> Result<Self, String> {
        let mut table: toml::Table = DEFAULTS.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let user: toml::Table = user.parse().map_err(|e: toml::de::Error| format!("ui config: {e}"))?;
        merge(&mut table, user);
        let config: Self = toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| format!("ui config: {e}"))?;
        config.graph_style.check(&config.theme).map_err(|e| format!("ui config: {e}"))?;
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
        assert_eq!(config.theme.ok, egui::Color32::from_rgb(0x50, 0xc8, 0x78));
        assert!(UiConfig::with_overrides("[theme]\nwarn = \"orange\"").is_err());
    }

    #[test]
    fn a_bad_user_file_is_reported() {
        assert!(UiConfig::with_overrides("[keymap.bind]\n\"x\" = { keys = [\"hyper+q\"] }").is_err());
    }
}
