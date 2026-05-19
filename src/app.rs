use crate::captions::{self, Cue};
use crate::clipboard;
use crate::player::{Player, PlayerState};
use crate::stats::{Stats, StatsSampler};
use crate::ytdlp::{self, Track};
use anyhow::Result;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Searching,
    Help,
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
    pub queue: Vec<Track>,
    pub current: Option<usize>,
    pub status: String,
    pub searching: bool,
    pub should_quit: bool,
    pub player: Player,
    pub events_rx: Receiver<SearchEvent>,
    events_tx: Sender<SearchEvent>,
    pub show_stats: bool,
    sampler: StatsSampler,
    last_stats: Stats,
    pub show_captions: bool,
    pub caption_status: CaptionStatus,
    pub captions: Vec<Cue>,
    captions_track_id: Option<String>,
    pub caption_events_rx: Receiver<CaptionEvent>,
    caption_events_tx: Sender<CaptionEvent>,
}

impl App {
    pub fn new(player: Player) -> Self {
        let (tx, rx) = mpsc::channel();
        let (cap_tx, cap_rx) = mpsc::channel();
        let sampler = StatsSampler::new(player.pid());
        Self {
            mode: Mode::Browse,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            queue: Vec::new(),
            current: None,
            status: "Press `s` to search, `?` for help, `q` to quit.".into(),
            searching: false,
            should_quit: false,
            player,
            events_rx: rx,
            events_tx: tx,
            show_stats: true,
            sampler,
            last_stats: Stats::default(),
            show_captions: false,
            caption_status: CaptionStatus::Idle,
            captions: Vec::new(),
            captions_track_id: None,
            caption_events_rx: cap_rx,
            caption_events_tx: cap_tx,
        }
    }

    pub fn toggle_stats(&mut self) {
        self.show_stats = !self.show_stats;
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
            // mpv reported end-of-file; advance
            let next = self.current.map(|i| i + 1);
            if let Some(n) = next {
                if n < self.queue.len() {
                    self.play_at(n);
                } else {
                    self.current = None;
                    self.status = "Queue finished.".into();
                }
            }
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.results.is_empty() {
            return;
        }
        let len = self.results.len() as i32;
        let mut new = self.selected as i32 + delta;
        if new < 0 {
            new = 0;
        }
        if new >= len {
            new = len - 1;
        }
        self.selected = new as usize;
    }

    pub fn play_selected(&mut self) {
        if let Some(track) = self.results.get(self.selected).cloned() {
            // Build the queue from the current result list, starting at selected.
            self.queue = self.results.clone();
            self.play_at(self.selected);
            self.status = format!("Playing: {} — {}", track.title, track.uploader);
        }
    }

    pub fn play_at(&mut self, idx: usize) {
        if let Some(track) = self.queue.get(idx).cloned() {
            self.current = Some(idx);
            if let Err(e) = self.player.load(&track.url()) {
                self.status = format!("mpv load failed: {}", e);
            } else {
                self.status = format!("[{}/{}] {} — {}", idx + 1, self.queue.len(), track.title, track.uploader);
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
        if let Some(i) = self.current {
            if i + 1 < self.queue.len() {
                self.play_at(i + 1);
            } else {
                self.status = "End of queue.".into();
            }
        }
    }

    pub fn prev_track(&mut self) {
        if let Some(i) = self.current {
            if i > 0 {
                self.play_at(i - 1);
            } else {
                // restart current track
                self.play_at(0);
            }
        }
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
        self.current.and_then(|i| self.queue.get(i))
    }

    pub fn yank_selected_url(&mut self) {
        let Some(track) = self.results.get(self.selected) else {
            self.status = "Nothing to copy — search and select a track first.".into();
            return;
        };
        let url = track.url();
        match clipboard::copy(&url) {
            Ok(_tool) => self.status = format!("Copied URL: {}", url),
            Err(e) => self.status = format!("Copy failed: {}", e),
        }
    }
}
