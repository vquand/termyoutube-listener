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
| `Tab`     | Switch focus between Results / Playlist |
| `+`       | Add selected result to playlist     |
| `-` / `⌫` / `Del` | Remove selected playlist entry |
| `L` / `l` | Cycle loop mode (off → all → one)   |
| `H` / `h` | Toggle shuffle                      |
| `/`       | Toggle nerd-stats modal             |
| `z` / `x` | Volume down / up (10% steps)        |
| `c`       | Toggle closed captions strip        |
| `y`       | Yank (copy) selected track URL      |
| `o`       | Open a local file or scan a folder (search bar input) |
| `p`       | Load a YouTube playlist URL (search bar input) |
| `` ` ``   | Open parameters menu                |
| `.`       | Hide / show shortcut bar            |
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
├── main.rs       # entry point, terminal setup, event loop, keybindings
├── app.rs        # application state (queue, mode, search, current track)
├── ui.rs         # ratatui rendering (search bar, list, now-playing, help)
├── player.rs     # spawns mpv, JSON IPC over a Unix socket
├── ytdlp.rs      # yt-dlp subprocess wrapper for search
├── captions.rs   # fetches & parses VTT subtitles for the closed-captions strip
├── clipboard.rs  # pbcopy / wl-copy / xclip fallback chain for URL yank
├── config.rs     # JSON config persistence (~/.config/ytmtui/config.json)
├── sprites.rs    # progress-bar cursor registry (built-ins + user dir)
└── stats.rs      # CPU/RAM sampler for the stats overlay

assets/sprites/   # JSON definitions for built-in progress cursors,
                  # embedded into the binary at compile time by build.rs.
```

## Audio output indicator

The top-right corner of the Now Playing block shows a small kaomoji that
hints at the current output device, plus a single block glyph that scales
with volume.

- `<(˃ᴗ˂)>` speaker (built-in, displayport, hdmi, or any device whose name
  contains "speaker")
- `Ω(˃ᴗ˂)Ω` headphones (well-known over/on-ear lines like WH-, QC, Bose,
  Studio, Beats, or any name containing "headphone")
- `ɞ(˃ᴗ˂)ʚ` earbuds (AirPods, Buds, or any Bluetooth device with a custom
  name)
- `?(˃ᴗ˂)?` unknown / detection unavailable

A small `ᛒ` (the Bluetooth rune) appears next to the kaomoji when the
device is connected over Bluetooth. The block glyph next to it is one of `▁ ▂ ▃ ▄ ▅ ▆ ▇ █` chosen by
volume level (single glyph, swaps as volume changes).

On macOS, the device is detected via `system_profiler SPAudioDataType -json`
(re-polled every 5 seconds in a background thread). On Linux and other Unix
the indicator falls back to the unknown kaomoji.

## Volume

`z` and `x` step the volume down / up by 10% (clamped to 0..=100). A small
popup near the bottom of the screen shows for ~2 seconds after each press: a
fixed-position 20-cell bar, the percentage, and a 6-stage kaomoji whose mouth
opens wider on louder stages with music notes trailing it.

Stages: mute, 1-20%, 21-40%, 41-60%, 61-80%, 81-100%. Volume is persisted in
`config.json` and applied to mpv at startup.

## Nerd stats

Hidden by default. Press `/` for a centered modal showing:

- ytmtui version (from Cargo)
- mpv + yt-dlp versions (queried at startup)
- CPU / RAM for the Rust UI and mpv child (sampled every 500 ms via [`sysinfo`])
- Current track's audio codec, bitrate, sample rate, channels (mpv property
  observations: `audio-codec-name`, `audio-bitrate`, `audio-params`)
- Output device name and a one-line model summary (kind + transport).
  Falls back to a generic "(default system output)" / "(generic audio)" if
  detection isn't available on this OS.

Press `/`, `Esc`, or `q` again to close. GPU is intentionally absent —
playback runs with `--no-video` and never touches the GPU.

## Contextual shortcut hints

When the shortcut bar is visible (`.` toggles it), shortcut hints appear
*inside* the panel titles where they're relevant:

- **Search bar title**: `s search` (or `↵ submit · esc cancel` in search mode)
- **Results / Playlist tab title**: `⇥ switch · + add · - remove · ↵ play · y URL`
- **Now Playing title**: `L loop:off · H shuffle:off · ␣ pause · n/b skip · f/r ±10s · c CC`

The global bar below the list pane is then trimmed to just the global modals
and meta keys: `? help · p params · / nerd · q quit · . hide`. When the bar
is hidden, contextual hints in titles also go away — only `. show shortcuts`
remains as a dim line.

Shortcut keys render in bright cyan; active state indicators (loop on, current
tab, etc.) render in yellow.

## Playlist

The list pane is tabbed: `Results` (your current search) and `Playlist` (a
persistent collection). `Tab` toggles which is focused; `j`/`k`/`Enter`/`+`/`-`
all operate on the focused list.

- `+` adds the highlighted search result to the playlist (dedupes by video id).
- `-` (or Backspace / Delete) removes the highlighted playlist entry.
- `Enter` builds the playback queue from the focused list and starts the
  selected track. The `▶` marker stays in whichever list the queue came from
  even if you switch focus.

The playlist is saved to `~/.config/ytmtui/playlist.json` (or `$XDG_CONFIG_HOME`
equivalent) on every change.

### Loop & shuffle

- `L` (or `l`) cycles loop mode: `off` → `all` → `one`. `all` wraps from the
  end of the queue back to the start; `one` repeats the current track.
- `H` (or `h`) toggles shuffle. When on, both auto-advance and manual
  `n`/`b` pick a random different track from the queue.

Both modes apply to whatever queue is currently playing (whether it came from
the search results or the playlist) and persist across restarts in
`config.json`.

### Hide-able shortcut bar

`.` hides the shortcut row above Now Playing. When hidden, a single dim hint
(`. show shortcuts`) stays in its place so the toggle's never lost. Persisted.

## Progress-bar cursors (add-on system)

The progress bar carries an animated "cursor" sprite — by default a nyan cat,
but you can cycle through several built-in mascots (or add your own) via the
parameters menu (`p`).

Each sprite is a small JSON file. Built-ins live in `assets/sprites/` in this
repo and are embedded into the binary at compile time. To add a custom cursor
without touching the source, drop a JSON file in:

```
$XDG_CONFIG_HOME/ytmtui/sprites/      # if XDG_CONFIG_HOME is set
~/.config/ytmtui/sprites/              # otherwise
```

The filename (minus `.json`) becomes the sprite's `id`. User files override
built-ins with the same id (so `~/.config/ytmtui/sprites/nyan.json` replaces
the bundled nyan). Restart `ytmtui` after adding or editing a file.

### Schema

```json
{
  "name": "my cursor",
  "frames": ["(•_•)", "(•ω•)", "(•‿•)"],
  "trail_left": "=~",
  "trail_right": " ",
  "accent": "magenta",
  "order": 100
}
```

| Field         | Type             | Notes                                                                                                |
| ------------- | ---------------- | ---------------------------------------------------------------------------------------------------- |
| `name`        | string, optional | Display name in the params menu. Defaults to the file's `id`.                                        |
| `frames`      | array of strings | One or more animation frames. The renderer cycles through them every 250 ms.                         |
| `trail_left`  | string           | Pattern repeated behind the cursor — its *last* character sits flush against the sprite.             |
| `trail_right` | string           | Pattern repeated ahead of the cursor — its *first* character sits flush against the sprite.          |
| `accent`      | string           | ANSI color name (see below).                                                                         |
| `order`       | integer, opt.    | Sort order in the params menu cycle. Defaults to `1000`, so custom sprites land after the built-ins. |
| `animate_on`  | string, opt.     | `"tick"` (default) cycles frames every 250 ms; `"move"` advances a frame only when the cursor steps to a new cell on the bar. Use `"move"` for many-frame sprites whose timed animation would feel too fast on a long track. |

### Trail patterns

Trails are *patterns*, not single characters. As the cursor moves, the pattern
appears to scroll outward — `trail_left` is right-anchored against the sprite,
`trail_right` is left-anchored. For a constant-color block bar, use a single
character (the built-in `none` cursor does this with `█` / `░`).

Example — bina (Columbina): `trail_left: "✦•┈๑⋅⋯"`, `trail_right: "⋯⋅๑┈•✦"`
renders as `...✦•┈๑⋅⋯<sprite>⋯⋅๑┈•✦...` and shifts as the track plays.

### Accent colors

`black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`,
`darkgray` (aliases: `gray`, `grey`), plus `lightred`, `lightgreen`,
`lightyellow`, `lightblue`, `lightmagenta`, `lightcyan`. Anything else falls
back to the terminal's default foreground.

### Tips

- Use a monospace font that handles your chosen unicode well — the renderer
  treats each `char` as one cell. CJK / box-drawing / emoji that render at
  double width will shift the cursor and trail by one cell.
- A frame may be the empty string `""`; that draws no sprite and the trail
  meets in the middle (used by the `none` cursor).
- All frames in a sprite don't have to be the same width. The cursor will
  "breathe" slightly as the bar grows or shrinks each frame.

### Example — minimal custom cursor

```json
{
  "name": "rocket",
  "frames": ["o>", "o»"],
  "trail_left": "-",
  "trail_right": ".",
  "accent": "lightcyan",
  "order": 50
}
```

Save as `~/.config/ytmtui/sprites/rocket.json`, restart, press `p`, cycle to
"rocket".

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
