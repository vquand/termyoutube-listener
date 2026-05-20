use crate::audio::{self, OutputDevice};
use crate::captions::{self, Cue};
use crate::clipboard;
use crate::config::{self, Config, LoopMode};
use crate::player::{Player, PlayerState};
use crate::playlist::{self, Playlist};
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
                    ' ' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | '\\' | '&' | ';' | '`' | '$' | '!' | '*' | '?'
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
        assert_eq!(normalize_path_input("\"/foo/bar baz.webm\""), "/foo/bar baz.webm");
    }

    #[test]
    fn strips_single_quotes() {
        assert_eq!(normalize_path_input("'/foo/bar baz.webm'"), "/foo/bar baz.webm");
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
    report.push_str(&format!("raw input chars: {} U+codepoints\n", raw.chars().count()));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Searching,
    Help,
    Params,
    Nerd,
    OpenFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFocus {
    Results,
    Playlist,
}

pub enum SearchEvent {
    Done(String, Result<Vec<Track>>),
}

pub enum CaptionEvent {
    Done(String, Result<Vec<Cue>>),
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
    pub captions: Vec<Cue>,
    captions_track_id: Option<String>,
    pub caption_events_rx: Receiver<CaptionEvent>,
    caption_events_tx: Sender<CaptionEvent>,
    pub config: Config,
    pub params_row: usize,
    sprites: Registry,
    pub playlist: Vec<Track>,
    pub playlist_selected: usize,
    pub focus: ListFocus,
    pub volume_popup_until: Option<Instant>,
    pub output_device: Option<OutputDevice>,
    pub device_events_rx: Receiver<Option<OutputDevice>>,
}

impl App {
    pub fn new(player: Player, config: Config, sprites: Registry, playlist: Playlist) -> Self {
        let (tx, rx) = mpsc::channel();
        let (cap_tx, cap_rx) = mpsc::channel();
        let (dev_tx, dev_rx) = mpsc::channel();
        audio::spawn_poller(dev_tx);
        let sampler = StatsSampler::new(player.pid());
        let app = Self {
            mode: Mode::Browse,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
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
            playlist_selected: 0,
            focus: ListFocus::Results,
            volume_popup_until: None,
            output_device: None,
            device_events_rx: dev_rx,
        };
        // Apply persisted volume to mpv at startup.
        let _ = app.player.set_volume(app.config.volume);
        app
    }

    pub fn focused_len(&self) -> usize {
        match self.focus {
            ListFocus::Results => self.results.len(),
            ListFocus::Playlist => self.playlist.len(),
        }
    }

    pub fn focused_selected(&self) -> usize {
        match self.focus {
            ListFocus::Results => self.selected,
            ListFocus::Playlist => self.playlist_selected,
        }
    }

    pub fn focused_tracks(&self) -> &[Track] {
        match self.focus {
            ListFocus::Results => &self.results,
            ListFocus::Playlist => &self.playlist,
        }
    }

    pub fn switch_focus(&mut self) {
        self.focus = match self.focus {
            ListFocus::Results => ListFocus::Playlist,
            ListFocus::Playlist => ListFocus::Results,
        };
    }

    pub fn add_focused_to_playlist(&mut self) {
        if self.focus != ListFocus::Results {
            return;
        }
        let Some(track) = self.results.get(self.selected).cloned() else {
            self.status = "nothing to add — search first".into();
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
        if self.focus != ListFocus::Playlist {
            return;
        }
        let removed_idx = self.playlist_selected;
        if removed_idx >= self.playlist.len() {
            return;
        }
        let removed = self.playlist.remove(removed_idx);
        // Keep `current` pointing at the same logical track.
        if let Some(cur) = self.current {
            self.current = match removed_idx.cmp(&cur) {
                std::cmp::Ordering::Less => Some(cur - 1),
                std::cmp::Ordering::Equal => None, // the playing track is gone
                std::cmp::Ordering::Greater => Some(cur),
            };
        }
        if self.playlist_selected >= self.playlist.len() && self.playlist_selected > 0 {
            self.playlist_selected -= 1;
        }
        self.persist_playlist();
        self.status = format!("kicked — {}", removed.title);
    }

    fn persist_playlist(&mut self) {
        let pl = Playlist { tracks: self.playlist.clone() };
        if let Err(e) = playlist::save(&pl) {
            self.status = format!("playlist saved (write failed: {})", e);
        }
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

    fn set_volume(&mut self, v: u8) {
        self.config.volume = v;
        let _ = self.player.set_volume(v);
        self.volume_popup_until = Some(Instant::now() + Duration::from_millis(2000));
        self.persist_config();
    }

    pub fn volume_popup_active(&self) -> bool {
        self.volume_popup_until
            .map_or(false, |t| Instant::now() < t)
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
        // Currently only one row (progress sprite). Extend by matching on params_row.
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
                    || matches!(self.caption_status, CaptionStatus::Idle | CaptionStatus::Error)
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
        thread::spawn(move || {
            let res = captions::fetch(&id);
            let _ = tx.send(CaptionEvent::Done(id, res));
        });
    }

    pub fn current_caption(&self) -> Option<&str> {
        if !self.show_captions {
            return None;
        }
        let pos = self.player.state().position;
        captions::active_cue(&self.captions, pos)
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
        if !matches!(self.mode, Mode::Searching | Mode::OpenFile) {
            return;
        }
        for c in text.chars() {
            if c == '\n' || c == '\r' {
                continue;
            }
            self.query.push(c);
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
        // Try canonicalize first; if it fails (file truly missing or NFC/NFD
        // mismatch confusing the resolver), fall back to a same-folder
        // grapheme-equivalent lookup that compares filenames after stripping
        // path normalization quirks.
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
        if !abs.is_file() {
            self.status = format!("not a file: {}", abs.display());
            return;
        }
        let path_str = abs.to_string_lossy().to_string();
        let title = abs
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path_str.clone());
        let track = Track {
            id: path_str.clone(),
            title,
            uploader: String::new(),
            duration: None,
            source: Some(path_str.clone()),
        };
        let idx = match self.playlist.iter().position(|t| t.id == track.id) {
            Some(i) => i,
            None => {
                self.playlist.push(track);
                self.persist_playlist();
                self.playlist.len() - 1
            }
        };
        self.focus = ListFocus::Playlist;
        self.playlist_selected = idx;
        self.play_at(idx);
    }

    pub fn submit_search(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.mode = Mode::Browse;
            return;
        }
        self.mode = Mode::Browse;
        self.searching = true;
        self.status = format!("Searching: {} ...", q);
        let tx = self.events_tx.clone();
        let q_clone = q.clone();
        thread::spawn(move || {
            let res = ytdlp::search(&q_clone, 20);
            let _ = tx.send(SearchEvent::Done(q_clone, res));
        });
    }

    pub fn drain_events(&mut self) {
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                SearchEvent::Done(q, Ok(tracks)) => {
                    self.searching = false;
                    self.results = tracks;
                    self.selected = 0;
                    self.status = format!("Found {} results for \"{}\".", self.results.len(), q);
                }
                SearchEvent::Done(_, Err(e)) => {
                    self.searching = false;
                    self.status = format!("Search failed: {}", e);
                }
            }
        }
        while let Ok(dev) = self.device_events_rx.try_recv() {
            self.output_device = dev;
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
        let n = self.playlist.len();
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
                self.status = "playlist finished".into();
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
            ListFocus::Playlist => self.playlist_selected = new as usize,
        }
    }

    pub fn play_selected(&mut self) {
        match self.focus {
            ListFocus::Playlist => {
                if self.playlist_selected < self.playlist.len() {
                    self.play_at(self.playlist_selected);
                }
            }
            ListFocus::Results => {
                let Some(track) = self.results.get(self.selected).cloned() else {
                    return;
                };
                let (idx, was_new) =
                    match self.playlist.iter().position(|t| t.id == track.id) {
                        Some(i) => (i, false),
                        None => {
                            self.playlist.push(track);
                            self.persist_playlist();
                            (self.playlist.len() - 1, true)
                        }
                    };
                self.play_at(idx);
                if was_new {
                    // play_at already set "[N/M] ...". Prepend a "+" so the
                    // user sees it was added.
                    self.status = format!("+ {}", self.status);
                }
            }
        }
    }

    pub fn play_at(&mut self, idx: usize) {
        if let Some(track) = self.playlist.get(idx).cloned() {
            self.current = Some(idx);
            if let Err(e) = self.player.load(&track.url()) {
                self.status = format!("mpv load failed: {}", e);
            } else {
                self.status = format!(
                    "[{}/{}] {} — {}",
                    idx + 1,
                    self.playlist.len(),
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
        let n = self.playlist.len();
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
            self.status = "end of playlist".into();
            return;
        };
        self.play_at(idx);
    }

    pub fn prev_track(&mut self) {
        let Some(i) = self.current else { return };
        let n = self.playlist.len();
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

    pub fn seek(&mut self, seconds: f64) {
        let _ = self.player.seek_relative(seconds);
    }

    pub fn player_state(&self) -> PlayerState {
        self.player.state()
    }

    pub fn current_track(&self) -> Option<&Track> {
        self.current.and_then(|i| self.playlist.get(i))
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
