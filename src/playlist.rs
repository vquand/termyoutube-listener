use crate::ytdlp::Track;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playlist {
    pub tracks: Vec<Track>,
}

pub fn path() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    base.join("ytmtui").join("playlist.json")
}

pub fn load() -> Playlist {
    let raw = match fs::read_to_string(path()) {
        Ok(s) => s,
        Err(_) => return Playlist::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save(pl: &Playlist) -> Result<()> {
    let p = path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(pl)?;
    fs::write(p, raw)?;
    Ok(())
}
