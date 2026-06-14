# ytmtui

A tiny YouTube Music player written in Rust. Streams audio from YouTube via
[`mpv`] and [`yt-dlp`] -- the Rust binary itself is well under 1 MB and idles
at near-zero CPU.

Two interfaces share the same backend: a terminal TUI (default) and a native
desktop GUI built with Slint.

## Why this design

- **Rust does the UI + glue.** [`ratatui`] + [`crossterm`] for a portable TUI;
  [`Slint`] for a native desktop GUI. Both talk to the same library core.
- **`mpv` does the audio.** Spawned once with `--idle --no-video` and driven
  through its JSON IPC socket. mpv handles HTTP streaming, format decoding,
  buffering and seeking natively -- far lighter than reimplementing any of
  that in Rust.
- **`yt-dlp` does search + URL resolution.** Both for `ytsearchN:<query>` and
  to feed mpv the actual audio stream URL.

The result: a ~575 KB binary that drives two well-maintained C/Python tools
to do the heavy lifting.

## Prerequisites

You need `mpv` and `yt-dlp` on your `$PATH`. For the copy-URL feature on Linux
you also need a clipboard tool (`xclip`, `wl-clipboard`, or `xsel`).

```sh
# macOS
brew install mpv yt-dlp

# Debian/Ubuntu
sudo apt install mpv yt-dlp xclip
```

## Build & run

```sh
cargo build --release
./target/release/ytmtui
```

### Linux desktop UI

The project also ships a native Slint GUI binary and a Linux desktop launcher
for desktop environments such as GNOME.

```sh
cargo run --release --bin ytmtui-gui
```

To install the GUI into your current user's app launcher:

```sh
scripts/install-linux-desktop.sh
```

This installs `ytmtui-gui` into `~/.local/bin`, registers
`ytmtui-gui.desktop` under `~/.local/share/applications`, and installs the app
icon under the user icon theme. No root access is required.

## How playback works

1. You search for something (type a query, hit Enter).
2. A background thread runs `yt-dlp --flat-playlist --dump-json ytsearch5:<q>`.
   YouTube and Bilibili are searched in parallel, each returning up to 5
   results (max 10 combined).
   Results stream back into the UI without blocking input.
3. Selecting a result queues the visible result list and tells the existing
   `mpv` instance to `loadfile https://youtube.com/watch?v=...`. mpv invokes
   `yt-dlp` internally to pick the best audio stream.
4. mpv pushes property events (`time-pos`, `duration`, `pause`, `eof-reached`)
   back over the IPC socket; we render them.
5. On `eof-reached`, we auto-advance to the next queued track.

## Module layout

```
src/
├── main.rs       # entry point, terminal setup, event loop, keybindings (TUI)
├── lib.rs        # shared library core (player, ytdlp, config, stats, ...)
├── app.rs        # application state (queue, mode, search, current track)
├── ui.rs         # ratatui rendering (TUI)
├── gui.rs        # Slint desktop UI entry point (GUI binary)
├── player.rs     # spawns mpv, JSON IPC over a Unix socket
├── ytdlp.rs      # yt-dlp subprocess wrapper for search
├── captions.rs   # fetches & parses VTT subtitles for the closed-captions strip
├── clipboard.rs  # pbcopy / wl-copy / xclip fallback chain for URL yank
├── config.rs     # JSON config persistence (~/.config/ytmtui/config.json)
├── sprites.rs    # progress-bar cursor registry (built-ins + user dir)
├── stats.rs      # CPU/RAM sampler for the stats overlay
├── audio.rs      # audio output device detection
├── library.rs   # local playlist / library persistence
├── local_scan.rs # local file and folder scanning
├── mpris.rs      # MPRIS D-Bus interface for Linux media keys
├── playlist.rs   # playlist data structures
└── probe.rs      # format probing helpers

assets/sprites/   # JSON definitions for built-in progress cursors,
                  # embedded into the binary at compile time by build.rs.
```

## Notes & limitations

- macOS / Linux only (uses Unix domain sockets for mpv IPC). Windows would
  need a named-pipe path.
- The TUI uses Unix domain sockets; the GUI uses the same IPC mechanism.
- Local file playback and Bilibili search are supported in the GUI; the TUI
  supports local files via the `o` key.

## Further reading

- [TUI terminal interface](README-tui.md) - keybindings, progress bar cursors, sprites
- [GUI desktop interface](README-gui.md) - mouse controls, search chips, desktop layout

[`mpv`]: https://mpv.io
[`yt-dlp`]: https://github.com/yt-dlp/yt-dlp
[`ratatui`]: https://ratatui.rs
[`crossterm`]: https://github.com/crossterm-rs/crossterm
[`Slint`]: https://slint.dev
[`sysinfo`]: https://github.com/GuillaumeGomez/sysinfo
