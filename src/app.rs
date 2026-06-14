use crate::audio::{self, OutputDevice};
use crate::captions::{self, CaptionTrack};
use crate::clipboard;
use crate::config::{self, Config, LoopMode};
use crate::library::{self, PlaylistLibrary};
use crate::local_scan;
use crate::player::{Player, PlayerState};
use crate::playlist::{self, Playlist};
use crate::probe;
use crate::sprites::{Registry, Sprite};
use crate::stats::{Stats, StatsSampler};
use crate::ytdlp::{self, Track};
use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

/// Small xorshift PRNG seeded from system time. Good enough for shuffle.
fn rand_u64() -> u64 {
    static SEED: AtomicU64 = AtomicU64::new(0);
    let mut s = SEED.load(Ordering::Relaxed);
    if s == 0 {
        s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdead_beef_cafe_babe)
            | 1;
    }
    s ^= s << 13;
    s ^= s >> 7;
    s ^= s << 17;
    SEED.store(s, Ordering::Relaxed);
    s
}

fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    if input == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(input)
}

/// Massage common copy-paste artifacts out of a path string before we touch
/// the filesystem. Handles surrounding quotes (ASCII + smart), shell-style
/// backslash escapes for common metacharacters, and `file://` URIs with
/// percent-encoding.
pub fn normalize_path_input(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = strip_outer_quotes(trimmed);
    if let Some(rest) = unquoted.strip_prefix("file://") {
        // file:// URIs may be `file:///abs/path` (host empty) or
        // `file://localhost/abs/path`. Drop a leading "localhost".
        let after_host = rest.strip_prefix("localhost").unwrap_or(rest);
        return percent_decode(after_host);
    }
    unescape_shell(unquoted)
}

fn strip_outer_quotes(s: &str) -> &str {
    const PAIRS: &[(char, char)] = &[
        ('"', '"'),
        ('\'', '\''),
        ('\u{201C}', '\u{201D}'), // smart double quotes
        ('\u{2018}', '\u{2019}'), // smart single quotes
        ('`', '`'),
    ];
    for (open, close) in PAIRS {
        let mut chars = s.chars();
        if chars.next() == Some(*open) && s.chars().last() == Some(*close) && s.chars().count() >= 2
        {
            let inner = &s[open.len_utf8()..s.len() - close.len_utf8()];
            return inner;
        }
    }
    s
}

fn unescape_shell(s: &str) -> String {
    // Only unescape `\` followed by a shell metachar; preserve backslashes
    // that introduce something else (e.g. Windows path separators, escape
    // sequences we do not own).
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                if matches!(
                    next,
                    ' ' | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | '\''
                        | '"'
                        | '\\'
                        | '&'
                        | ';'
                        | '`'
                        | '$'
                        | '!'
                        | '*'
                        | '?'
                ) {
                    out.push(next);
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(a), Some(b)) = (hi, lo) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod path_input_tests {
    use super::normalize_path_input;

    #[test]
    fn strips_double_quotes() {
        assert_eq!(
            normalize_path_input("\"/foo/bar baz.webm\""),
            "/foo/bar baz.webm"
        );
    }

    #[test]
    fn strips_single_quotes() {
        assert_eq!(
            normalize_path_input("'/foo/bar baz.webm'"),
            "/foo/bar baz.webm"
        );
    }

    #[test]
    fn unescapes_shell_spaces_and_parens() {
        assert_eq!(
            normalize_path_input(r"/foo/Main\ Title\ \(2024\).webm"),
            "/foo/Main Title (2024).webm"
        );
    }

    #[test]
    fn keeps_unrelated_backslashes() {
        // Windows-style separators must survive.
        assert_eq!(normalize_path_input(r"C:\Users\foo"), r"C:\Users\foo");
    }

    #[test]
    fn decodes_file_uri() {
        assert_eq!(
            normalize_path_input("file:///foo/Main%20Title.webm"),
            "/foo/Main Title.webm"
        );
        assert_eq!(
            normalize_path_input("file://localhost/foo/Main%20Title.webm"),
            "/foo/Main Title.webm"
        );
    }

    #[test]
    fn keeps_full_width_chars() {
        // Quotes stripped, full-width punctuation preserved.
        assert_eq!(
            normalize_path_input("\"/tmp/Arknights： ｜ Theme.webm\""),
            "/tmp/Arknights： ｜ Theme.webm"
        );
    }

    #[test]
    fn passthrough_plain_path() {
        assert_eq!(normalize_path_input("/foo/bar.webm"), "/foo/bar.webm");
    }

    #[test]
    fn trims_whitespace() {
        assert_eq!(normalize_path_input("  /foo/bar.webm  "), "/foo/bar.webm");
    }
}

/// Last-resort lookup: scan the target file's parent dir for a filename
/// that matches the requested one after collapsing consecutive whitespace
/// and lowercasing. Rescues pastes that drift slightly from the on-disk
/// name (extra spaces, case differences from a fuzzy IME, etc.).
fn fuzzy_lookup(target: &std::path::Path) -> Option<PathBuf> {
    let parent = target.parent()?;
    let needle_key = fuzzy_key(&target.file_name()?.to_string_lossy());
    if needle_key.is_empty() {
        return None;
    }
    for entry in std::fs::read_dir(parent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if fuzzy_key(&name) == needle_key {
            return Some(entry.path());
        }
    }
    None
}

fn fuzzy_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.extend(c.to_lowercase());
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn write_open_debug_log(
    raw: &str,
    normalized: &str,
    expanded: &std::path::Path,
    err: &std::io::Error,
) -> PathBuf {
    let path = std::env::temp_dir().join("ytmtui-open-debug.log");
    let mut report = String::new();
    report.push_str("ytmtui open-file failure debug\n\n");
    report.push_str(&format!(
        "raw input chars: {} U+codepoints\n",
        raw.chars().count()
    ));
    report.push_str(&format!("  text: {raw:?}\n"));
    report.push_str("  codepoints:");
    for c in raw.chars() {
        report.push_str(&format!(" U+{:04X}", c as u32));
    }
    report.push_str("\n\n");
    report.push_str(&format!("normalized: {normalized:?}\n"));
    report.push_str("  codepoints:");
    for c in normalized.chars() {
        report.push_str(&format!(" U+{:04X}", c as u32));
    }
    report.push_str("\n\n");
    report.push_str(&format!("expanded path: {}\n", expanded.display()));
    report.push_str(&format!("canonicalize error: {err}\n\n"));

    if let Some(parent) = expanded.parent() {
        report.push_str(&format!("parent dir: {}\n", parent.display()));
        match std::fs::read_dir(parent) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    report.push_str(&format!("  {name}\n"));
                    report.push_str("    codepoints:");
                    for c in name.chars() {
                        report.push_str(&format!(" U+{:04X}", c as u32));
                    }
                    report.push('\n');
                }
            }
            Err(e) => {
                report.push_str(&format!("  (failed to list parent: {e})\n"));
            }
        }
    }

    let _ = std::fs::write(&path, report);
    path
}

/// Parse a search-bar query into (platforms_to_query, cleaned_query).
/// A `#Y` / `#B` / `#home` token at the start scopes the search to one
/// platform; anything else searches all three.
fn parse_search_filter(input: &str) -> (Vec<ytdlp::Platform>, String) {
    let trimmed = input.trim();
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim().to_string();
    let upper = first.to_uppercase();
    match upper.as_str() {
        "#Y" | "#YT" | "#YOUTUBE" => (vec![ytdlp::Platform::YouTube], rest),
        "#B" | "#BILI" | "#BILIBILI" => (vec![ytdlp::Platform::Bilibili], rest),
        "#H" | "#HOME" | "#LOCAL" => (vec![ytdlp::Platform::Local], rest),
        _ => (
            vec![
                ytdlp::Platform::YouTube,
                ytdlp::Platform::Bilibili,
                ytdlp::Platform::Local,
            ],
            trimmed.to_string(),
        ),
    }
}

fn platform_short(p: ytdlp::Platform) -> &'static str {
    match p {
        ytdlp::Platform::YouTube => "YT",
        ytdlp::Platform::Bilibili => "B",
        ytdlp::Platform::Local => "local",
    }
}

fn short_label(entry: &crate::library::PlaylistEntry) -> String {
    if entry.title.len() > 60 {
        format!("{}…", &entry.title[..60])
    } else {
        entry.title.clone()
    }
}

fn rand_index_excluding(n: usize, except: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let mut idx = (rand_u64() as usize) % n;
    if idx == except {
        idx = (idx + 1) % n;
    }
    idx
}

/// Spawn one background worker that probes the duration of each
/// `(track_id, source_path)` pair and emits a single batched event when
/// done. If `targets` is empty the channel is closed immediately so the
/// drain loop just sees no events.
fn spawn_duration_backfill(targets: &[(String, String)]) -> Receiver<DurationBackfillEvent> {
    let (tx, rx) = mpsc::channel();
    if targets.is_empty() {
        drop(tx);
        return rx;
    }
    let owned: Vec<(String, String)> = targets.to_vec();
    thread::spawn(move || {
        let mut updates = Vec::new();
        for (id, src) in owned {
            if let Some(d) = probe::duration(std::path::Path::new(&src)) {
                updates.push((id, d));
            }
        }
        if !updates.is_empty() {
            let _ = tx.send(DurationBackfillEvent::Done(updates));
        }
    });
    rx
}

/// Probe each track's duration in chunks across a small fixed-size thread
/// pool. ffprobe is single-threaded per file but mostly IO-bound, so a
/// handful of parallel workers cuts wall-clock significantly without
/// thrashing.
fn probe_durations_parallel(tracks: Vec<Track>) -> Vec<Track> {
    if tracks.is_empty() {
        return tracks;
    }
    const WORKERS: usize = 4;
    let chunks: Vec<Vec<Track>> = {
        let n = tracks.len();
        let stride = n.div_ceil(WORKERS);
        let mut iter = tracks.into_iter();
        (0..WORKERS)
            .map(|_| iter.by_ref().take(stride).collect())
            .filter(|c: &Vec<Track>| !c.is_empty())
            .collect()
    };
    let handles: Vec<_> = chunks
        .into_iter()
        .map(|mut chunk| {
            thread::spawn(move || {
                for t in chunk.iter_mut() {
                    if t.duration.is_some() {
                        continue;
                    }
                    if let Some(src) = &t.source {
                        t.duration = probe::duration(std::path::Path::new(src));
                    }
                }
                chunk
            })
        })
        .collect();
    let mut out = Vec::new();
    for h in handles {
        if let Ok(chunk) = h.join() {
            out.extend(chunk);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Searching,
    Help,
    Params,
    Nerd,
    OpenFile,
    YtPlaylistInput,
    SavePlaylist,
}

/// The focus targets that the Tab key cycles through. YtLibrary is the
/// Saved-Playlists sub-pane and YtPlaylist is the Tracks sub-pane of the
/// unified Library tab. The old standalone Playlist tab is gone; its
/// data now lives as the "Unsaved" virtual row inside YtLibrary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFocus {
    Results,
    YtLibrary,
    YtPlaylist,
    LocalFolder,
}

/// Which Library row is currently feeding the Tracks pane (and the
/// playback queue when Enter is pressed). Unsaved means the live
/// `playlist` working list; Saved(i) means `library.entries[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveLibrary {
    Unsaved,
    Saved(usize),
}

#[derive(Debug, Clone)]
pub enum QueueSource {
    Results,
    Unsaved,
    Saved { name: String },
    LocalFolder,
}

pub enum SearchEvent {
    Done(String, Result<Vec<Track>>),
}

pub enum YtPlaylistEvent {
    Done(String, Result<ytdlp::PlaylistFetch>),
}

pub enum LocalFolderEvent {
    Done(String, Vec<Track>),
}

pub enum DurationBackfillEvent {
    /// (track id, probed duration in seconds). Track ids for local files
    /// are absolute paths, so the same value can backfill duplicates
    /// across the Playlist and Local Folder lists in one pass.
    Done(Vec<(String, u64)>),
}

pub enum CaptionEvent {
    Done(String, Result<Vec<CaptionTrack>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionStatus {
    Idle,
    Loading,
    Ready,
    None,
    Error,
}

pub struct App {
    pub mode: Mode,
    pub query: String,
    pub results: Vec<Track>,
    pub selected: usize,
    pub queue: Vec<Track>,
    pub queue_source: QueueSource,
    pub current: Option<usize>,
    pub status: String,
    pub searching: bool,
    pub should_quit: bool,
    pub player: Player,
    pub events_rx: Receiver<SearchEvent>,
    events_tx: Sender<SearchEvent>,
    sampler: StatsSampler,
    last_stats: Stats,
    pub mpv_version: Option<String>,
    pub ytdlp_version: Option<String>,
    pub show_captions: bool,
    pub caption_status: CaptionStatus,
    pub captions: Vec<CaptionTrack>,
    captions_track_id: Option<String>,
    pub caption_events_rx: Receiver<CaptionEvent>,
    caption_events_tx: Sender<CaptionEvent>,
    pub config: Config,
    pub params_row: usize,
    sprites: Registry,
    pub playlist: Vec<Track>,
    pub focus: ListFocus,
    pub volume_popup_until: Option<Instant>,
    pub output_device: Option<OutputDevice>,
    pub device_events_rx: Receiver<Option<OutputDevice>>,
    pub yt_playlist: Vec<Track>,
    pub yt_playlist_selected: usize,
    pub yt_playlist_loading: bool,
    pub yt_playlist_events_rx: Receiver<YtPlaylistEvent>,
    yt_playlist_events_tx: Sender<YtPlaylistEvent>,
    pub local_folder: Vec<Track>,
    pub local_folder_selected: usize,
    pub local_folder_scanning: bool,
    pub local_folder_events_rx: Receiver<LocalFolderEvent>,
    local_folder_events_tx: Sender<LocalFolderEvent>,
    pub duration_backfill_rx: Receiver<DurationBackfillEvent>,
    pending_searches: usize,
    current_search_query: Option<String>,
    pub library: PlaylistLibrary,
    pub library_selected: usize,
    pub active_library: ActiveLibrary,
    pub last_saved_as: Option<String>,
}

impl App {
    pub fn new(
        player: Player,
        config: Config,
        sprites: Registry,
        playlist: Playlist,
        yt_playlist: Playlist,
        local_folder: Playlist,
        library: PlaylistLibrary,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let (cap_tx, cap_rx) = mpsc::channel();
        let (dev_tx, dev_rx) = mpsc::channel();
        let (yt_tx, yt_rx) = mpsc::channel();
        let (lf_tx, lf_rx) = mpsc::channel();
        audio::spawn_poller(dev_tx);
        let sampler = StatsSampler::new(player.pid());
        let mut app = Self {
            mode: Mode::Browse,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            queue: Vec::new(),
            queue_source: QueueSource::Unsaved,
            current: None,
            status: "Press `s` to search, `?` for help, `q` to quit.".into(),
            searching: false,
            should_quit: false,
            player,
            events_rx: rx,
            events_tx: tx,
            sampler,
            last_stats: Stats::default(),
            mpv_version: crate::player::version(),
            ytdlp_version: ytdlp::version(),
            show_captions: false,
            caption_status: CaptionStatus::Idle,
            captions: Vec::new(),
            captions_track_id: None,
            caption_events_rx: cap_rx,
            caption_events_tx: cap_tx,
            config,
            params_row: 0,
            sprites,
            playlist: playlist.tracks,
            focus: ListFocus::Results,
            volume_popup_until: None,
            output_device: None,
            device_events_rx: dev_rx,
            yt_playlist: yt_playlist.tracks,
            yt_playlist_selected: 0,
            yt_playlist_loading: false,
            yt_playlist_events_rx: yt_rx,
            yt_playlist_events_tx: yt_tx,
            local_folder: local_folder.tracks,
            local_folder_selected: 0,
            local_folder_scanning: false,
            local_folder_events_rx: lf_rx,
            local_folder_events_tx: lf_tx,
            duration_backfill_rx: spawn_duration_backfill(&[]),
            pending_searches: 0,
            current_search_query: None,
            library,
            library_selected: 0,
            active_library: ActiveLibrary::Unsaved,
            last_saved_as: None,
        };
        // Apply persisted volume to mpv at startup.
        let _ = app.player.set_volume(app.config.volume);
        // Backfill cached library totals for any local entry whose JSON
        // was written before total_duration existed.
        let mut lib_changed = false;
        for entry in app.library.entries.iter_mut() {
            if entry.total_duration.is_none() {
                if let Some(tracks) = &entry.tracks {
                    let total = library::sum_durations(tracks);
                    if total.is_some() {
                        entry.total_duration = total;
                        lib_changed = true;
                    }
                }
            }
        }
        if lib_changed {
            let _ = library::save(&app.library);
        }
        // Backfill durations for any local tracks loaded from cache that
        // were saved before the probe step existed.
        app.duration_backfill_rx = app.spawn_duration_backfill();
        app
    }

    fn spawn_duration_backfill(&self) -> Receiver<DurationBackfillEvent> {
        let mut targets: Vec<(String, String)> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for list in [&self.playlist, &self.local_folder, &self.yt_playlist] {
            for t in list.iter() {
                if t.duration.is_some() {
                    continue;
                }
                let Some(src) = t.source.as_ref() else {
                    continue;
                };
                if seen.insert(t.id.clone()) {
                    targets.push((t.id.clone(), src.clone()));
                }
            }
        }
        spawn_duration_backfill(&targets)
    }

    pub fn focused_len(&self) -> usize {
        match self.focus {
            ListFocus::Results => self.results.len(),
            ListFocus::YtLibrary => self.saved_playlist_row_count(),
            ListFocus::YtPlaylist => self.active_tracks().len(),
            ListFocus::LocalFolder => self.local_folder.len(),
        }
    }

    pub fn focused_selected(&self) -> usize {
        match self.focus {
            ListFocus::Results => self.selected,
            ListFocus::YtLibrary => self.library_selected,
            ListFocus::YtPlaylist => self.yt_playlist_selected,
            ListFocus::LocalFolder => self.local_folder_selected,
        }
    }

    pub fn focused_tracks(&self) -> &[Track] {
        match self.focus {
            ListFocus::Results => &self.results,
            ListFocus::YtLibrary => &[],
            ListFocus::YtPlaylist => self.active_tracks(),
            ListFocus::LocalFolder => &self.local_folder,
        }
    }

    pub fn switch_focus(&mut self) {
        self.focus = match self.focus {
            ListFocus::Results => ListFocus::YtLibrary,
            ListFocus::YtLibrary => ListFocus::YtPlaylist,
            ListFocus::YtPlaylist => ListFocus::LocalFolder,
            ListFocus::LocalFolder => ListFocus::Results,
        };
    }

    pub fn switch_focus_back(&mut self) {
        self.focus = match self.focus {
            ListFocus::Results => ListFocus::LocalFolder,
            ListFocus::YtLibrary => ListFocus::Results,
            ListFocus::YtPlaylist => ListFocus::YtLibrary,
            ListFocus::LocalFolder => ListFocus::YtPlaylist,
        };
    }

    /// True when the live Unsaved row should be visible in the Saved
    /// Playlists pane (it hides itself when empty so the user only ever
    /// sees a fresh row once they actually have tracks queued).
    pub fn unsaved_visible(&self) -> bool {
        !self.playlist.is_empty()
    }

    /// Total row count of the Saved Playlists pane: library entries plus
    /// the synthetic Unsaved row when non-empty.
    pub fn saved_playlist_row_count(&self) -> usize {
        self.library.entries.len() + if self.unsaved_visible() { 1 } else { 0 }
    }

    /// Map a row index in the Saved Playlists pane to either the live
    /// Unsaved row or a saved library entry index.
    pub fn saved_row_at(&self, idx: usize) -> Option<ActiveLibrary> {
        if self.unsaved_visible() {
            if idx == 0 {
                return Some(ActiveLibrary::Unsaved);
            }
            self.library
                .entries
                .get(idx - 1)
                .map(|_| ActiveLibrary::Saved(idx - 1))
        } else {
            self.library
                .entries
                .get(idx)
                .map(|_| ActiveLibrary::Saved(idx))
        }
    }

    /// The track list that feeds the Tracks pane (and play_selected when
    /// focus = YtPlaylist). Routes by `active_library`.
    pub fn active_tracks(&self) -> &[Track] {
        match &self.active_library {
            ActiveLibrary::Unsaved => &self.playlist,
            ActiveLibrary::Saved(i) => {
                let Some(entry) = self.library.entries.get(*i) else {
                    return &[];
                };
                match &entry.tracks {
                    Some(t) => t.as_slice(),
                    None => &self.yt_playlist,
                }
            }
        }
    }

    pub fn active_title(&self) -> String {
        match &self.active_library {
            ActiveLibrary::Unsaved => "Unsaved".to_string(),
            ActiveLibrary::Saved(i) => self
                .library
                .entries
                .get(*i)
                .map(|e| e.title.clone())
                .unwrap_or_else(|| "Saved Playlist".into()),
        }
    }

    pub fn add_focused_to_playlist(&mut self) {
        let track = match self.focus {
            ListFocus::Results => self.results.get(self.selected).cloned(),
            ListFocus::YtPlaylist => self.active_tracks().get(self.yt_playlist_selected).cloned(),
            ListFocus::LocalFolder => self.local_folder.get(self.local_folder_selected).cloned(),
            ListFocus::YtLibrary => {
                return;
            }
        };
        let Some(track) = track else {
            self.status = "nothing to add — pick a track first".into();
            return;
        };
        if self.playlist.iter().any(|t| t.id == track.id) {
            self.status = format!("not added — already on the list ({})", track.title);
            return;
        }
        let title = track.title.clone();
        self.playlist.push(track);
        self.persist_playlist();
        self.status = format!("added — {}", title);
    }

    pub fn remove_focused_from_playlist(&mut self) {
        // - only operates on the live Unsaved row, viewed inside the
        // Tracks pane. Saved entries are immutable snapshots.
        if self.focus != ListFocus::YtPlaylist
            || !matches!(self.active_library, ActiveLibrary::Unsaved)
        {
            return;
        }
        let removed_idx = self.yt_playlist_selected;
        if removed_idx >= self.playlist.len() {
            return;
        }
        let removed = self.playlist.remove(removed_idx);
        if self.yt_playlist_selected >= self.playlist.len() && self.yt_playlist_selected > 0 {
            self.yt_playlist_selected -= 1;
        }
        self.persist_playlist();
        self.status = format!("kicked - {}", removed.title);
    }

    fn persist_playlist(&mut self) {
        let pl = Playlist {
            tracks: self.playlist.clone(),
        };
        if let Err(e) = playlist::save(&pl) {
            self.status = format!("playlist saved (write failed: {})", e);
        }
    }

    fn persist_yt_playlist(&mut self) {
        let pl = Playlist {
            tracks: self.yt_playlist.clone(),
        };
        if let Err(e) = playlist::save_yt(&pl) {
            self.status = format!("yt playlist saved (write failed: {})", e);
        }
    }

    fn persist_local_folder(&mut self) {
        let pl = Playlist {
            tracks: self.local_folder.clone(),
        };
        if let Err(e) = playlist::save_local(&pl) {
            self.status = format!("local folder saved (write failed: {})", e);
        }
    }

    fn start_local_folder_scan(&mut self, path: PathBuf) {
        let folder_str = path.to_string_lossy().to_string();
        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| folder_str.clone());
        self.config.local_folder = Some(folder_str.clone());
        self.config.local_folder_label = Some(label.clone());
        self.persist_config();
        self.local_folder_scanning = true;
        self.focus = ListFocus::LocalFolder;
        self.local_folder_selected = 0;
        self.status = format!("Scanning {}...", folder_str);
        let tx = self.local_folder_events_tx.clone();
        let path_clone = path.clone();
        let folder_key = folder_str.clone();
        thread::spawn(move || {
            let mut tracks = local_scan::scan_folder(&path_clone);
            // Probe each file's duration so the list column is filled in
            // before the user clicks Play. ffprobe takes ~50ms per file,
            // so we run probes across a small thread pool to keep big
            // libraries under control.
            tracks = probe_durations_parallel(tracks);
            let _ = tx.send(LocalFolderEvent::Done(folder_key, tracks));
        });
    }

    pub fn cycle_loop(&mut self) {
        self.config.loop_mode = self.config.loop_mode.next();
        self.persist_config();
        self.status = format!("loop: {}", self.config.loop_mode.label());
    }

    pub fn toggle_shuffle(&mut self) {
        self.config.shuffle = !self.config.shuffle;
        self.persist_config();
        self.status = format!(
            "shuffle: {}",
            if self.config.shuffle { "on" } else { "off" }
        );
    }

    pub fn toggle_shortcuts(&mut self) {
        self.config.show_shortcuts = !self.config.show_shortcuts;
        self.persist_config();
    }

    pub fn volume_up(&mut self) {
        self.set_volume(self.config.volume.saturating_add(10).min(100));
    }

    pub fn volume_down(&mut self) {
        self.set_volume(self.config.volume.saturating_sub(10));
    }

    pub fn set_volume(&mut self, v: u8) {
        self.config.volume = v;
        let _ = self.player.set_volume(v);
        self.volume_popup_until = Some(Instant::now() + Duration::from_millis(2000));
        self.persist_config();
    }

    pub fn volume_popup_active(&self) -> bool {
        self.volume_popup_until
            .map_or(false, |t| Instant::now() < t)
    }

    pub fn save_config(&mut self) {
        self.persist_config();
    }

    fn persist_config(&mut self) {
        if let Err(e) = config::save(&self.config) {
            self.status = format!("config write failed: {}", e);
        }
    }

    pub fn current_sprite(&self) -> &Sprite {
        self.sprites.get(&self.config.progress_sprite)
    }

    pub fn sprites(&self) -> &Registry {
        &self.sprites
    }

    pub fn open_params(&mut self) {
        self.mode = Mode::Params;
        self.params_row = 0;
    }

    pub fn close_params(&mut self) {
        self.mode = Mode::Browse;
    }

    pub fn params_change(&mut self, delta: i32) {
        match self.params_row {
            0 => self.params_cycle_sprite(delta),
            1 => self.params_cycle_caption_lang(delta),
            _ => {}
        }
    }

    pub fn params_move(&mut self, delta: i32) {
        let row_count: i32 = 2;
        let mut new = self.params_row as i32 + delta;
        if new < 0 {
            new = 0;
        }
        if new >= row_count {
            new = row_count - 1;
        }
        self.params_row = new as usize;
    }

    fn params_cycle_sprite(&mut self, delta: i32) {
        let new_id = match delta.signum() {
            -1 => self.sprites.prev_id(&self.config.progress_sprite),
            _ => self.sprites.next_id(&self.config.progress_sprite),
        };
        self.config.progress_sprite = new_id;
        let name = self.sprites.get(&self.config.progress_sprite).name.clone();
        if let Err(e) = config::save(&self.config) {
            self.status = format!("Saved cursor (config write failed: {})", e);
        } else {
            self.status = format!("Cursor: {}", name);
        }
    }

    fn params_cycle_caption_lang(&mut self, delta: i32) {
        let langs = config::CAPTION_LANGS;
        let cur = langs
            .iter()
            .position(|l| *l == self.config.caption_lang.as_str())
            .unwrap_or(0);
        let next = if delta.signum() < 0 {
            (cur + langs.len() - 1) % langs.len()
        } else {
            (cur + 1) % langs.len()
        };
        self.config.caption_lang = langs[next].to_string();
        self.config.caption_langs = vec![self.config.caption_lang.clone()];
        if let Err(e) = config::save(&self.config) {
            self.status = format!("Saved CC lang (config write failed: {})", e);
        } else {
            self.status = format!("CC language: {}", self.config.caption_lang);
        }
        if self.show_captions {
            if let Some(track) = self.current_track().cloned() {
                self.spawn_caption_fetch(&track.id);
            }
        }
    }

    pub fn toggle_nerd(&mut self) {
        self.mode = match self.mode {
            Mode::Nerd => Mode::Browse,
            _ => Mode::Nerd,
        };
    }

    pub fn toggle_captions(&mut self) {
        self.show_captions = !self.show_captions;
        if self.show_captions {
            // Lazy fetch: if we have a current track but never loaded captions for it.
            if let Some(track) = self.current_track().cloned() {
                if self.captions_track_id.as_deref() != Some(&track.id)
                    || matches!(
                        self.caption_status,
                        CaptionStatus::Idle | CaptionStatus::Error
                    )
                {
                    self.spawn_caption_fetch(&track.id);
                }
            }
        }
    }

    fn spawn_caption_fetch(&mut self, track_id: &str) {
        self.caption_status = CaptionStatus::Loading;
        self.captions.clear();
        self.captions_track_id = Some(track_id.to_string());
        let tx = self.caption_events_tx.clone();
        let id = track_id.to_string();
        let langs = self.config.preferred_caption_langs();
        let options = captions::FetchOptions {
            cookies_from_browser: self.config.ytdlp_cookies_from_browser.clone(),
            cookies: self.config.ytdlp_cookies.clone(),
        };
        thread::spawn(move || {
            let res = captions::fetch(&id, &langs, &options);
            let _ = tx.send(CaptionEvent::Done(id, res));
        });
    }

    pub fn current_captions(&self) -> Vec<&str> {
        if !self.show_captions {
            return Vec::new();
        }
        let pos = self.player.state().position;
        self.captions
            .iter()
            .filter_map(|track| captions::active_cue(&track.cues, pos))
            .take(2)
            .collect()
    }

    pub fn refresh_stats(&mut self) {
        self.last_stats = self.sampler.sample();
    }

    pub fn stats(&self) -> &Stats {
        &self.last_stats
    }

    pub fn enter_search(&mut self) {
        self.mode = Mode::Searching;
        self.focus = ListFocus::Results;
        self.query.clear();
        self.status = "Type your search and press Enter. Esc to cancel.".into();
    }

    pub fn cancel_search(&mut self) {
        self.mode = Mode::Browse;
        self.status.clear();
    }

    pub fn enter_open_file(&mut self) {
        self.mode = Mode::OpenFile;
        self.query.clear();
        self.status = "Type a file path (~ ok) and press Enter. Esc to cancel.".into();
    }

    /// Append a bracketed-paste payload to whichever input is active. We
    /// strip embedded newlines so a multi-line clipboard does not fire
    /// Enter mid-paste, which used to truncate long file paths that
    /// wrapped on a narrow terminal.
    pub fn handle_paste(&mut self, text: &str) {
        if !matches!(
            self.mode,
            Mode::Searching | Mode::OpenFile | Mode::YtPlaylistInput | Mode::SavePlaylist
        ) {
            return;
        }
        for c in text.chars() {
            if c == '\n' || c == '\r' {
                continue;
            }
            self.query.push(c);
        }
    }

    pub fn enter_yt_playlist_input(&mut self) {
        self.mode = Mode::YtPlaylistInput;
        self.query.clear();
        if let Some(current) = self.config.yt_playlist_url.clone() {
            self.query = current;
        }
        self.status =
            "Paste a YouTube or Bilibili playlist URL and press Enter. Esc to cancel.".into();
    }

    pub fn enter_save_playlist(&mut self) {
        if self.focus != ListFocus::YtLibrary {
            return;
        }
        if self.playlist.is_empty() {
            self.status = "nothing to save - Unsaved is empty".into();
            return;
        }
        self.mode = Mode::SavePlaylist;
        self.query.clear();
        // Default name: the last save target (so re-pressing S on the
        // same Unsaved overwrites it by default).
        if let Some(name) = self.last_saved_as.clone() {
            self.query = name;
        }
        self.status = "Save Unsaved as: type a name. Same name overwrites.".into();
    }

    pub fn cancel_save_playlist(&mut self) {
        self.mode = Mode::Browse;
        self.query.clear();
        self.status.clear();
    }

    pub fn submit_save_playlist(&mut self) {
        let name = self.query.trim().to_string();
        self.mode = Mode::Browse;
        self.query.clear();
        if name.is_empty() {
            self.status = "save cancelled - empty name".into();
            return;
        }
        let tracks_snapshot = self.playlist.clone();
        let count = tracks_snapshot.len();

        // Look for an existing local entry with the same display name and
        // overwrite in place; otherwise create a new one.
        let existing = self
            .library
            .entries
            .iter()
            .position(|e| e.title == name && matches!(e.platform, ytdlp::Platform::Local));
        let saved_idx = if let Some(i) = existing {
            let total = library::sum_durations(&tracks_snapshot);
            self.library.entries[i].tracks = Some(tracks_snapshot);
            self.library.entries[i].track_count = count;
            self.library.entries[i].total_duration = total;
            i
        } else {
            self.library.insert_local(&name, tracks_snapshot)
        };
        let _ = library::save(&self.library);
        self.last_saved_as = Some(name.clone());
        self.active_library = ActiveLibrary::Saved(saved_idx);
        self.yt_playlist_selected = 0;

        // Per spec: after save, Unsaved is no longer Unsaved - clear it.
        self.playlist.clear();
        self.persist_playlist();

        // Snap the Saved Playlists cursor onto the new entry. Library
        // sort may have moved it; recompute its row.
        if let Some(i) = self
            .library
            .entries
            .iter()
            .position(|e| matches!(e.platform, ytdlp::Platform::Local) && e.title == name)
        {
            self.active_library = ActiveLibrary::Saved(i);
            self.library_selected = i + if self.unsaved_visible() { 1 } else { 0 };
        }
        if existing.is_some() {
            self.status = format!("overwritten: {} ({} tracks)", name, count);
        } else {
            self.status = format!("saved: {} ({} tracks)", name, count);
        }
    }

    pub fn cancel_yt_playlist_input(&mut self) {
        self.mode = Mode::Browse;
        self.query.clear();
        self.status.clear();
    }

    pub fn submit_yt_playlist(&mut self) {
        let raw = self.query.trim().to_string();
        self.mode = Mode::Browse;
        self.query.clear();
        if raw.is_empty() {
            return;
        }
        let url = normalize_path_input(&raw); // strips quotes / shell escapes
        self.start_yt_playlist_fetch(url);
    }

    /// Kick off (or restart) the fetch for `url`, set it as the active
    /// playlist, and pre-register a library entry so the user sees the
    /// URL show up in the library list immediately.
    fn start_yt_playlist_fetch(&mut self, url: String) {
        let platform = ytdlp::platform_from_url(&url);
        let provisional_title = url.clone();
        self.library.upsert(&url, &provisional_title, platform, 0);
        let _ = library::save(&self.library);
        self.library_selected = self.library.position(&url).unwrap_or(0);

        self.config.yt_playlist_url = Some(url.clone());
        self.persist_config();
        self.yt_playlist_loading = true;
        self.focus = ListFocus::YtPlaylist;
        self.yt_playlist_selected = 0;
        self.status = format!("Fetching {} playlist...", platform_short(platform));
        let tx = self.yt_playlist_events_tx.clone();
        let url_clone = url.clone();
        thread::spawn(move || {
            let res = ytdlp::fetch_playlist(&url_clone);
            let _ = tx.send(YtPlaylistEvent::Done(url_clone, res));
        });
    }

    pub fn activate_library_entry(&mut self) {
        let row = self.library_selected;
        match self.saved_row_at(row) {
            Some(ActiveLibrary::Unsaved) => {
                self.active_library = ActiveLibrary::Unsaved;
                self.yt_playlist_selected = 0;
                self.status = "Active: Unsaved".to_string();
            }
            Some(ActiveLibrary::Saved(i)) => {
                let entry = self.library.entries[i].clone();
                if entry.tracks.is_some() {
                    // Local saved playlist - no fetch needed; tracks live
                    // inline on the entry.
                    self.active_library = ActiveLibrary::Saved(i);
                    self.yt_playlist_selected = 0;
                    self.status = format!("Active: {} (local)", entry.title);
                } else {
                    // Remote (YT / Bilibili) - kick off a refetch.
                    self.active_library = ActiveLibrary::Saved(i);
                    self.start_yt_playlist_fetch(entry.url);
                }
            }
            None => {}
        }
    }

    pub fn toggle_library_favorite(&mut self) {
        if self.focus != ListFocus::YtLibrary {
            return;
        }
        // Map the selected row to a saved entry. The Unsaved row has no
        // favorite state.
        let saved_idx = match self.saved_row_at(self.library_selected) {
            Some(ActiveLibrary::Saved(i)) => i,
            _ => return,
        };
        let url = self.library.entries.get(saved_idx).map(|e| e.url.clone());
        self.library.toggle_favorite(saved_idx);
        let _ = library::save(&self.library);
        if let Some(u) = url {
            if let Some(new_idx) = self.library.position(&u) {
                // Saved entries sit below Unsaved when it is visible.
                self.library_selected = new_idx + if self.unsaved_visible() { 1 } else { 0 };
            }
        }
    }

    pub fn remove_library_entry(&mut self) {
        if self.focus != ListFocus::YtLibrary {
            return;
        }
        // Unsaved row is not removable.
        let saved_idx = match self.saved_row_at(self.library_selected) {
            Some(ActiveLibrary::Saved(i)) => i,
            _ => return,
        };
        if let Some(removed) = self.library.remove(saved_idx) {
            let _ = library::save(&self.library);
            // If we removed the active entry, fall back to Unsaved.
            if matches!(&self.active_library, ActiveLibrary::Saved(i) if *i == saved_idx) {
                self.active_library = ActiveLibrary::Unsaved;
            }
            // Renumber active if a lower-indexed entry was deleted.
            if let ActiveLibrary::Saved(i) = &mut self.active_library {
                if *i > saved_idx {
                    *i -= 1;
                }
            }
            let total = self.saved_playlist_row_count();
            if self.library_selected >= total && total > 0 {
                self.library_selected = total - 1;
            }
            self.status = format!("removed from library: {}", short_label(&removed));
        }
    }

    pub fn cancel_open_file(&mut self) {
        self.mode = Mode::Browse;
        self.query.clear();
        self.status.clear();
    }

    pub fn submit_open_file(&mut self) {
        let raw = self.query.trim().to_string();
        self.mode = Mode::Browse;
        self.query.clear();
        if raw.is_empty() {
            return;
        }
        let normalized = normalize_path_input(&raw);
        let expanded = expand_tilde(&normalized);
        let abs = match std::fs::canonicalize(&expanded) {
            Ok(p) => p,
            Err(e) => match fuzzy_lookup(&expanded) {
                Some(p) => p,
                None => {
                    let log_path = write_open_debug_log(&raw, &normalized, &expanded, &e);
                    self.status = format!(
                        "not found: {} -- debug log at {}",
                        expanded.display(),
                        log_path.display()
                    );
                    return;
                }
            },
        };
        if abs.is_dir() {
            self.start_local_folder_scan(abs);
            return;
        }
        if !abs.is_file() {
            self.status = format!("not a file or folder: {}", abs.display());
            return;
        }
        let path_str = abs.to_string_lossy().to_string();
        let title = abs
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.clone());
        let duration = probe::duration(&abs);
        let track = Track {
            id: path_str.clone(),
            title,
            uploader: String::new(),
            duration,
            source: Some(path_str.clone()),
            local_depth: None,
            platform: Some(ytdlp::Platform::Local),
        };
        let idx = match self.playlist.iter().position(|t| t.id == track.id) {
            Some(i) => {
                // Backfill duration if a previous open stored None.
                if self.playlist[i].duration.is_none() && track.duration.is_some() {
                    self.playlist[i].duration = track.duration;
                    self.persist_playlist();
                }
                i
            }
            None => {
                self.playlist.push(track);
                self.persist_playlist();
                self.playlist.len() - 1
            }
        };
        // Land the user in the Tracks pane viewing Unsaved with the new
        // file selected. play_at handles starting playback against the
        // captured queue.
        self.focus = ListFocus::YtPlaylist;
        self.active_library = ActiveLibrary::Unsaved;
        self.yt_playlist_selected = idx;
        self.queue = self.playlist.clone();
        self.queue_source = QueueSource::Unsaved;
        self.play_at(idx);
    }

    pub fn submit_search(&mut self) {
        let raw = self.query.trim().to_string();
        if raw.is_empty() {
            self.mode = Mode::Browse;
            return;
        }
        self.mode = Mode::Browse;
        let (platforms, query) = parse_search_filter(&raw);
        if query.is_empty() {
            self.status = "Empty search after filter".into();
            return;
        }

        // Reset state for this new search.
        self.results.clear();
        self.selected = 0;
        self.current_search_query = Some(query.clone());
        self.pending_searches = 0;
        self.searching = true;
        let scope_label = if platforms.len() == 3 {
            "all".to_string()
        } else {
            platforms
                .iter()
                .map(|p| match p {
                    ytdlp::Platform::YouTube => "Y",
                    ytdlp::Platform::Bilibili => "B",
                    ytdlp::Platform::Local => "⌂",
                })
                .collect::<Vec<_>>()
                .join("+")
        };
        self.status = format!("Searching ({}): {} ...", scope_label, query);

        // Local search is synchronous and instant.
        if platforms.contains(&ytdlp::Platform::Local) {
            let q_lower = query.to_lowercase();
            let mut local_hits: Vec<Track> = self
                .local_folder
                .iter()
                .filter(|t| t.title.to_lowercase().contains(&q_lower))
                .cloned()
                .collect();
            self.results.append(&mut local_hits);
        }

        // YouTube and Bilibili in parallel background threads.
        for platform in platforms {
            if matches!(platform, ytdlp::Platform::Local) {
                continue;
            }
            let tx = self.events_tx.clone();
            let q_clone = query.clone();
            self.pending_searches += 1;
            thread::spawn(move || {
                let res = ytdlp::search(&q_clone, 20, platform);
                let _ = tx.send(SearchEvent::Done(q_clone, res));
            });
        }

        if self.pending_searches == 0 {
            self.searching = false;
            self.status = format!("Local results: {} for \"{}\".", self.results.len(), query);
        }
    }

    pub fn drain_events(&mut self) {
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                SearchEvent::Done(q, res) => {
                    // Ignore stale results from a previous query.
                    if self.current_search_query.as_deref() != Some(q.as_str()) {
                        continue;
                    }
                    match res {
                        Ok(mut tracks) => {
                            self.results.append(&mut tracks);
                        }
                        Err(e) => {
                            self.status = format!("Search failed: {}", e);
                        }
                    }
                    if self.pending_searches > 0 {
                        self.pending_searches -= 1;
                    }
                    if self.pending_searches == 0 {
                        self.searching = false;
                        self.status =
                            format!("Found {} results for \"{}\".", self.results.len(), q);
                    }
                }
            }
        }
        while let Ok(dev) = self.device_events_rx.try_recv() {
            self.output_device = dev;
        }
        while let Ok(ev) = self.yt_playlist_events_rx.try_recv() {
            let YtPlaylistEvent::Done(url, res) = ev;
            // Ignore stale fetches (user already swapped to another URL).
            if self.config.yt_playlist_url.as_deref() != Some(&url) {
                continue;
            }
            self.yt_playlist_loading = false;
            match res {
                Ok(fetched) => {
                    self.yt_playlist = fetched.tracks;
                    self.yt_playlist_selected = 0;
                    self.persist_yt_playlist();
                    // Refresh the library entry with the real title +
                    // count + cached total duration so the row in the
                    // Saved Playlists pane shows length without a
                    // re-fetch.
                    let platform = ytdlp::platform_from_url(&url);
                    let title = fetched.title.unwrap_or_else(|| url.clone());
                    let idx = self
                        .library
                        .upsert(&url, &title, platform, self.yt_playlist.len());
                    let total = library::sum_durations(&self.yt_playlist);
                    self.library.set_total_duration(idx, total);
                    let _ = library::save(&self.library);
                    self.library_selected = self.library.position(&url).unwrap_or(0);
                    self.status = format!(
                        "{} playlist loaded ({} tracks)",
                        platform_short(platform),
                        self.yt_playlist.len()
                    );
                }
                Err(e) => {
                    self.status = format!("YT playlist failed: {}", e);
                }
            }
        }
        while let Ok(ev) = self.local_folder_events_rx.try_recv() {
            let LocalFolderEvent::Done(folder, tracks) = ev;
            // Stale: a fresher scan was started against another folder.
            if self.config.local_folder.as_deref() != Some(&folder) {
                continue;
            }
            self.local_folder_scanning = false;
            self.local_folder = tracks;
            self.local_folder_selected = 0;
            self.persist_local_folder();
            self.status = format!("Folder scan: {} files", self.local_folder.len());
        }
        while let Ok(ev) = self.duration_backfill_rx.try_recv() {
            let DurationBackfillEvent::Done(updates) = ev;
            let mut changed_playlist = false;
            let mut changed_local = false;
            let mut changed_library = false;
            for (id, dur) in updates {
                for t in self.playlist.iter_mut() {
                    if t.id == id && t.duration.is_none() {
                        t.duration = Some(dur);
                        changed_playlist = true;
                    }
                }
                for t in self.local_folder.iter_mut() {
                    if t.id == id && t.duration.is_none() {
                        t.duration = Some(dur);
                        changed_local = true;
                    }
                }
                for t in self.queue.iter_mut() {
                    if t.id == id && t.duration.is_none() {
                        t.duration = Some(dur);
                    }
                }
                // Local saved playlists store tracks inline; backfill
                // their durations too so the cached totals get refreshed
                // below.
                for entry in self.library.entries.iter_mut() {
                    if let Some(tracks) = entry.tracks.as_mut() {
                        for t in tracks.iter_mut() {
                            if t.id == id && t.duration.is_none() {
                                t.duration = Some(dur);
                                changed_library = true;
                            }
                        }
                    }
                }
            }
            if changed_playlist {
                self.persist_playlist();
            }
            if changed_local {
                self.persist_local_folder();
            }
            if changed_library {
                // Recompute cached totals for affected entries.
                for entry in self.library.entries.iter_mut() {
                    if let Some(tracks) = &entry.tracks {
                        entry.total_duration = library::sum_durations(tracks);
                    }
                }
                let _ = library::save(&self.library);
            }
        }
        while let Ok(ev) = self.caption_events_rx.try_recv() {
            let CaptionEvent::Done(id, res) = ev;
            // Ignore stale fetches (user already skipped to another track).
            if self.captions_track_id.as_deref() != Some(&id) {
                continue;
            }
            match res {
                Ok(cues) if cues.is_empty() => {
                    self.caption_status = CaptionStatus::None;
                    self.captions.clear();
                }
                Ok(cues) => {
                    self.caption_status = CaptionStatus::Ready;
                    self.captions = cues;
                }
                Err(_) => {
                    self.caption_status = CaptionStatus::Error;
                    self.captions.clear();
                }
            }
        }
        // auto-advance on track end
        let st = self.player.state();
        if st.eof_reached && self.current.is_some() {
            self.advance_after_eof();
        }
    }

    fn advance_after_eof(&mut self) {
        let Some(cur) = self.current else { return };
        let n = self.queue.len();
        if n == 0 {
            self.current = None;
            return;
        }
        let next = match self.config.loop_mode {
            LoopMode::One => Some(cur),
            _ if self.config.shuffle => Some(rand_index_excluding(n, cur)),
            LoopMode::All => Some((cur + 1) % n),
            LoopMode::Off => {
                if cur + 1 < n {
                    Some(cur + 1)
                } else {
                    None
                }
            }
        };
        match next {
            Some(idx) => self.play_at(idx),
            None => {
                self.current = None;
                self.status = "queue finished".into();
            }
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        let len = self.focused_len() as i32;
        if len == 0 {
            return;
        }
        let cur = self.focused_selected() as i32;
        let mut new = cur + delta;
        if new < 0 {
            new = 0;
        }
        if new >= len {
            new = len - 1;
        }
        match self.focus {
            ListFocus::Results => self.selected = new as usize,
            ListFocus::YtLibrary => self.library_selected = new as usize,
            ListFocus::YtPlaylist => self.yt_playlist_selected = new as usize,
            ListFocus::LocalFolder => self.local_folder_selected = new as usize,
        }
    }

    pub fn play_selected(&mut self) {
        // Saved-Playlists pane is a chooser; Enter activates the row.
        if matches!(self.focus, ListFocus::YtLibrary) {
            self.activate_library_entry();
            return;
        }
        let (source_tracks, source, sel_idx, mirror) = match self.focus {
            ListFocus::Results => (
                self.results.clone(),
                QueueSource::Results,
                self.selected,
                true,
            ),
            ListFocus::YtPlaylist => {
                let tracks = self.active_tracks().to_vec();
                let src = match &self.active_library {
                    ActiveLibrary::Unsaved => QueueSource::Unsaved,
                    ActiveLibrary::Saved(i) => QueueSource::Saved {
                        name: self
                            .library
                            .entries
                            .get(*i)
                            .map(|e| e.title.clone())
                            .unwrap_or_else(|| "Saved Playlist".into()),
                    },
                };
                // Tracks pane viewing Unsaved already IS the Unsaved list,
                // so no mirror copy is needed for that case.
                let mirror = !matches!(self.active_library, ActiveLibrary::Unsaved);
                (tracks, src, self.yt_playlist_selected, mirror)
            }
            ListFocus::LocalFolder => (
                self.local_folder.clone(),
                QueueSource::LocalFolder,
                self.local_folder_selected,
                true,
            ),
            ListFocus::YtLibrary => return, // handled above
        };
        let Some(track) = source_tracks.get(sel_idx).cloned() else {
            return;
        };
        self.queue = source_tracks;
        self.queue_source = source;

        // Mirror the selected track into the live Unsaved list for memory
        // / visibility unless we are already playing from Unsaved itself.
        let was_new = if mirror && !self.playlist.iter().any(|t| t.id == track.id) {
            self.playlist.push(track);
            self.persist_playlist();
            true
        } else {
            false
        };

        self.play_at(sel_idx);
        if was_new {
            self.status = format!("+ {}", self.status);
        }
    }

    pub fn play_at(&mut self, idx: usize) {
        if let Some(track) = self.queue.get(idx).cloned() {
            self.current = Some(idx);
            if let Err(e) = self.player.load(&track.url()) {
                self.status = format!("mpv load failed: {}", e);
            } else {
                self.status = format!(
                    "[{}/{}] {} — {}",
                    idx + 1,
                    self.queue.len(),
                    track.title,
                    track.uploader
                );
            }
            // Reset captions for the new track; only fetch if the overlay is on.
            self.captions.clear();
            self.captions_track_id = None;
            self.caption_status = CaptionStatus::Idle;
            if self.show_captions {
                self.spawn_caption_fetch(&track.id);
            }
        }
    }

    pub fn next_track(&mut self) {
        let Some(i) = self.current else { return };
        let n = self.queue.len();
        if n == 0 {
            return;
        }
        let idx = if self.config.shuffle {
            rand_index_excluding(n, i)
        } else if i + 1 < n {
            i + 1
        } else if self.config.loop_mode == LoopMode::All {
            0
        } else {
            self.status = "end of queue".into();
            return;
        };
        self.play_at(idx);
    }

    pub fn prev_track(&mut self) {
        let Some(i) = self.current else { return };
        let n = self.queue.len();
        if n == 0 {
            return;
        }
        let idx = if self.config.shuffle {
            rand_index_excluding(n, i)
        } else if i > 0 {
            i - 1
        } else if self.config.loop_mode == LoopMode::All {
            n - 1
        } else {
            0 // restart first track
        };
        self.play_at(idx);
    }

    pub fn toggle_pause(&mut self) {
        let _ = self.player.toggle_pause();
    }

    pub fn pause(&mut self) {
        let _ = self.player.set_pause(true);
    }

    pub fn play(&mut self) {
        if self.current_track().is_some() {
            let _ = self.player.set_pause(false);
        }
    }

    pub fn seek(&mut self, seconds: f64) {
        let _ = self.player.seek_relative(seconds);
    }

    pub fn player_state(&self) -> PlayerState {
        self.player.state()
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.queue.get(i))
    }

    pub fn yank_selected_url(&mut self) {
        let idx = self.focused_selected();
        let tracks = self.focused_tracks();
        let Some(track) = tracks.get(idx) else {
            self.status = "nothing to copy".into();
            return;
        };
        let url = track.url();
        match clipboard::copy(&url) {
            Ok(_tool) => self.status = format!("Copied URL: {}", url),
            Err(e) => self.status = format!("Copy failed: {}", e),
        }
    }
}
