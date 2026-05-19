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
}

impl App {
    pub fn new(player: Player) -> Self {
        let (tx, rx) = mpsc::channel();
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
        }
    }

    pub fn toggle_stats(&mut self) {
        self.show_stats = !self.show_stats;
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
}
