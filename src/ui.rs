use crate::app::{App, CaptionStatus, ListFocus, Mode, QueueSource};
use crate::audio::{self, DeviceKind};
use crate::config::LoopMode;
use crate::sprites::{AnimateOn, Sprite};
use crate::stats::fmt_bytes;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let mut constraints = vec![
        Constraint::Length(3), // search bar
        Constraint::Min(3),    // results
        Constraint::Length(1), // shortcut hints
        Constraint::Length(4), // now playing
    ];
    if app.show_captions {
        constraints.push(Constraint::Length(4)); // captions strip
    }
    constraints.push(Constraint::Length(1)); // status
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_search(f, app, chunks[0]);
    draw_results(f, app, chunks[1]);
    draw_shortcuts(f, app, chunks[2]);
    draw_now_playing(f, app, chunks[3]);
    let status_idx = if app.show_captions {
        draw_captions(f, app, chunks[4]);
        5
    } else {
        4
    };
    draw_status(f, app, chunks[status_idx]);

    if app.mode == Mode::Help {
        draw_help_overlay(f, area);
    }
    if app.mode == Mode::Params {
        draw_params_overlay(f, app, area);
    }
    if app.mode == Mode::Nerd {
        draw_nerd_overlay(f, app, area);
    }
    if app.volume_popup_active() {
        draw_volume_overlay(f, app, area);
    }
}

fn draw_volume_overlay(f: &mut Frame, app: &App, area: Rect) {
    let w: u16 = 44.min(area.width.saturating_sub(4));
    let h: u16 = 3;
    if area.width < w || area.height < h {
        return;
    }
    let x = (area.width.saturating_sub(w)) / 2;
    let y = area.height.saturating_sub(h + 4); // sit a few rows above the bottom
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let v = app.config.volume;
    let bar_cells: usize = 20;
    let filled = (v as usize * bar_cells + 50) / 100; // round to nearest
    let bar_filled: String = "█".repeat(filled);
    let bar_empty: String = "░".repeat(bar_cells - filled);
    let pct = format!(" {:>3}% ", v);
    let face = volume_face(v);

    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            bar_filled,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(bar_empty, Style::default().fg(Color::DarkGray)),
        Span::styled(
            pct,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            face,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" volume [z/x] ")
        .style(Style::default().bg(Color::Black));
    let p = Paragraph::new(line).block(block);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

/// 6-stage kaomoji whose mouth opens wider as the volume rises, with sound
/// waves emanating from it on louder stages.
fn volume_face(v: u8) -> &'static str {
    match v {
        0 => "(˘_˘) zZz",
        1..=20 => "(•‿•) ♪",
        21..=40 => "(•ω•) ♪♬",
        41..=60 => "(•o•)) ♬♪",
        61..=80 => "(•O•))) ♪♬♪",
        _ => "(•◯•)))) ♬♪♫♬",
    }
}

fn key_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn label_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn shortcut_pair(k: &str, l: &str) -> [Span<'static>; 2] {
    [
        Span::styled(k.to_string(), key_style()),
        Span::styled(format!(" {}", l), label_style()),
    ]
}

fn shortcut_sep() -> Span<'static> {
    Span::styled("  ·  ", label_style())
}

fn draw_shortcuts(f: &mut Frame, app: &App, area: Rect) {
    if !app.config.show_shortcuts {
        let line = Line::from(vec![
            Span::styled(
                " .",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" show shortcuts", label_style()),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }
    // Slim global bar — modals + meta. Pane-specific shortcuts live in the
    // adjacent panel titles.
    let entries = [
        ("?", "help"),
        ("`", "params"),
        ("/", "nerd"),
        ("q", "quit"),
        (".", "hide"),
    ];
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (k, l)) in entries.iter().enumerate() {
        if i > 0 {
            spans.push(shortcut_sep());
        }
        spans.extend(shortcut_pair(k, l));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_captions(f: &mut Frame, app: &App, area: Rect) {
    let (label, text, color) = match app.caption_status {
        CaptionStatus::Idle => (" CC [c] ", "(off)".to_string(), Color::DarkGray),
        CaptionStatus::Loading => (" CC — loading [c] ", "…".to_string(), Color::DarkGray),
        CaptionStatus::None => (
            " CC — none available [c] ",
            "(this track has no captions)".to_string(),
            Color::DarkGray,
        ),
        CaptionStatus::Error => (
            " CC — error [c] ",
            "(yt-dlp failed to fetch captions)".to_string(),
            Color::Red,
        ),
        CaptionStatus::Ready => (" CC [c] ", app.current_captions().join("\n"), Color::White),
    };
    let block = Block::default().borders(Borders::ALL).title(label);
    let p = Paragraph::new(text)
        .style(Style::default().fg(color))
        .wrap(Wrap { trim: true })
        .block(block);
    f.render_widget(p, area);
}

fn device_name_str(app: &App) -> String {
    match &app.output_device {
        Some(d) => d.name.clone(),
        None => "(default system output)".to_string(),
    }
}

fn device_model_str(app: &App) -> String {
    match &app.output_device {
        Some(d) => {
            if d.bluetooth {
                format!("{}, {}", d.kind.label(), d.transport)
            } else {
                format!("{}, {}", d.kind.label(), d.transport)
            }
        }
        None => "(generic audio)".to_string(),
    }
}

fn draw_nerd_overlay(f: &mut Frame, app: &App, area: Rect) {
    let w = 56.min(area.width.saturating_sub(4));
    let h = 17.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let s = app.stats();
    let st = app.player_state();
    let cpu_color = if s.total_cpu() > 50.0 {
        Color::Red
    } else if s.total_cpu() > 15.0 {
        Color::Yellow
    } else {
        Color::Green
    };
    let key = Style::default().fg(Color::DarkGray);
    let val = Style::default().add_modifier(Modifier::BOLD);
    let bitrate_str = match st.audio_bitrate {
        Some(b) if b > 0.0 => format!("{:.0} kbps", b / 1000.0),
        _ => "—".to_string(),
    };
    let rate_str = match st.samplerate {
        Some(r) => format!("{} Hz", r),
        None => "—".to_string(),
    };
    let chan_str = match st.channels {
        Some(c) => format!("{} ch", c),
        None => "—".to_string(),
    };
    let codec_str = st.audio_codec.clone().unwrap_or_else(|| "—".to_string());

    let row = |k: &'static str, v: Span<'static>| -> Line<'static> {
        Line::from(vec![Span::styled(format!("  {:<11}", k), key), v])
    };

    let lines = vec![
        row(
            "ytmtui",
            Span::styled(format!("v{}", env!("CARGO_PKG_VERSION")), val),
        ),
        row(
            "mpv",
            Span::styled(app.mpv_version.clone().unwrap_or_else(|| "—".into()), val),
        ),
        row(
            "yt-dlp",
            Span::styled(app.ytdlp_version.clone().unwrap_or_else(|| "—".into()), val),
        ),
        Line::from(""),
        row(
            "CPU",
            Span::styled(
                format!(
                    "{:>5.1}%  (ui {:.1} + mpv {:.1})",
                    s.total_cpu(),
                    s.self_proc.cpu_percent,
                    s.mpv.cpu_percent
                ),
                Style::default().fg(cpu_color).add_modifier(Modifier::BOLD),
            ),
        ),
        row(
            "RAM",
            Span::styled(
                format!(
                    "{}  (ui {} + mpv {})",
                    fmt_bytes(s.total_rss()),
                    fmt_bytes(s.self_proc.rss_bytes),
                    fmt_bytes(s.mpv.rss_bytes),
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ),
        Line::from(""),
        row("codec", Span::styled(codec_str, val)),
        row("bitrate", Span::styled(bitrate_str, val)),
        row("sample-rate", Span::styled(rate_str, val)),
        row("channels", Span::styled(chan_str, val)),
        Line::from(""),
        row("device", Span::styled(device_name_str(app), val)),
        row("model", Span::styled(device_model_str(app), val)),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled("nerd stats", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("  [/, Esc to close] ", Style::default().fg(Color::DarkGray)),
        ]))
        .style(Style::default().bg(Color::Black));
    let p = Paragraph::new(lines).block(block);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let (title, content, style) = match app.mode {
        Mode::Searching => {
            let mut spans = vec![Span::raw(" Search ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("↵", "submit"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("esc", "cancel"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                format!("{}_", app.query),
                Style::default().fg(Color::Yellow),
            )
        }
        Mode::OpenFile => {
            let mut spans = vec![Span::raw(" Open file ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("↵", "play"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("esc", "cancel"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                format!("{}_", app.query),
                Style::default().fg(Color::Yellow),
            )
        }
        Mode::YtPlaylistInput => {
            let mut spans = vec![Span::raw(" YT playlist URL ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("↵", "load"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("esc", "cancel"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                format!("{}_", app.query),
                Style::default().fg(Color::Yellow),
            )
        }
        Mode::SavePlaylist => {
            let mut spans = vec![Span::raw(" Save Unsaved as ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("↵", "save"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("esc", "cancel"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                format!("{}_", app.query),
                Style::default().fg(Color::Yellow),
            )
        }
        _ => {
            let mut spans = vec![Span::raw(" Search ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("s", "search"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("o", "open file"));
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("p", "yt playlist"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                if app.query.is_empty() {
                    "Press `s` to search, `o` to open file, `p` to load a YT playlist..."
                        .to_string()
                } else {
                    app.query.clone()
                },
                Style::default().fg(Color::DarkGray),
            )
        }
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let p = Paragraph::new(content).style(style).block(block);
    f.render_widget(p, area);
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    // The Y/B Playlist section actually carries two stacked panes: the
    // library of saved playlists and the tracks of the active one. Render
    // both together when either is focused.
    if matches!(app.focus, ListFocus::YtLibrary | ListFocus::YtPlaylist) {
        draw_remote_playlist_split(f, app, area);
        return;
    }
    let (tracks, selected) = match app.focus {
        ListFocus::Results => (app.results.as_slice(), app.selected),
        ListFocus::LocalFolder => (app.local_folder.as_slice(), app.local_folder_selected),
        ListFocus::YtLibrary | ListFocus::YtPlaylist => unreachable!(),
    };

    let current_id = app.current_track().map(|t| t.id.clone());

    let items: Vec<ListItem> = tracks
        .iter()
        .map(|t| {
            // ▶ matches by id regardless of which list — playback always
            // references the playlist now, but a result may also represent
            // the playing track.
            let is_playing = current_id.as_deref() == Some(&t.id);
            let marker = if is_playing { "▶ " } else { "  " };
            let dur = t.duration_str();
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(Color::Green)),
                Span::raw(format!("{:>5}  ", dur)),
                Span::styled(
                    t.source_glyph().to_string(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
            ];
            if let Some(d) = t.local_depth {
                spans.push(Span::styled(
                    crate::local_scan::depth_marker(d),
                    Style::default().fg(Color::DarkGray),
                ));
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                t.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if !t.is_local() && !t.uploader.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("— {}", t.uploader),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let items = if items.is_empty() {
        let hint = match app.focus {
            ListFocus::Results => {
                "Press `s` to search YouTube, `o` to open a local file or folder..."
            }
            ListFocus::LocalFolder => {
                if app.local_folder_scanning {
                    "scanning..."
                } else {
                    "Press `o` and give a directory path to scan (recursive, up to depth 4)."
                }
            }
            // YtLibrary / YtPlaylist render via draw_remote_playlist_split
            // and never reach this branch.
            ListFocus::YtLibrary | ListFocus::YtPlaylist => unreachable!(),
        };
        vec![ListItem::new(Line::from(Span::styled(
            format!("  {}", hint),
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        items
    };

    let title = build_tab_title(app);
    let mut block = Block::default().borders(Borders::ALL).title(title);
    if let Some(total) = total_duration_for_focus(app, tracks) {
        block = block.title(total.right_aligned());
    }
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");

    let mut state = ListState::default();
    if !tracks.is_empty() {
        state.select(Some(selected.min(tracks.len() - 1)));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_remote_playlist_split(f: &mut Frame, app: &App, area: Rect) {
    // Outer block reuses the regular tab title so the user still sees
    // [ Results · Playlist · Y Playlist · ⌂ ] context.
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(build_tab_title(app))
        .title(
            total_duration_for_focus(app, app.active_tracks())
                .map(|l| l.right_aligned())
                .unwrap_or_else(|| Line::from("")),
        );
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    // Wider terminals get the library on the left; narrow ones stack it
    // on top so each pane keeps a usable row count.
    let horizontal = inner.width >= 110;
    let panes = if horizontal {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(inner)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(inner)
    };

    draw_library_pane(f, app, panes[0], app.focus == ListFocus::YtLibrary);
    draw_yt_tracks_pane(f, app, panes[1], app.focus == ListFocus::YtPlaylist);
}

fn draw_library_pane(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let entries = &app.library.entries;
    let unsaved_visible = app.unsaved_visible();

    let mut items: Vec<ListItem> = Vec::new();
    let dim = Style::default().fg(Color::DarkGray);
    if unsaved_visible {
        let is_active = matches!(app.active_library, crate::app::ActiveLibrary::Unsaved);
        let marker = if is_active { "▶ " } else { "  " };
        let unsaved_style = Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD);
        let unsaved_total: u64 = app.playlist.iter().filter_map(|t| t.duration).sum();
        let dur = fmt_total_duration(unsaved_total).unwrap_or_else(|| "--".into());
        items.push(ListItem::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(Color::Green)),
            Span::styled("✎ ", unsaved_style),
            Span::styled("  ", Style::default()), // align with the favorite glyph slot
            Span::styled("Unsaved", unsaved_style),
            Span::raw("  "),
            Span::styled(format!("({})", app.playlist.len()), dim),
            Span::raw("  "),
            Span::styled(dur, dim),
        ])));
    }
    for (i, e) in entries.iter().enumerate() {
        let is_active = matches!(app.active_library, crate::app::ActiveLibrary::Saved(j) if j == i);
        let marker = if is_active { "▶ " } else { "  " };
        let fav = if e.favorite { "★" } else { "☆" };
        let glyph = match e.platform {
            crate::ytdlp::Platform::Bilibili => "B",
            crate::ytdlp::Platform::Local => "⌂",
            _ => "Y",
        };
        let dur = e
            .total_duration
            .and_then(fmt_total_duration)
            .unwrap_or_else(|| "--".into());
        items.push(ListItem::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(Color::Green)),
            Span::styled(
                format!("{} ", fav),
                if e.favorite {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(
                format!("{} ", glyph),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                e.title.clone(),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(format!("({})", e.track_count), dim),
            Span::raw("  "),
            Span::styled(dur, dim),
        ])));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  Empty. Press `p` to add a playlist URL, or play tracks to seed Unsaved.".to_string(),
            Style::default().fg(Color::DarkGray),
        ))));
    }

    let title_style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut title_spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("Saved Playlists ({})", app.saved_playlist_row_count()),
            title_style,
        ),
        Span::raw(" "),
    ];
    if app.config.show_shortcuts {
        for (k, l) in [
            ("⇥", "switch"),
            ("↵", "activate"),
            ("S", "save Unsaved"),
            ("f", "fav"),
            ("d/⌫", "remove"),
        ] {
            title_spans.push(shortcut_sep());
            title_spans.extend(shortcut_pair(k, l));
        }
        title_spans.push(Span::raw(" "));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title_spans));
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(if focused {
                    Color::Blue
                } else {
                    Color::DarkGray
                })
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");

    let total_rows = app.saved_playlist_row_count();
    let mut state = ListState::default();
    if total_rows > 0 {
        state.select(Some(app.library_selected.min(total_rows - 1)));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_yt_tracks_pane(f: &mut Frame, app: &App, area: Rect, focused: bool) {
    let tracks = app.active_tracks();
    let current_id = app.current_track().map(|t| t.id.clone());

    let items: Vec<ListItem> = tracks
        .iter()
        .map(|t| {
            let is_playing = current_id.as_deref() == Some(&t.id);
            let marker = if is_playing { "▶ " } else { "  " };
            let dur = t.duration_str();
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(Color::Green)),
                Span::raw(format!("{:>5}  ", dur)),
                Span::styled(
                    t.source_glyph().to_string(),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    t.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ];
            if !t.is_local() && !t.uploader.is_empty() {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("— {}", t.uploader),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let items = if items.is_empty() {
        let hint = if app.yt_playlist_loading {
            "loading..."
        } else if matches!(app.active_library, crate::app::ActiveLibrary::Unsaved) {
            "Unsaved is empty. Play something from Results, ⌂, or a saved entry."
        } else {
            "No tracks. Pick an entry in Saved Playlists and press Enter."
        };
        vec![ListItem::new(Line::from(Span::styled(
            format!("  {}", hint),
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        items
    };

    let title_style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let active_name = app.active_title();
    let label = if app.yt_playlist_loading {
        format!("Tracks · {} (loading…)", active_name)
    } else {
        format!("Tracks · {} ({})", active_name, tracks.len())
    };
    let mut title_spans = vec![
        Span::raw(" "),
        Span::styled(label, title_style),
        Span::raw(" "),
    ];
    if app.config.show_shortcuts {
        for (k, l) in [("⇥", "switch"), ("+", "add"), ("↵", "play"), ("y", "URL")] {
            title_spans.push(shortcut_sep());
            title_spans.extend(shortcut_pair(k, l));
        }
        title_spans.push(Span::raw(" "));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title_spans));
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(if focused {
                    Color::Blue
                } else {
                    Color::DarkGray
                })
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("» ");

    let mut state = ListState::default();
    if !tracks.is_empty() {
        state.select(Some(app.yt_playlist_selected.min(tracks.len() - 1)));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// Human-readable total. Returns `None` when the input is zero / empty
/// so callers can render `--` instead of `0:00`.
pub fn fmt_total_duration(total: u64) -> Option<String> {
    if total == 0 {
        return None;
    }
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    Some(if h > 0 {
        format!("{}h {:02}m", h, m)
    } else {
        format!("{}:{:02}", m, s)
    })
}

fn total_duration_for_focus(app: &App, tracks: &[crate::ytdlp::Track]) -> Option<Line<'static>> {
    // Results tab doesn't get a total — by design.
    if app.focus == ListFocus::Results || tracks.is_empty() {
        return None;
    }
    let total: u64 = tracks.iter().filter_map(|t| t.duration).sum();
    let body = fmt_total_duration(total)?;
    Some(Line::from(vec![
        Span::styled(" total ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            body,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ]))
}

fn build_device_title(app: &App) -> Line<'static> {
    let on = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let (kind, bt) = match &app.output_device {
        Some(d) => (d.kind, d.bluetooth),
        None => (DeviceKind::Unknown, false),
    };
    let vol_block = audio::volume_block(app.config.volume);
    let mut spans: Vec<Span<'static>> =
        vec![Span::raw(" "), Span::styled(kind.kaomoji().to_string(), on)];
    if bt {
        spans.push(Span::raw(" "));
        spans.push(Span::styled("ᛒ", on));
    }
    spans.push(Span::raw(" "));
    spans.push(Span::styled(vol_block.to_string(), on));
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn queue_source_label(app: &App) -> String {
    match &app.queue_source {
        QueueSource::Results => "search results".to_string(),
        QueueSource::Unsaved => "Unsaved".to_string(),
        QueueSource::Saved { name } => name.clone(),
        QueueSource::LocalFolder => {
            let name = app.config.local_folder_label.as_deref().unwrap_or("folder");
            format!("⌂: {}", name)
        }
    }
}

fn build_now_playing_title(app: &App) -> Line<'static> {
    let on = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = label_style();
    let mut spans = vec![Span::raw(" Now Playing ")];
    if app.current_track().is_some() {
        spans.push(Span::styled("· from: ", dim));
        spans.push(Span::styled(format!("{} ", queue_source_label(app)), on));
    }
    if !app.config.show_shortcuts {
        // Still surface state indicators when shortcuts are hidden.
        let off = label_style();
        spans.push(match app.config.loop_mode {
            LoopMode::Off => Span::styled("· loop:off ", off),
            LoopMode::All => Span::styled("· loop:all ", on),
            LoopMode::One => Span::styled("· loop:one ", on),
        });
        spans.push(if app.config.shuffle {
            Span::styled("· shuffle ", on)
        } else {
            Span::styled("· shuffle:off ", off)
        });
        return Line::from(spans);
    }
    // Shortcuts on: state and key fused per entry. Key cyan, state-label
    // yellow when active else dim.
    let loop_state_style = match app.config.loop_mode {
        LoopMode::Off => label_style(),
        _ => on,
    };
    let shuffle_state_style = if app.config.shuffle {
        on
    } else {
        label_style()
    };

    spans.push(shortcut_sep());
    spans.push(Span::styled("L", key_style()));
    spans.push(Span::styled(
        format!(" loop:{}", app.config.loop_mode.label()),
        loop_state_style,
    ));
    spans.push(shortcut_sep());
    spans.push(Span::styled("H", key_style()));
    spans.push(Span::styled(
        if app.config.shuffle {
            " shuffle".to_string()
        } else {
            " shuffle:off".to_string()
        },
        shuffle_state_style,
    ));

    for (k, l) in [
        ("␣", "pause"),
        ("n/b", "skip"),
        ("f/r", "±10s"),
        ("z/x", "vol"),
        ("c", "CC"),
    ] {
        spans.push(shortcut_sep());
        spans.extend(shortcut_pair(k, l));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn build_tab_title(app: &App) -> Line<'static> {
    let active = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let inactive = Style::default().fg(Color::DarkGray);
    let results_label = if app.searching {
        "Results (searching…)".to_string()
    } else {
        format!("Results ({})", app.results.len())
    };
    let library_label = format!("Library ({})", app.saved_playlist_row_count());
    let local_label = {
        let name = app.config.local_folder_label.as_deref().unwrap_or("—");
        if app.local_folder_scanning {
            format!("⌂: {} (scanning…)", name)
        } else {
            format!("⌂: {} ({})", name, app.local_folder.len())
        }
    };

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    let focus_matches = |slot: ListFocus| -> bool {
        match slot {
            ListFocus::YtPlaylist => {
                matches!(app.focus, ListFocus::YtPlaylist | ListFocus::YtLibrary)
            }
            other => app.focus == other,
        }
    };
    for (label, focus) in [
        (results_label, ListFocus::Results),
        (library_label, ListFocus::YtPlaylist),
        (local_label, ListFocus::LocalFolder),
    ] {
        if focus_matches(focus) {
            spans.push(Span::raw("[ "));
            spans.push(Span::styled(label, active));
            spans.push(Span::raw(" ]  "));
        } else {
            spans.push(Span::styled(label, inactive));
            spans.push(Span::raw("  "));
        }
    }

    if app.config.show_shortcuts {
        // Outer hints stay minimal for the Library section; each sub-pane
        // (Saved Playlists / Tracks) carries its own hints in its inner
        // title.
        let context: &[(&str, &str)] = match app.focus {
            ListFocus::Results => &[("⇥", "switch"), ("+", "add"), ("↵", "play"), ("y", "URL")],
            ListFocus::YtPlaylist | ListFocus::YtLibrary => {
                &[("⇥", "switch"), ("p", "add URL"), ("S", "save Unsaved")]
            }
            ListFocus::LocalFolder => &[
                ("⇥", "switch"),
                ("+", "add"),
                ("↵", "play"),
                ("o", "change folder"),
            ],
        };
        for (k, l) in context.iter() {
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair(k, l));
        }
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(build_now_playing_title(app))
        .title(build_device_title(app).right_aligned());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let st = app.player_state();
    let (marker, title_text) = match app.current_track() {
        Some(t) => {
            let pause = if st.paused { "⏸ " } else { "▶ " };
            let body = if t.is_local() || t.uploader.is_empty() {
                t.title.clone()
            } else {
                format!("{} — {}", t.title, t.uploader)
            };
            (pause, body)
        }
        None => (
            "  ",
            "(nothing playing — pick a track and press Enter)".to_string(),
        ),
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let (pos, dur) = (st.position.max(0.0), st.duration.max(0.0));
    let label = if app.current_track().is_some() {
        format!("  {}  /  {}", fmt_secs(pos), fmt_secs(dur))
    } else {
        String::new()
    };
    let label_w = cell_width(&label);
    let marker_w = cell_width(marker);
    let row_w = rows[0].width as usize;
    let title_max = row_w
        .saturating_sub(marker_w)
        .saturating_sub(label_w)
        .max(8);
    let title_part = marquee(&title_text, title_max);
    let dim = Style::default().fg(Color::DarkGray);
    let title_line = Line::from(vec![
        Span::styled(marker.to_string(), Style::default().fg(Color::Green)),
        Span::raw(title_part),
        Span::styled(label, dim),
    ]);
    f.render_widget(Paragraph::new(title_line), rows[0]);

    let ratio = if dur > 0.0 {
        (pos / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };
    const MAX_BAR: u16 = 80;
    const SAFETY: u16 = 4;
    let bar_width = rows[1].width.saturating_sub(SAFETY).min(MAX_BAR);
    let spans = cursor_spans(app.current_sprite(), ratio, bar_width);
    f.render_widget(Paragraph::new(Line::from(spans)), rows[1]);
}

/// Display-cell width of one char. Covers CJK + full-width punctuation
/// (the common cases in this UI) without pulling in `unicode-width`.
fn char_cells(c: char) -> usize {
    if c.is_control() {
        return 0;
    }
    let code = c as u32;
    let wide = matches!(
        code,
        0x1100..=0x115F        // Hangul Jamo
        | 0x2E80..=0x303E      // CJK radicals, Kangxi, CJK symbols start
        | 0x3041..=0x33FF      // Hiragana, Katakana, CJK symbols
        | 0x3400..=0x4DBF      // CJK extension A
        | 0x4E00..=0x9FFF      // CJK unified ideographs
        | 0xA000..=0xA4CF      // Yi
        | 0xAC00..=0xD7A3      // Hangul syllables
        | 0xF900..=0xFAFF      // CJK compat ideographs
        | 0xFE30..=0xFE4F      // CJK compat forms
        | 0xFF00..=0xFF60      // Fullwidth ASCII
        | 0xFFE0..=0xFFE6      // Fullwidth signs
    );
    if wide {
        2
    } else {
        1
    }
}

fn cell_width(s: &str) -> usize {
    s.chars().map(char_cells).sum()
}

/// Pad a short title to `max_cells` or scroll it like a banner ad when it
/// is wider than the row. Steps one *char* per ~300ms so CJK content still
/// reads smoothly even though each glyph eats two cells.
fn marquee(title: &str, max_cells: usize) -> String {
    if max_cells == 0 {
        return String::new();
    }
    let chars: Vec<char> = title.chars().collect();
    let title_w = cell_width(title);
    if title_w <= max_cells {
        // Fits — render as-is, padded right with spaces so trailing
        // content (the time label) lands flush against the row edge.
        let mut s: String = chars.iter().collect();
        for _ in 0..(max_cells - title_w) {
            s.push(' ');
        }
        return s;
    }

    let separator: Vec<char> = "   •   ".chars().collect();
    let combined: Vec<char> = chars.iter().chain(separator.iter()).cloned().collect();
    let cycle_len = combined.len();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    const STEP_MS: u128 = 300;
    let offset = (now_ms / STEP_MS) as usize % cycle_len;

    let mut out = String::with_capacity(max_cells * 4);
    let mut used = 0usize;
    let mut i = 0usize;
    while used < max_cells && i < cycle_len * 2 {
        let c = combined[(offset + i) % cycle_len];
        let w = char_cells(c);
        if used + w > max_cells {
            // Next glyph would overflow (a CJK char sneaking past the
            // boundary): pad the remaining cell with a space and stop.
            out.push(' ');
            used += 1;
            break;
        }
        out.push(c);
        used += w;
        i += 1;
    }
    while used < max_cells {
        out.push(' ');
        used += 1;
    }
    out
}

fn cursor_spans(sprite: &Sprite, ratio: f64, width: u16) -> Vec<Span<'static>> {
    let ratio = ratio.clamp(0.0, 1.0);
    // Use the max frame width for position math so the cursor's column stays
    // stable across frames of different widths.
    let max_w = sprite
        .frames
        .iter()
        .map(|f| f.chars().count() as u16)
        .max()
        .unwrap_or(0);

    if width < max_w + 2 {
        return vec![Span::raw(sprite.frame(0).to_string())];
    }
    let track_w = width - max_w;
    let cat_pos = (ratio * track_w as f64).round() as u16;
    let cat_pos = cat_pos.min(track_w);

    let frame_idx = match sprite.animate_on {
        AnimateOn::Move => (cat_pos as usize) % sprite.frame_count(),
        AnimateOn::Tick => {
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
                / 250) as usize
                % sprite.frame_count()
        }
    };
    let cat = sprite.frame(frame_idx).to_string();
    let cat_w = cat.chars().count() as u16;

    let trail = fill_left(&sprite.trail_left, cat_pos as usize);
    let lead_w = width.saturating_sub(cat_pos).saturating_sub(cat_w) as usize;
    let lead = fill_right(&sprite.trail_right, lead_w);
    vec![
        Span::styled(trail, Style::default().fg(sprite.accent)),
        Span::styled(
            cat,
            Style::default()
                .fg(sprite.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(lead, Style::default().fg(Color::DarkGray)),
    ]
}

/// Repeat `pattern` so its last char sits flush against the cursor.
/// e.g. pattern "abc" width 5 -> "bcabc".
fn fill_left(pattern: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let pchars: Vec<char> = pattern.chars().collect();
    if pchars.is_empty() {
        return " ".repeat(width);
    }
    let plen = pchars.len();
    let start = (plen - width % plen) % plen;
    (0..width).map(|i| pchars[(start + i) % plen]).collect()
}

/// Repeat `pattern` so its first char sits flush against the cursor.
fn fill_right(pattern: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let pchars: Vec<char> = pattern.chars().collect();
    if pchars.is_empty() {
        return " ".repeat(width);
    }
    let plen = pchars.len();
    (0..width).map(|i| pchars[i % plen]).collect()
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let style = Style::default().fg(Color::DarkGray);
    let p = Paragraph::new(app.status.as_str()).style(style);
    f.render_widget(p, area);
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let w = 60.min(area.width.saturating_sub(4));
    let h = 29.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let text = vec![
        Line::from(Span::styled(
            " ytmtui — keybindings ",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("  s        Search YouTube"),
        Line::from("  o        Open local file (path prompt)"),
        Line::from("  Enter    Play selected (queues from results)"),
        Line::from("  Space    Pause / resume"),
        Line::from("  n        Next track"),
        Line::from("  b        Back (previous track)"),
        Line::from("  f / F    Forward 10s / 1min"),
        Line::from("  r / R    Rewind  10s / 1min"),
        Line::from("  j / k    Move selection down / up"),
        Line::from("  Tab      Switch focus (Results / Playlist)"),
        Line::from("  +        Add selected result to playlist"),
        Line::from("  -/⌫/Del  Remove selected playlist entry"),
        Line::from("  L / l    Cycle loop mode (off → all → one)"),
        Line::from("  H / h    Toggle shuffle"),
        Line::from("  /        Toggle nerd-stats modal"),
        Line::from("  z / x    Volume down / up (10% steps)"),
        Line::from("  c        Toggle closed captions"),
        Line::from("  y        Yank (copy) selected track URL"),
        Line::from("  o        Open a local file or scan a folder (search bar)"),
        Line::from("  p        Load YT/Bilibili playlist URL (search bar)"),
        Line::from("  `        Parameters menu"),
        Line::from("  .        Hide / show shortcut bar"),
        Line::from("  ?        Toggle this help"),
        Line::from("  q        Quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close.",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Help ")
        .style(Style::default().bg(Color::Black));
    let p = Paragraph::new(text).block(block);
    // clear under it
    f.render_widget(ratatui::widgets::Clear, rect);
    f.render_widget(p, rect);
}

fn fmt_secs(s: f64) -> String {
    let s = s as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{}:{:02}:{:02}", h, m, sec)
    } else {
        format!("{}:{:02}", m, sec)
    }
}

fn draw_params_overlay(f: &mut Frame, app: &App, area: Rect) {
    let w = 56.min(area.width.saturating_sub(4));
    let h = 12.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    let sprite = app.current_sprite();
    let registry = app.sprites();
    let total = registry.all().len();
    let idx = registry.index_of(&sprite.id);
    let preview = sprite.frame(0).to_string();
    let langs = crate::config::CAPTION_LANGS;
    let lang_idx = langs
        .iter()
        .position(|l| *l == app.config.caption_lang.as_str())
        .unwrap_or(0);

    let marker_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(Color::Yellow);
    let dim = Style::default().fg(Color::DarkGray);

    let marker = |row_idx: usize| -> Span<'static> {
        Span::styled(
            if app.params_row == row_idx {
                " ▶ "
            } else {
                "   "
            },
            marker_style,
        )
    };

    let rows: Vec<Line> = vec![
        Line::from(Span::styled(
            " Parameters ",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            marker(0),
            Span::raw("Progress cursor    "),
            Span::styled("◀ ", dim),
            Span::styled(
                format!("{:^12}", sprite.name),
                Style::default()
                    .fg(sprite.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ▶", dim),
        ]),
        Line::from(vec![
            Span::raw("      preview: "),
            Span::styled(
                preview,
                Style::default()
                    .fg(sprite.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(format!("({}/{})", idx + 1, total), dim),
        ]),
        Line::from(""),
        Line::from(vec![
            marker(1),
            Span::raw("CC language        "),
            Span::styled("◀ ", dim),
            Span::styled(
                format!("{:^12}", app.config.caption_lang),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ▶", dim),
            Span::raw("  "),
            Span::styled(format!("({}/{})", lang_idx + 1, langs.len()), dim),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ↑/↓ ", key_style),
            Span::styled("row   ", dim),
            Span::styled("←/→ ", key_style),
            Span::styled("change   ", dim),
            Span::styled("`/Esc ", key_style),
            Span::styled("close", dim),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Parameters ")
        .style(Style::default().bg(Color::Black));
    let p = Paragraph::new(rows).block(block);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}
