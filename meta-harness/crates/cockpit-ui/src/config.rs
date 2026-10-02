//! UI configuration: branding and keymap. The defaults ship in the binary
//! (`config/default.toml`); a user file served by the server is merged over them.

use serde::Deserialize;

use crate::keymap::KeymapConfig;

const DEFAULTS: &str = include_str!("../config/default.toml");

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Brand {
    pub name: String,
    pub tagline: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct UiConfig {
    pub brand: Brand,
    pub keymap: KeymapConfig,
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
        toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| format!("ui config: {e}"))
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
    fn a_bad_user_file_is_reported() {
        assert!(UiConfig::with_overrides("[keymap.bind]\n\"x\" = { keys = [\"hyper+q\"] }").is_err());
    }
}
