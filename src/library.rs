use crate::ytdlp::Platform;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaylistEntry {
    pub url: String,
    pub title: String,
    pub platform: Platform,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub track_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaylistLibrary {
    pub entries: Vec<PlaylistEntry>,
}

impl PlaylistLibrary {
    pub fn position(&self, url: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.url == url)
    }

    /// Insert (or update count for an existing) entry. Returns the index
    /// of the touched row after the post-edit sort. Sort key: favorites
    /// first, then insertion order via a stable sort on the existing
    /// vec order.
    pub fn upsert(
        &mut self,
        url: &str,
        title: &str,
        platform: Platform,
        track_count: usize,
    ) -> usize {
        match self.position(url) {
            Some(i) => {
                self.entries[i].track_count = track_count;
                if !title.is_empty() {
                    self.entries[i].title = title.to_string();
                }
                self.entries[i].platform = platform;
            }
            None => self.entries.push(PlaylistEntry {
                url: url.to_string(),
                title: title.to_string(),
                platform,
                favorite: false,
                track_count,
            }),
        }
        self.sort();
        self.position(url).unwrap_or(0)
    }

    pub fn toggle_favorite(&mut self, idx: usize) {
        if let Some(e) = self.entries.get_mut(idx) {
            e.favorite = !e.favorite;
        }
        self.sort();
    }

    pub fn remove(&mut self, idx: usize) -> Option<PlaylistEntry> {
        if idx < self.entries.len() {
            Some(self.entries.remove(idx))
        } else {
            None
        }
    }

    /// Stable sort: favorites first, otherwise keep insertion order.
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| b.favorite.cmp(&a.favorite));
    }
}

fn path() -> PathBuf {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    base.join("ytmtui").join("playlists.json")
}

pub fn load() -> PlaylistLibrary {
    let raw = match fs::read_to_string(path()) {
        Ok(s) => s,
        Err(_) => return PlaylistLibrary::default(),
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn save(lib: &PlaylistLibrary) -> Result<()> {
    let p = path();
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(lib)?;
    fs::write(p, raw)?;
    Ok(())
}
