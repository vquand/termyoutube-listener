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

fn yt_path() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    base.join("ytmtui").join("yt_playlist.json")
}

pub fn load_yt() -> Playlist {
    let raw = match fs::read_to_string(yt_path()) {
        Ok(s) => s,
        Err(_) => return Playlist::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_yt(pl: &Playlist) -> Result<()> {
    let p = yt_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(pl)?;
    fs::write(p, raw)?;
    Ok(())
}

fn local_path() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    base.join("ytmtui").join("local_folder.json")
}

pub fn load_local() -> Playlist {
    let raw = match fs::read_to_string(local_path()) {
        Ok(s) => s,
        Err(_) => return Playlist::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save_local(pl: &Playlist) -> Result<()> {
    let p = local_path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(pl)?;
    fs::write(p, raw)?;
    Ok(())
}
