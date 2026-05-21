use crate::ytdlp::{Platform, Track};
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
    /// Inline track list for Local playlists; remote (YT / Bilibili)
    /// entries leave this `None` and fetch on activate.
    #[serde(default)]
    pub tracks: Option<Vec<Track>>,
    /// Sum of every track's duration in whole seconds. Cached at upsert /
    /// insert time so the Saved Playlists row can show a total without
    /// touching the network. `None` when no track in the playlist
    /// reported a duration yet.
    #[serde(default)]
    pub total_duration: Option<u64>,
}

/// Sum the durations on a slice of tracks. Returns `None` when no track
/// reported a duration (zero-sum vs. unknown collapses to None so the
/// UI can render `--` rather than `0:00`).
pub fn sum_durations(tracks: &[Track]) -> Option<u64> {
    let mut total: u64 = 0;
    let mut any = false;
    for t in tracks {
        if let Some(d) = t.duration {
            total = total.saturating_add(d);
            any = true;
        }
    }
    if any { Some(total) } else { None }
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
                tracks: None,
                total_duration: None,
            }),
        }
        self.sort();
        self.position(url).unwrap_or(0)
    }

    /// Insert a new local saved playlist (a snapshot of an Unsaved list).
    /// Returns the row index of the new entry after the post-edit sort.
    /// The synthetic URL `local:<title>:<count>` is just an internal key
    /// so the upsert dedupe logic still works.
    pub fn insert_local(&mut self, title: &str, tracks: Vec<Track>) -> usize {
        let url = format!(
            "local:{}:{}",
            title,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        );
        let count = tracks.len();
        let total = sum_durations(&tracks);
        self.entries.push(PlaylistEntry {
            url: url.clone(),
            title: title.to_string(),
            platform: Platform::Local,
            favorite: false,
            track_count: count,
            tracks: Some(tracks),
            total_duration: total,
        });
        self.sort();
        self.position(&url).unwrap_or(0)
    }

    /// Set the cached total duration on a row. Used by the remote-fetch
    /// completion path that knows the tracks but doesn't own them.
    pub fn set_total_duration(&mut self, idx: usize, total: Option<u64>) {
        if let Some(e) = self.entries.get_mut(idx) {
            e.total_duration = total;
        }
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
