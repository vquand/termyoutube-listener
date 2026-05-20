use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopMode {
    Off,
    All,
    One,
}

impl LoopMode {
    pub fn next(self) -> Self {
        match self {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LoopMode::Off => "off",
            LoopMode::All => "all",
            LoopMode::One => "one",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub progress_sprite: String,
    pub loop_mode: LoopMode,
    pub shuffle: bool,
    pub show_shortcuts: bool,
    pub volume: u8,
    pub yt_playlist_url: Option<String>,
    pub local_folder: Option<String>,
    pub local_folder_label: Option<String>,
    pub caption_lang: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            progress_sprite: "nyan".into(),
            loop_mode: LoopMode::Off,
            shuffle: false,
            show_shortcuts: true,
            volume: 80,
            yt_playlist_url: None,
            local_folder: None,
            local_folder_label: None,
            caption_lang: "en".into(),
        }
    }
}

/// Selectable caption languages exposed in the params menu. Keep "en" as
/// element 0 since the default falls back here.
pub const CAPTION_LANGS: &[&str] = &[
    "en", "vi", "fr", "de", "es", "it", "pt", "ja", "ko", "zh", "ru",
];

pub fn path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("ytmtui").join("config.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home)
        .join(".config")
        .join("ytmtui")
        .join("config.json")
}

pub fn load() -> Config {
    let raw = match fs::read_to_string(path()) {
        Ok(s) => s,
        Err(_) => return Config::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save(cfg: &Config) -> Result<()> {
    let p = path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(cfg)?;
    fs::write(p, raw)?;
    Ok(())
}
