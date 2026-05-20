use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub uploader: String,
    pub duration: Option<u64>,
    /// When set, this is the absolute path or URL handed straight to mpv.
    /// Used for local files opened via `o`; YouTube tracks leave it `None`
    /// so `url()` builds the watch URL from `id` as before.
    #[serde(default)]
    pub source: Option<String>,
}

impl Track {
    pub fn url(&self) -> String {
        if let Some(s) = &self.source {
            return s.clone();
        }
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    pub fn duration_str(&self) -> String {
        match self.duration {
            Some(s) => format!("{}:{:02}", s / 60, s % 60),
            None => "--:--".to_string(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FlatEntry {
    id: Option<String>,
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

pub fn search(query: &str, limit: usize) -> Result<Vec<Track>> {
    let q = format!("ytsearch{}:{}", limit, query);
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--default-search",
            "ytsearch",
            "--dump-json",
            "--no-warnings",
            "--skip-download",
            &q,
        ])
        .output()
        .context("failed to run yt-dlp (is it installed and on PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tracks = Vec::with_capacity(limit);
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: FlatEntry = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(id) = entry.id else { continue };
        let title = entry.title.unwrap_or_else(|| "(untitled)".into());
        let uploader = entry
            .uploader
            .or(entry.channel)
            .unwrap_or_else(|| "(unknown)".into());
        let duration = entry.duration.map(|d| d as u64);
        tracks.push(Track {
            id,
            title,
            uploader,
            duration,
            source: None,
        });
    }
    Ok(tracks)
}

pub fn check_installed() -> Result<()> {
    Command::new("yt-dlp")
        .arg("--version")
        .output()
        .context("yt-dlp not found on PATH. Install it: `brew install yt-dlp` or see https://github.com/yt-dlp/yt-dlp")?;
    Ok(())
}

pub fn version() -> Option<String> {
    let out = Command::new("yt-dlp").arg("--version").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
