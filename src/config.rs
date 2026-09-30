//! User settings, stored as TOML in the config directory.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Language hint for the ASR, e.g. "de"; "auto" = detect from speech, "system" = system language.
    pub language: String,
    /// Input device name; `None` = system default.
    pub microphone: Option<String>,
    /// Spoken "new line", "comma" etc. are typed as such.
    pub voice_commands: bool,
    /// Languages with a column in the voice command table, e.g. "en".
    pub command_languages: Vec<String>,
    pub commands: Vec<VoiceCommand>,
    /// Words the model gets wrong, replaced after recognition.
    pub vocabulary: Vec<Replacement>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceCommand {
    /// What gets typed: `\n` breaks the line, punctuation attaches to the previous word,
    /// anything else is typed as a word.
    pub output: String,
    /// What to say, per language code; alternatives separated by commas.
    pub phrases: BTreeMap<String, String>,
}

impl VoiceCommand {
    fn new(output: &str, english: &str, german: &str) -> Self {
        Self {
            output: output.into(),
            phrases: BTreeMap::from([("en".into(), english.into()), ("de".into(), german.into())]),
        }
    }

    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new("\n", "new line", "neue Zeile"),
            Self::new("\n\n", "new paragraph", "neuer Absatz"),
            Self::new(".", "full stop, period", "Punkt"),
            Self::new(",", "comma", "Komma"),
            Self::new("?", "question mark", "Fragezeichen"),
            Self::new("!", "exclamation mark, exclamation point", "Ausrufezeichen"),
            Self::new(":", "colon", "Doppelpunkt"),
            Self::new(";", "semicolon", "Semikolon"),
        ]
    }

    pub fn default_languages() -> Vec<String> {
        vec!["en".into(), "de".into()]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Replacement {
    pub written: String,
    /// The ways the model gets it wrong.
    pub heard: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: crate::language::DETECT.into(),
            microphone: None,
            voice_commands: true,
            command_languages: VoiceCommand::default_languages(),
            commands: VoiceCommand::defaults(),
            vocabulary: Vec::new(),
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
