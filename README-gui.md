# GUI -- desktop interface

## ytmtui - Desktop GUI

The GUI version shares the same Rust backend as the terminal version but
presents a desktop-native interface built with [Slint].

## Layout

The window is arranged as a vertical stack:

**Header** -- a search bar flanked by toolbar buttons: Open (file/folder),
Playlist, Settings, and Help.

**Search chip** -- typing `y` then Tab scopes the search to YouTube, `b` then
Tab to Bilibili, `h` then Tab to Local files. Enter submits the query as a
fallback without a platform chip. The active chip appears as a colored pill
in the search bar and scopes all subsequent searches to that platform. Press
`x` or click the chip's close icon to dismiss it and return to the default
scope.

**Tab bar** -- three tabs: Results, Library, Local. Clicking a tab switches
the content area below.

**Content area** -- the focused tab's list. Results shows search output;
Library shows saved playlists and favorites; Local shows files and folders
from the scan path.

**Now Playing card** -- a two-row strip at the bottom of the content area:
- Top row: track title, artist, and duration.
- Bottom row: transport controls.

**Transport** -- previous, play/pause, and next buttons. To the right, a
clickable progress bar for seeking and a continuous volume slider. Loop,
shuffle, and closed-captions toggles sit at the far end.

**Captions strip** -- a toggleable area below the Now Playing card that
displays the current track's subtitles as they play.

**Status bar** -- a thin strip at the window bottom showing the current state
(e.g. "Playing", "Paused", "Searching...") and any contextual messages.

## Mouse interactions

- **Single-click** a row to select it.
- **Double-click** a track to play it immediately; double-click a playlist
  to open it.
- **Row action buttons** -- each row in the list has inline buttons: Play,
  Add, Remove, URL (copy), Open, Favorite, Delete. These appear on hover or
  selection depending on the list.
- **Progress bar** -- click anywhere on the bar to seek to that position.
- **Volume slider** -- drag to set volume continuously (no discrete steps).

## Dialogs

- **Open File/Folder** -- triggered by the toolbar Open button. Uses the
  native file picker to select a local audio file or scan a folder.
- **Load Playlist** -- triggered by the toolbar Playlist button. Prompts for
  a YouTube playlist URL and imports its tracks.
- **Save Playlist** -- triggered by the Library tab's Save as button. Saves
  the current playlist to `~/.config/ytmtui/playlist.json`.
- **Settings** -- triggered by the toolbar gear icon. Contains:
  - Progress cursor selection (same sprite system as the TUI)
  - Closed-captions language preference
  - Nerd stats toggle
- **Help** -- triggered by the toolbar `?` button. Shows the available
  keyboard shortcuts and mouse controls in a modal dialog.
- **Nerd stats** -- accessible from Settings. Shows CPU/RAM usage, audio
  codec info, output device, and version numbers in a dedicated dialog.

## Platform search filtering

The search chip in the header scopes queries to a specific platform. The
available platforms are:

| Shortcut | Platform | Description |
| -------- | -------- | ----------- |
| `y` + Tab | YouTube | Searches YouTube via yt-dlp |
| `b` + Tab | Bilibili | Searches Bilibili |
| `h` + Tab | Local | Searches locally indexed files |

The chip renders as a colored pill inside the search bar -- the color
indicates the active platform. Dismiss it with `x` or by clicking its close
icon.

## Window title

The window title updates dynamically to show the currently playing track,
following the pattern "ytmtui -- <artist> - <title>".
