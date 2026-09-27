//! User settings, stored as TOML in the config directory.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Language hint for the ASR, e.g. "de", or "auto".
    pub language: String,
    /// Input device name; `None` = system default.
    pub microphone: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "auto".into(),
            microphone: None,
        }
    }
}

impl Config {
    /// Linux: `~/.config/freisprech/config.toml`, Windows: `%APPDATA%\freisprech\config.toml`.
    fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_default()
            .join("freisprech")
            .join("config.toml")
    }

    /// Falls back to defaults if the file is missing or unreadable.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "Invalid configuration, using defaults");
            Self::default()
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
