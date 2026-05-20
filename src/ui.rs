use crate::app::{App, CaptionStatus, ListFocus, Mode};
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
        constraints.push(Constraint::Length(3)); // captions strip
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
}

fn key_style() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
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
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
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
        ("p", "params"),
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
        CaptionStatus::Ready => {
            let line = app.current_caption().unwrap_or("").to_string();
            (" CC [c] ", line, Color::White)
        }
    };
    let block = Block::default().borders(Borders::ALL).title(label);
    let p = Paragraph::new(text)
        .style(Style::default().fg(color))
        .wrap(Wrap { trim: true })
        .block(block);
    f.render_widget(p, area);
}

fn draw_nerd_overlay(f: &mut Frame, app: &App, area: Rect) {
    let w = 56.min(area.width.saturating_sub(4));
    let h = 14.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };

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
            Span::styled(
                app.mpv_version.clone().unwrap_or_else(|| "—".into()),
                val,
            ),
        ),
        row(
            "yt-dlp",
            Span::styled(
                app.ytdlp_version.clone().unwrap_or_else(|| "—".into()),
                val,
            ),
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
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ),
        Line::from(""),
        row("codec", Span::styled(codec_str, val)),
        row("bitrate", Span::styled(bitrate_str, val)),
        row("sample-rate", Span::styled(rate_str, val)),
        row("channels", Span::styled(chan_str, val)),
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
        _ => {
            let mut spans = vec![Span::raw(" Search ")];
            spans.push(shortcut_sep());
            spans.extend(shortcut_pair("s", "search"));
            spans.push(Span::raw(" "));
            (
                Line::from(spans),
                if app.query.is_empty() {
                    "Press `s` to search YouTube...".to_string()
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
    let (tracks, selected) = match app.focus {
        ListFocus::Results => (app.results.as_slice(), app.selected),
        ListFocus::Playlist => (app.playlist.as_slice(), app.playlist_selected),
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
            let line = Line::from(vec![
                Span::styled(marker, Style::default().fg(Color::Green)),
                Span::raw(format!("{:>5}  ", dur)),
                Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled(format!("— {}", t.uploader), Style::default().fg(Color::DarkGray)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let title = build_tab_title(app);
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
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

fn build_now_playing_title(app: &App) -> Line<'static> {
    let on = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" Now Playing ")];
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
    let shuffle_state_style = if app.config.shuffle { on } else { label_style() };

    spans.push(shortcut_sep());
    spans.push(Span::styled("L", key_style()));
    spans.push(Span::styled(
        format!(" loop:{}", app.config.loop_mode.label()),
        loop_state_style,
    ));
    spans.push(shortcut_sep());
    spans.push(Span::styled("H", key_style()));
    spans.push(Span::styled(
        if app.config.shuffle { " shuffle".to_string() } else { " shuffle:off".to_string() },
        shuffle_state_style,
    ));

    for (k, l) in [
        ("␣", "pause"),
        ("n/b", "skip"),
        ("f/r", "±10s"),
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
    let playlist_label = format!("Playlist ({})", app.playlist.len());
    let mut spans: Vec<Span> = match app.focus {
        ListFocus::Results => vec![
            Span::raw(" [ "),
            Span::styled(results_label, active),
            Span::raw(" ]  "),
            Span::styled(playlist_label, inactive),
        ],
        ListFocus::Playlist => vec![
            Span::raw(" "),
            Span::styled(results_label, inactive),
            Span::raw("  [ "),
            Span::styled(playlist_label, active),
            Span::raw(" ] "),
        ],
    };
    if app.config.show_shortcuts {
        let context = match app.focus {
            ListFocus::Results => [
                ("⇥", "switch"),
                ("+", "add"),
                ("↵", "play"),
                ("y", "URL"),
            ],
            ListFocus::Playlist => [
                ("⇥", "switch"),
                ("-", "remove"),
                ("↵", "play"),
                ("y", "URL"),
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
        .title(build_now_playing_title(app));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let st = app.player_state();
    let track_line = match app.current_track() {
        Some(t) => {
            let pause = if st.paused { "⏸ " } else { "▶ " };
            format!("{}{} — {}", pause, t.title, t.uploader)
        }
        None => "(nothing playing — pick a track and press Enter)".to_string(),
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    let p = Paragraph::new(track_line).wrap(Wrap { trim: true });
    f.render_widget(p, rows[0]);

    let (pos, dur) = (st.position.max(0.0), st.duration.max(0.0));
    let ratio = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) } else { 0.0 };
    let label = format!("  {}  /  {}", fmt_secs(pos), fmt_secs(dur));
    let label_width = label.chars().count() as u16;
    let bar_width = rows[1].width.saturating_sub(label_width);

    let mut spans = cursor_spans(app.current_sprite(), ratio, bar_width);
    spans.push(Span::styled(label, Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(Line::from(spans)), rows[1]);
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
            Style::default().fg(sprite.accent).add_modifier(Modifier::BOLD),
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
    let h = 26.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };

    let text = vec![
        Line::from(Span::styled(" ytmtui — keybindings ", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("  s        Search YouTube"),
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
        Line::from("  c        Toggle closed captions"),
        Line::from("  y        Yank (copy) selected track URL"),
        Line::from("  p        Parameters menu"),
        Line::from("  .        Hide / show shortcut bar"),
        Line::from("  ?        Toggle this help"),
        Line::from("  q        Quit"),
        Line::from(""),
        Line::from(Span::styled("  Press any key to close.", Style::default().fg(Color::DarkGray))),
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
    format!("{}:{:02}", s / 60, s % 60)
}

fn draw_params_overlay(f: &mut Frame, app: &App, area: Rect) {
    let w = 52.min(area.width.saturating_sub(4));
    let h = 9.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect { x, y, width: w, height: h };

    let sprite = app.current_sprite();
    let registry = app.sprites();
    let total = registry.all().len();
    let idx = registry.index_of(&sprite.id);
    let preview = sprite.frame(0).to_string();
    let marker_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let key_style = Style::default().fg(Color::Yellow);
    let dim = Style::default().fg(Color::DarkGray);

    let rows: Vec<Line> = vec![
        Line::from(Span::styled(
            " Parameters ",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                if app.params_row == 0 { " ▶ " } else { "   " },
                marker_style,
            ),
            Span::raw("Progress cursor    "),
            Span::styled("◀ ", dim),
            Span::styled(
                format!("{:^12}", sprite.name),
                Style::default().fg(sprite.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ▶", dim),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("   preview: "),
            Span::styled(
                preview,
                Style::default().fg(sprite.accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(format!("({}/{})", idx + 1, total), dim),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  ←/→ ", key_style),
            Span::styled("change   ", dim),
            Span::styled("Enter ", key_style),
            Span::styled("cycle   ", dim),
            Span::styled("p/Esc ", key_style),
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
