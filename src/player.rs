use anyhow::{Context, Result};
use interprocess::local_socket::prelude::*;
#[cfg(unix)]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::Stream;
use interprocess::TryClone;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Wraps a long-running `mpv --idle` process controlled via JSON IPC over a
/// local socket. The same code path uses Unix sockets on macOS/Linux and
/// named pipes on Windows; the `interprocess` crate hides the difference.
pub struct Player {
    _child: Child,
    socket_path: PathBuf,
    writer: Mutex<Stream>,
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

/// Address the mpv IPC socket on disk (Unix) or in the named-pipe namespace
/// (Windows). Returned as a `PathBuf` so the existing `--input-ipc-server`
/// arg builder still works unchanged.
#[cfg(unix)]
fn ipc_socket_path(pid: u32) -> PathBuf {
    std::env::temp_dir().join(format!("ytmtui-mpv-{}.sock", pid))
}

#[cfg(windows)]
fn ipc_socket_path(pid: u32) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\ytmtui-mpv-{}", pid))
}

/// Try-connect-with-retry. The socket file (Unix) or named pipe (Windows)
/// is created asynchronously by mpv after spawn, so we poll instead of
/// stat'ing — Windows named pipes never show up in the filesystem.
fn connect_with_retry(socket_path: &PathBuf, deadline: Duration) -> Result<Stream> {
    let start = Instant::now();
    loop {
        let attempt = connect_once(socket_path);
        match attempt {
            Ok(s) => return Ok(s),
            Err(_) if start.elapsed() < deadline => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("mpv IPC socket never came up at {}", socket_path.display())
                });
            }
        }
    }
}

#[cfg(unix)]
fn connect_once(socket_path: &PathBuf) -> Result<Stream> {
    let name = socket_path
        .as_os_str()
        .to_fs_name::<GenericFilePath>()
        .context("invalid Unix socket path")?;
    Ok(Stream::connect(name)?)
}

#[cfg(windows)]
fn connect_once(socket_path: &PathBuf) -> Result<Stream> {
    // mpv listens on `\\.\pipe\<name>`; interprocess's namespaced lookup
    // wants the bare `<name>` and re-derives the prefix itself.
    let raw = socket_path.to_string_lossy();
    let name_str = raw.strip_prefix(r"\\.\pipe\").unwrap_or(&raw).to_string();
    let name = name_str
        .to_ns_name::<GenericNamespaced>()
        .context("invalid named-pipe name")?;
    Ok(Stream::connect(name)?)
}

impl Player {
    pub fn spawn() -> Result<Self> {
        let socket_path = ipc_socket_path(std::process::id());
        // On Unix the path is in /tmp and may be left over from a previous
        // crashed run; clear it. On Windows the path lives in the pipe
        // namespace and remove_file is a no-op / not meaningful.
        #[cfg(unix)]
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

        let writer = connect_with_retry(&socket_path, Duration::from_secs(5))?;
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
                                    s.audio_codec =
                                        data.and_then(|d| d.as_str()).map(|x| x.to_string());
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
        self.command(vec![json!("loadfile"), json!(url), json!("replace")])?;
        // mpv carries the previous `pause` state across loadfile, so an
        // explicit unpause makes Enter-on-a-track behave like "play it"
        // instead of "load it paused" when the user had paused earlier.
        self.command(vec![json!("set_property"), json!("pause"), json!(false)])
    }

    pub fn toggle_pause(&self) -> Result<()> {
        self.command(vec![json!("cycle"), json!("pause")])
    }

    pub fn set_pause(&self, paused: bool) -> Result<()> {
        self.command(vec![json!("set_property"), json!("pause"), json!(paused)])
    }

    pub fn seek_relative(&self, seconds: f64) -> Result<()> {
        self.command(vec![json!("seek"), json!(seconds), json!("relative")])
    }

    pub fn set_volume(&self, v: u8) -> Result<()> {
        self.command(vec![
            json!("set_property"),
            json!("volume"),
            json!(v as i64),
        ])
    }

    pub fn state(&self) -> PlayerState {
        self.state.lock().unwrap().clone()
    }

    pub fn pid(&self) -> u32 {
        self._child.id()
    }

    pub fn shutdown(&mut self) {
        let _ = self.send(&json!({ "command": ["quit"] }));
        thread::sleep(Duration::from_millis(50));
        let _ = self._child.kill();
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.socket_path);
        #[cfg(windows)]
        let _ = &self.socket_path;
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shutdown();
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
