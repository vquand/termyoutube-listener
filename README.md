# ytmtui

A tiny terminal-based YouTube Music player written in Rust. Streams audio from
YouTube via [`mpv`] and [`yt-dlp`] — the Rust binary itself is well under 1 MB
and idles at near-zero CPU.

## Why this design

- **Rust does the UI + glue.** [`ratatui`] + [`crossterm`] for a portable TUI.
- **`mpv` does the audio.** Spawned once with `--idle --no-video` and driven
  through its JSON IPC socket. mpv handles HTTP streaming, format decoding,
  buffering and seeking natively — far lighter than reimplementing any of
  that in Rust.
- **`yt-dlp` does search + URL resolution.** Both for `ytsearchN:<query>` and
  to feed mpv the actual audio stream URL.

The result: a ~575 KB binary that drives two well-maintained C/Python tools
to do the heavy lifting.

## Prerequisites

You need `mpv` and `yt-dlp` on your `$PATH`.

```sh
brew install mpv yt-dlp
```

(Or your distro's equivalent; both are widely packaged.)

## Build & run

```sh
cargo build --release
./target/release/ytmtui
```

## Keybindings

All controls are single letters (plus Space/Enter) — no Function keys, no
arrow-key dependencies, no OS-specific media keys.

| Key       | Action                              |
| --------- | ----------------------------------- |
| `s`       | Search YouTube                      |
| `Enter`   | Play selected result (queues list)  |
| `Space`   | Pause / resume                      |
| `n`       | Next track                          |
| `b`       | Back (previous track)               |
| `f` / `F` | Forward 10 seconds / 1 minute       |
| `r` / `R` | Rewind 10 seconds / 1 minute        |
| `j` / `k` | Move selection down / up            |
| `t`       | Toggle CPU/RAM usage corner         |
| `?`       | Toggle help overlay                 |
| `q`       | Quit (also `Ctrl-C`)                |

In **search mode**: type your query, `Enter` to submit, `Esc` to cancel.

## How playback works

1. You press `s`, type a query, hit `Enter`.
2. A background thread runs `yt-dlp --flat-playlist --dump-json ytsearch20:<q>`.
   Results stream back into the TUI without blocking input.
3. `Enter` on a result queues the visible result list and tells the existing
   `mpv` instance to `loadfile https://youtube.com/watch?v=…`. mpv invokes
   `yt-dlp` internally to pick the best audio stream.
4. mpv pushes property events (`time-pos`, `duration`, `pause`, `eof-reached`)
   back over the IPC socket; we render them.
5. On `eof-reached`, we auto-advance to the next queued track.

## Layout

```
src/
├── main.rs    # entry point, terminal setup, event loop, keybindings
├── app.rs     # application state (queue, mode, search, current track)
├── ui.rs      # ratatui rendering (search bar, list, now-playing, help)
├── player.rs  # spawns mpv, JSON IPC over a Unix socket
└── ytdlp.rs   # yt-dlp subprocess wrapper for search
```

## Resource usage overlay

Top-right corner shows live CPU and RAM for both the Rust UI process and the
mpv child, sampled every 500 ms via [`sysinfo`]. Press `t` to hide/show.

GPU is reported as `idle (audio-only)` because mpv runs with `--no-video`
and never touches the GPU. (On macOS, per-process GPU usage isn't exposed
without `sudo powermetrics`, so there'd be nothing meaningful to display
even if we wanted to.)

## Notes & limitations

- macOS / Linux only (uses Unix domain sockets for mpv IPC). Windows would
  need a named-pipe path.
- No persistent playlist — the queue is the most recent search results.
- No volume control binding yet (mpv defaults are used). Add `+`/`-` later
  if needed.

[`mpv`]: https://mpv.io
[`yt-dlp`]: https://github.com/yt-dlp/yt-dlp
[`ratatui`]: https://ratatui.rs
[`crossterm`]: https://github.com/crossterm-rs/crossterm
[`sysinfo`]: https://github.com/GuillaumeGomez/sysinfo
