use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    YouTube,
    Bilibili,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub uploader: String,
    pub duration: Option<u64>,
    /// When set, this is the absolute path or URL handed straight to mpv.
    /// Used for local files opened via `o`; streaming tracks leave it
    /// `None` so `url()` builds the watch URL from `id`.
    #[serde(default)]
    pub source: Option<String>,
    /// Distance from a scanned local-folder root. `Some(0)` means the file
    /// sits directly in the folder, `Some(1)` one level down, etc. Only
    /// set for tracks discovered through folder scan; `None` for everything
    /// else.
    #[serde(default)]
    pub local_depth: Option<u8>,
    /// Origin platform. `None` is treated as YouTube for tracks without a
    /// `source` and as Local for tracks with one, so caches written
    /// before this field existed still classify correctly.
    #[serde(default)]
    pub platform: Option<Platform>,
}

impl Track {
    pub fn url(&self) -> String {
        if let Some(s) = &self.source {
            return s.clone();
        }
        match self.effective_platform() {
            Platform::Bilibili => format!("https://www.bilibili.com/video/{}", self.id),
            _ => format!("https://www.youtube.com/watch?v={}", self.id),
        }
    }

    pub fn duration_str(&self) -> String {
        match self.duration {
            Some(s) => format!("{}:{:02}", s / 60, s % 60),
            None => "--:--".to_string(),
        }
    }

    /// One-glyph source marker shown between the duration and the title in
    /// list rows. Y / B / ⌂ for YouTube / Bilibili / Local.
    pub fn source_glyph(&self) -> &'static str {
        match self.effective_platform() {
            Platform::YouTube => "Y",
            Platform::Bilibili => "B",
            Platform::Local => "⌂",
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self.effective_platform(), Platform::Local)
    }

    /// Platform with legacy-cache fallback: tracks written before the
    /// `platform` field existed default to Local when they carry a
    /// `source` path, else YouTube.
    pub fn effective_platform(&self) -> Platform {
        if let Some(p) = self.platform {
            return p;
        }
        if self.source.is_some() {
            Platform::Local
        } else {
            Platform::YouTube
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

/// Fetch every video from a public YouTube playlist URL (flat metadata
/// only, no actual download). Use this for the dedicated "YT Playlist"
/// tab.
/// Sniff which streaming platform an URL belongs to. Defaults to YouTube
/// for anything we don't explicitly recognise (yt-dlp's own extractor
/// chain handles the rest if it can).
pub fn platform_from_url(url: &str) -> Platform {
    if url.contains("bilibili.com") || url.contains("b23.tv") {
        Platform::Bilibili
    } else {
        Platform::YouTube
    }
}

pub fn fetch_playlist(url: &str) -> Result<Vec<Track>> {
    let platform = platform_from_url(url);
    let output = Command::new("yt-dlp")
        .args([
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--skip-download",
            url,
        ])
        .output()
        .context("failed to run yt-dlp (is it installed and on PATH?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp failed: {}", stderr.trim());
    }
    Ok(parse_track_jsonl(
        &String::from_utf8_lossy(&output.stdout),
        platform,
    ))
}

fn parse_track_jsonl(stdout: &str, platform: Platform) -> Vec<Track> {
    let mut tracks = Vec::new();
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
            local_depth: None,
            platform: Some(platform),
        });
    }
    tracks
}

pub fn search(query: &str, limit: usize, platform: Platform) -> Result<Vec<Track>> {
    let prefix = match platform {
        Platform::YouTube => "ytsearch",
        Platform::Bilibili => "bilisearch",
        Platform::Local => anyhow::bail!("yt-dlp cannot search local files"),
    };
    let q = format!("{}{}:{}", prefix, limit, query);
    let mut args: Vec<&str> = Vec::new();
    // YouTube's search page already exposes title / duration / uploader,
    // so the cheap --flat-playlist path is fine. Bilibili's flat results
    // are URL stubs only, so we must let yt-dlp resolve each entry to
    // populate metadata. To survive the mix of unsupported result kinds
    // (cheese / live / unsupported extractors), pass --ignore-errors and
    // accept yt-dlp's non-zero exit so we still keep the rows that did
    // resolve. The Referer header sidesteps Bilibili's HTTP 412 gate.
    if matches!(platform, Platform::YouTube) {
        args.push("--flat-playlist");
    }
    if matches!(platform, Platform::Bilibili) {
        args.extend([
            "--ignore-errors",
            "--add-header",
            "Referer: https://www.bilibili.com/",
        ]);
    }
    args.extend([
        "--default-search",
        prefix,
        "--dump-json",
        "--no-warnings",
        "--skip-download",
        &q,
    ]);
    let output = Command::new("yt-dlp")
        .args(&args)
        .output()
        .context("failed to run yt-dlp (is it installed and on PATH?)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let tracks = parse_track_jsonl(&stdout, platform);
    // Only bail if nothing came back AND yt-dlp signalled failure. Partial
    // success (some entries unsupported, others fine) is the common case
    // for Bilibili and should not look like a hard error to the user.
    if tracks.is_empty() && !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp failed: {}", stderr.trim());
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
