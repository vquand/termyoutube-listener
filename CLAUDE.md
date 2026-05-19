# ytmtui

Minimal terminal-based YouTube Music player. Rust drives the TUI and glue; `mpv`
does the audio; `yt-dlp` does search + stream resolution.

## Build & run

```sh
cargo build --release           # release: ~590K binary, opt-level="z" + LTO + strip
cargo run --release             # or ./target/release/ytmtui
cargo check                     # quick type-check while iterating
```

Requires `mpv` and `yt-dlp` on `$PATH`. `main.rs` runs a preflight check and
exits with an install hint if either is missing.

## Module layout

```
src/main.rs    entry point, terminal setup, event loop, keybindings dispatch
src/app.rs     application state (mode, queue, current, search results, stats cache)
src/ui.rs      ratatui rendering — search bar, results list, now-playing gauge, stats overlay, help
src/player.rs  spawns `mpv --idle --no-video`, controls via JSON IPC over a Unix socket
src/ytdlp.rs   `yt-dlp ytsearchN:<q>` subprocess wrapper returning Vec<Track>
src/stats.rs   sysinfo-based CPU/RAM sampler for the ytmtui + mpv PIDs
```

## Architecture invariants

- **mpv is a long-lived child process.** Spawned once at startup in `Player::spawn`
  with a Unix socket at `/tmp/ytmtui-mpv-<pid>.sock`. Commands are JSON lines
  (`{"command":["loadfile", url, "replace"]}`). A background thread parses
  `event`/`property-change` lines and updates `PlayerState` (position, duration,
  paused, eof). **Do not spawn a new mpv per track.**
- **Auto-advance** is driven by mpv's `eof-reached` property, polled in
  `App::drain_events` on every tick. Don't add a separate timer for it.
- **Search is async via `std::sync::mpsc`.** `App::submit_search` spawns a
  thread; results land via `SearchEvent::Done` and are picked up in
  `drain_events`. UI never blocks on yt-dlp.
- **All key handling is in `main.rs::handle_key`,** dispatched by `app.mode`
  (`Browse` | `Searching` | `Help`). Keybindings are letter-keys only (plus
  Space/Enter/Esc) — never Function keys, never platform-specific media keys.
  See README for the table.
- **Unix-only.** Uses `std::os::unix::net::UnixStream` for mpv IPC. Windows
  would need a named-pipe path in `player.rs`.

## Resource usage

- ytmtui Rust process: ~5-10 MB RSS, ~0% CPU idle.
- mpv child: ~80-90 MB RSS idle on macOS (FFmpeg + AVFoundation + libav codec
  tables — not Python; yt-dlp only runs transiently during search/load).
- GPU: not used (`--no-video`). Stats overlay reports `GPU: idle (audio-only)`
  because per-process GPU on macOS requires `sudo powermetrics` anyway.
- Stats overlay (`t` to toggle) samples every 500 ms via `sysinfo`; cached
  between samples so the 200 ms render tick doesn't thrash.

If asked to reduce mpv footprint, the biggest single lever is
`--demuxer-max-bytes=2M --demuxer-max-back-bytes=2M` (defaults to 150 MB).
See `Player::spawn` in `src/player.rs`.

## Conventions

- No `unwrap()` on external I/O — use `anyhow::Context` for actionable errors
  (e.g. `"mpv not found on PATH. Install: brew install mpv"`).
- Don't comment what the code does — names already say it. Only comment a
  non-obvious *why* (e.g. the `loadfile` reset block in `Player::load` exists
  because mpv doesn't clear `duration` until the new file's headers parse).
- TUI tick is 200 ms (`main.rs::run`). Keep per-tick work cheap.
- Release profile is tuned for size (`opt-level="z"`, `lto=true`, `strip=true`,
  `panic="abort"`). Don't switch to `opt-level=3` without a measured reason.
