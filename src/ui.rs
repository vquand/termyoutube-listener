use crate::app::{App, Mode};
use crate::stats::fmt_bytes;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search bar
            Constraint::Min(3),    // results
            Constraint::Length(4), // now playing
            Constraint::Length(1), // status
        ])
        .split(area);

    draw_search(f, app, chunks[0]);
    draw_results(f, app, chunks[1]);
    draw_now_playing(f, app, chunks[2]);
    draw_status(f, app, chunks[3]);

    if app.show_stats {
        draw_stats_corner(f, app, area);
    }

    if app.mode == Mode::Help {
        draw_help_overlay(f, area);
    }
}

fn draw_stats_corner(f: &mut Frame, app: &App, area: Rect) {
    let s = app.stats();
    let w: u16 = 30;
    let h: u16 = 5;
    if area.width < w + 2 || area.height < h + 2 {
        return;
    }
    let rect = Rect {
        x: area.width - w - 1,
        y: 1,
        width: w,
        height: h,
    };

    let cpu_color = if s.total_cpu() > 50.0 {
        Color::Red
    } else if s.total_cpu() > 15.0 {
        Color::Yellow
    } else {
        Color::Green
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("CPU ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>5.1}%", s.total_cpu()),
                Style::default().fg(cpu_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  (ui {:.1} + mpv {:.1})", s.self_proc.cpu_percent, s.mpv.cpu_percent),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("RAM ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:>8}", fmt_bytes(s.total_rss())),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  (ui {} + mpv {})",
                    fmt_bytes(s.self_proc.rss_bytes),
                    fmt_bytes(s.mpv.rss_bytes)
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled("GPU ", Style::default().fg(Color::DarkGray)),
            Span::styled("idle", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(" (audio-only, --no-video)", Style::default().fg(Color::DarkGray)),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Usage [t] ")
        .style(Style::default().bg(Color::Black));
    let p = Paragraph::new(lines).block(block);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

fn draw_search(f: &mut Frame, app: &App, area: Rect) {
    let (title, content, style) = match app.mode {
        Mode::Searching => (
            " Search (Enter to submit, Esc to cancel) ",
            format!("{}_", app.query),
            Style::default().fg(Color::Yellow),
        ),
        _ => (
            " Search [s] ",
            if app.query.is_empty() {
                "Press `s` to search YouTube...".to_string()
            } else {
                app.query.clone()
            },
            Style::default().fg(Color::DarkGray),
        ),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let p = Paragraph::new(content).style(style).block(block);
    f.render_widget(p, area);
}

fn draw_results(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .results
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let marker = if Some(i) == app.current { "▶ " } else { "  " };
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

    let title = if app.searching {
        " Results (searching…) ".to_string()
    } else if app.results.is_empty() {
        " Results — empty. Press `s` to search ".to_string()
    } else {
        format!(" Results ({}) ", app.results.len())
    };

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
    if !app.results.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_now_playing(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Now Playing ");
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
    let label = format!("{}  /  {}", fmt_secs(pos), fmt_secs(dur));
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::Cyan).bg(Color::Black))
        .ratio(ratio)
        .label(label);
    f.render_widget(gauge, rows[1]);
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let style = Style::default().fg(Color::DarkGray);
    let p = Paragraph::new(app.status.as_str()).style(style);
    f.render_widget(p, area);
}

fn draw_help_overlay(f: &mut Frame, area: Rect) {
    let w = 60.min(area.width.saturating_sub(4));
    let h = 17.min(area.height.saturating_sub(4));
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
        Line::from("  t        Toggle CPU/RAM stats corner"),
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
