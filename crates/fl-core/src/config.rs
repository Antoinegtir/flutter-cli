//! User configuration, loaded from `~/.config/fl/config.toml`.
//! Absent or partial files fall back to defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub log_level: String,
    pub theme: ThemeName,
    pub flutter_path: Option<PathBuf>,
    pub adb_path: Option<PathBuf>,
    pub default_target_port: u16,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    TokyoNight,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            log_level: "info".into(),
            theme: ThemeName::TokyoNight,
            flutter_path: None,
            adb_path: None,
            default_target_port: 5555,
        }
    }
}

impl Config {
    pub fn from_str(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn from_path_or_default(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(s) => Self::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "fl")
            .map(|d| d.config_dir().join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let c = Config::default();
        assert_eq!(c.log_level, "info");
        assert_eq!(c.default_target_port, 5555);
        assert_eq!(c.theme, ThemeName::TokyoNight);
    }

    #[test]
    fn empty_toml_loads_defaults() {
        let c = Config::from_str("").unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn partial_toml_overrides_only_named_fields() {
        let c = Config::from_str(r#"log_level = "debug""#).unwrap();
        assert_eq!(c.log_level, "debug");
        assert_eq!(c.default_target_port, 5555);
    }

    #[test]
    fn missing_path_falls_back() {
        let c = Config::from_path_or_default(Path::new("/does/not/exist.toml"));
        assert_eq!(c, Config::default());
    }
}
