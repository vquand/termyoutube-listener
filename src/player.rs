use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Wraps a long-running `mpv --idle` process controlled via JSON IPC over a unix socket.
pub struct Player {
    _child: Child,
    socket_path: PathBuf,
    writer: Mutex<UnixStream>,
    state: Arc<Mutex<PlayerState>>,
}

#[derive(Debug, Default, Clone)]
pub struct PlayerState {
    pub paused: bool,
    pub position: f64,
    pub duration: f64,
    pub idle: bool,
    pub eof_reached: bool,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<f64>,
    pub samplerate: Option<u32>,
    pub channels: Option<u32>,
}

impl Player {
    pub fn spawn() -> Result<Self> {
        let socket_path = std::env::temp_dir().join(format!("ytmtui-mpv-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket_path);

        let child = Command::new("mpv")
            .args([
                "--idle=yes",
                "--no-video",
                "--no-terminal",
                "--no-input-default-bindings",
                "--really-quiet",
                "--audio-display=no",
                "--ytdl-format=bestaudio/best",
                &format!("--input-ipc-server={}", socket_path.display()),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn mpv (is it installed and on PATH?)")?;

        // wait for socket to appear (mpv creates it asynchronously)
        let start = Instant::now();
        while !socket_path.exists() {
            if start.elapsed() > Duration::from_secs(5) {
                anyhow::bail!("mpv IPC socket never appeared at {}", socket_path.display());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let writer = UnixStream::connect(&socket_path).context("failed to connect to mpv IPC")?;
        let reader_stream = writer.try_clone().context("clone mpv socket")?;

        let state = Arc::new(Mutex::new(PlayerState::default()));
        let state_for_reader = state.clone();
        thread::spawn(move || {
            let reader = BufReader::new(reader_stream);
            for line in reader.lines().map_while(|l| l.ok()) {
                let Ok(val) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if let Some(event) = val.get("event").and_then(|e| e.as_str()) {
                    let mut s = state_for_reader.lock().unwrap();
                    match event {
                        "property-change" => {
                            let name = val.get("name").and_then(|n| n.as_str()).unwrap_or("");
                            let data = val.get("data");
                            match name {
                                "time-pos" => {
                                    if let Some(d) = data.and_then(|d| d.as_f64()) {
                                        s.position = d;
                                    }
                                }
                                "duration" => {
                                    if let Some(d) = data.and_then(|d| d.as_f64()) {
                                        s.duration = d;
                                    }
                                }
                                "pause" => {
                                    if let Some(b) = data.and_then(|d| d.as_bool()) {
                                        s.paused = b;
                                    }
                                }
                                "idle-active" => {
                                    if let Some(b) = data.and_then(|d| d.as_bool()) {
                                        s.idle = b;
                                    }
                                }
                                "eof-reached" => {
                                    if let Some(b) = data.and_then(|d| d.as_bool()) {
                                        s.eof_reached = b;
                                    }
                                }
                                "audio-codec-name" => {
                                    s.audio_codec = data
                                        .and_then(|d| d.as_str())
                                        .map(|x| x.to_string());
                                }
                                "audio-bitrate" => {
                                    s.audio_bitrate = data.and_then(|d| d.as_f64());
                                }
                                "audio-params" => {
                                    if let Some(obj) = data.and_then(|d| d.as_object()) {
                                        s.samplerate = obj
                                            .get("samplerate")
                                            .and_then(|v| v.as_u64())
                                            .map(|x| x as u32);
                                        s.channels = obj
                                            .get("channels")
                                            .and_then(|v| v.as_u64())
                                            .map(|x| x as u32);
                                    }
                                }
                                _ => {}
                            }
                        }
                        "end-file" => {
                            s.eof_reached = true;
                            s.position = 0.0;
                            s.duration = 0.0;
                        }
                        "start-file" => {
                            s.eof_reached = false;
                            s.idle = false;
                        }
                        _ => {}
                    }
                }
            }
        });

        let player = Player {
            _child: child,
            socket_path,
            writer: Mutex::new(writer),
            state,
        };

        // subscribe to property events
        player.observe("time-pos")?;
        player.observe("duration")?;
        player.observe("pause")?;
        player.observe("idle-active")?;
        player.observe("eof-reached")?;
        player.observe("audio-codec-name")?;
        player.observe("audio-bitrate")?;
        player.observe("audio-params")?;
        Ok(player)
    }

    fn send(&self, payload: &Value) -> Result<()> {
        let mut w = self.writer.lock().unwrap();
        let mut bytes = serde_json::to_vec(payload)?;
        bytes.push(b'\n');
        w.write_all(&bytes)?;
        w.flush()?;
        Ok(())
    }

    fn command(&self, args: Vec<Value>) -> Result<()> {
        self.send(&json!({ "command": args }))
    }

    fn observe(&self, name: &str) -> Result<()> {
        self.command(vec![json!("observe_property"), json!(1), json!(name)])
    }

    pub fn load(&self, url: &str) -> Result<()> {
        // reset perceived duration immediately
        {
            let mut s = self.state.lock().unwrap();
            s.duration = 0.0;
            s.position = 0.0;
            s.eof_reached = false;
            s.idle = false;
            s.paused = false;
        }
        self.command(vec![json!("loadfile"), json!(url), json!("replace")])
    }

    pub fn toggle_pause(&self) -> Result<()> {
        self.command(vec![json!("cycle"), json!("pause")])
    }

    pub fn seek_relative(&self, seconds: f64) -> Result<()> {
        self.command(vec![json!("seek"), json!(seconds), json!("relative")])
    }

    pub fn set_volume(&self, v: u8) -> Result<()> {
        self.command(vec![json!("set_property"), json!("volume"), json!(v as i64)])
    }

    pub fn state(&self) -> PlayerState {
        self.state.lock().unwrap().clone()
    }

    pub fn pid(&self) -> u32 {
        self._child.id()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.send(&json!({ "command": ["quit"] }));
        thread::sleep(Duration::from_millis(50));
        let _ = self._child.kill();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

pub fn check_installed() -> Result<()> {
    Command::new("mpv")
        .arg("--version")
        .output()
        .context("mpv not found on PATH. Install it: `brew install mpv` or see https://mpv.io")?;
    Ok(())
}

/// Best-effort one-shot version probe. Returns the first line of `mpv --version`.
pub fn version() -> Option<String> {
    let out = Command::new("mpv").arg("--version").output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    s.lines().next().map(|l| l.trim().to_string())
}
