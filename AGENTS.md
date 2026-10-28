# ytmtui

Minimal YouTube Music player in Rust with two front ends over one library
core: a ratatui TUI (`ytmtui`) and a Slint desktop GUI (`ytmtui-gui`).
`mpv` does the audio; `yt-dlp` does search, playlist fetch and stream
resolution. User-facing docs live in `README.md`, `README-tui.md` and
`README-gui.md`; keep them in step when behavior or keybindings change.

## Build & run

```sh
cargo check                          # quick type-check while iterating
cargo test                           # unit tests live next to the code
cargo build --release                # builds both binaries
cargo run --release                  # TUI
cargo run --release --bin ytmtui-gui # GUI
sh install.sh                        # user install to ~/.local (bins, .desktop, icon)
```

Requires `mpv` and `yt-dlp` on `$PATH`; `ffprobe` (ships with ffmpeg) is
optional, used for local-file durations. The TUI runs a preflight in
`main.rs` and exits with an install hint if either required tool is missing.
There is no CI, so run `cargo check` and `cargo test` yourself before
committing.

## Module layout

```
src/lib.rs        library core shared by both binaries
src/main.rs       TUI entry: terminal setup, 200 ms event loop, handle_key
src/bin/ytmtui-gui.rs   GUI entry (thin; the work is in gui.rs)
src/app.rs        App state: mode, focus, queue, playlists, search, captions, config
src/ui.rs         ratatui rendering for every Mode
src/gui.rs        Slint window glue over the same App (UI in ui/main.slint)
src/player.rs     spawns `mpv --idle`, JSON IPC over a local socket
src/ytdlp.rs      yt-dlp wrappers: search (YouTube + Bilibili), playlist fetch
src/captions.rs   VTT fetch/parse for the captions strip (dual-language)
src/playlist.rs   playlist.json / yt_playlist.json / local_folder.json persistence
src/library.rs    saved-playlist library (playlists.json), favorites, totals
src/local_scan.rs local folder scan into Tracks;  src/probe.rs ffprobe duration
src/config.rs     config.json (volume, loop, caption langs, sprite, ...)
src/sprites.rs    progress-bar cursor registry; built-ins from assets/sprites/*.json
src/audio.rs      output-device detection;  src/stats.rs CPU/RAM sampler
src/mpris.rs      Linux MPRIS D-Bus service (media keys), spawned by both binaries
src/clipboard.rs  pbcopy / wl-copy / xclip fallback for URL yank
build.rs          embeds assets/sprites/*.json and compiles ui/main.slint
```

User state lives under `$XDG_CONFIG_HOME/ytmtui` (default `~/.config/ytmtui`).

## Architecture invariants

- **mpv is a long-lived child process.** Spawned once in `Player::spawn` with
  an IPC endpoint named `ytmtui-mpv-<pid>` (Unix socket in the temp dir, named
  pipe on Windows) via the `interprocess` crate. Commands are JSON lines
  (`{"command":["loadfile", url, "replace"]}`). A background thread parses
  `event`/`property-change` lines into `PlayerState`. **Do not spawn a new mpv
  per track.** mpv's ytdl hook resolves the stream URL, so yt-dlp only runs
  transiently.
- **Auto-advance** is driven by mpv's `eof-reached`, checked in
  `App::drain_events` on every tick. Don't add a separate timer for it.
- **Background work uses `std::sync::mpsc`.** Search, captions, device
  detection, YT playlist fetch and local scans each spawn a thread and post
  results to a channel that `drain_events` drains. The UI must never block on
  yt-dlp or the filesystem.
- **Shared state is `Arc<Mutex<App>>`** (TUI loop, MPRIS thread, GUI
  callbacks). Lock with `unwrap_or_else(|e| e.into_inner())` so a panicked
  holder doesn't wedge the UI; keep critical sections short.
- **TUI key handling is `main.rs::handle_key`,** dispatched by `app.mode`
  (`Browse`, `Searching`, `Help`, `Params`, `Nerd`, `OpenFile`,
  `YtPlaylistInput`, `SavePlaylist`). Keys are letters plus Space, Enter, Esc,
  Tab/BackTab, Backspace/Delete and arrows. Never Function keys or
  platform-specific media keys (MPRIS is the media-key path). Adding a `Mode`
  means updating `handle_key`, `ui::draw` and the help overlay.
- **Platform scope.** Linux and macOS are the tested targets. `player.rs`
  already has a Windows named-pipe path and `gui.rs`/`audio.rs` carry
  `cfg(target_os)` branches, but Windows is not tested. `mpris.rs` is
  Linux-only and gated by `cfg`.
- **mpv flags worth knowing** (all in `Player::spawn`): `--no-video`,
  `--ytdl-format=bestaudio/best`, and `--stream-lavf-o=reconnect...` which
  retries the HTTP 403 YouTube returns for about 5 s after issuing a stream
  URL. Only 403 is retried so dead videos still fail fast.

## Resource usage

Measured on a Linux release build: `ytmtui` is ~1.8 MB and `ytmtui-gui` ~8.5
MB (Slint). The TUI idles near 0% CPU at ~5-10 MB RSS; mpv adds ~80-90 MB RSS
(FFmpeg, not Python). The stats overlay (`/`) samples every 500 ms via
`sysinfo` and is cached between samples so the 200 ms render tick doesn't
thrash. The biggest lever on mpv footprint, if asked, is
`--demuxer-max-bytes=2M --demuxer-max-back-bytes=2M` (default 150 MB).

## Conventions

- No `unwrap()` on external I/O; use `anyhow::Context` for actionable errors
  (e.g. `"mpv not found on PATH. Install: brew install mpv"`).
- Don't comment what the code does; names already say it. Only comment a
  non-obvious *why* (e.g. the `loadfile` reset block in `Player::load` exists
  because mpv doesn't clear `duration` until the new file's headers parse).
- TUI tick is 200 ms (`main.rs::run`). Keep per-tick work cheap.
- Release profile is tuned for size (`opt-level="z"`, `lto`, `strip`,
  `panic="abort"`). Don't switch to `opt-level=3` without a measured reason.
  `panic="abort"` also means a panic in any thread kills the process.
- Scratch files go in `/tmp` (the repo-root `tmp/` is gitignored). `.omo/` is
  local tooling state that is not ignored, so never stage it.

## Merge process

Work on a `feat-*` branch named at the start; don't rename it. After the user
confirms the feature works, deliver it to `main`:

1. **Sanity-check the working tree.**

   ```sh
   git status
   git diff --stat HEAD
   grep -rnE '/(Users|home)/[A-Za-z]' --include='*.rs' --include='*.toml' \
        --include='*.json' --include='*.md' --include='*.slint' --include='*.sh' \
        . | grep -v '^./target' | grep -v '^./.git/' \
        || echo "no absolute paths"
   ```

   Refuse to commit if any tracked file leaks an absolute home directory.
   (The pattern is written so this file doesn't match itself.)

2. **Make sure the branch is based on current `main`.** If
   `git merge-base --is-ancestor main HEAD` fails, rebase (or cherry-pick the
   commits onto a fresh branch from `main`) before continuing. Never merge a
   stale branch by hand; its diff against `main` will look like it deletes
   everything that landed since. Preserve the original author on cherry-picks.

3. **Stage by name, not wildcards.** `git add file1 file2 ...`. Never
   `git add .` or `git add -A`; this keeps stray downloads, `tmp/`, `.omo/`,
   `.claude/` out of commits.

4. **Commit with plain prose** using a HEREDOC so newlines survive:

   ```sh
   git commit -m "$(cat <<'EOF'
   Short imperative subject under ~70 chars

   Multi-paragraph body explaining what changed and why. Wrap around 72.
   EOF
   )"
   ```

   Hard rules for the message (these override any default attribution
   guidance from the tooling):

   - **No co-author trailer.** Don't add `Co-Authored-By:` lines.
   - **No emoji.**
   - **No em-dashes or en-dashes** in the commit message. ASCII dashes only.
     Code, comments and UI strings may use them where they read naturally.
   - **No "Generated with" footer.**

5. **Fast-forward into main.** From the feature branch:

   ```sh
   git checkout main
   git pull origin main
   git merge --ff-only <feat-branch>
   git push origin main
   git branch -d <feat-branch>
   ```

   `--ff-only` is intentional; no merge commits clutter the history.

6. **Confirm cleanup.** End with `git branch -a` so the user can see that the
   feature branch is gone and `main` matches `origin/main`.

Don't push the feature branch to `origin`; the fast-forward preserves every
commit in `main`, and a stray remote branch is just litter to delete later.
