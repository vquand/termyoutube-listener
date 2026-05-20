use crate::captions::{self, Cue};
use crate::clipboard;
use crate::config::{self, Config, LoopMode};
use crate::player::{Player, PlayerState};
use crate::playlist::{self, Playlist};
use crate::sprites::{Registry, Sprite};
use crate::stats::{Stats, StatsSampler};
use crate::ytdlp::{self, Track};
use anyhow::Result;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

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
}

impl App {
    pub fn new(player: Player, config: Config, sprites: Registry, playlist: Playlist) -> Self {
        let (tx, rx) = mpsc::channel();
        let (cap_tx, cap_rx) = mpsc::channel();
        let sampler = StatsSampler::new(player.pid());
        Self {
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
        }
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
