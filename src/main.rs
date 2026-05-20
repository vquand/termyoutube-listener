mod app;
mod audio;
mod captions;
mod clipboard;
mod config;
mod local_scan;
mod player;
mod playlist;
mod sprites;
mod stats;
mod ui;
mod ytdlp;

use anyhow::Result;
use app::{App, Mode};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use player::Player;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    if let Err(e) = ytdlp::check_installed() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    if let Err(e) = player::check_installed() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let player = Player::spawn()?;
    let cfg = config::load();
    let registry = sprites::Registry::load();
    let pl = playlist::load();
    let yt_pl = playlist::load_yt();
    let local_pl = playlist::load_local();
    let mut app = App::new(player, cfg, registry, pl, yt_pl, local_pl);

    let mut terminal = setup_terminal()?;
    let res = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    res
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let tick = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    while !app.should_quit {
        app.drain_events();
        app.refresh_stats();
        terminal.draw(|f| ui::draw(f, app))?;

        let timeout = tick.saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    handle_key(app, key.code, key.modifiers);
                }
                Event::Paste(text) => {
                    app.handle_paste(&text);
                }
                _ => {}
            }
        }
        if last_tick.elapsed() >= tick {
            last_tick = Instant::now();
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, mods: KeyModifiers) {
    match app.mode {
        Mode::Help => {
            app.mode = Mode::Browse;
        }
        Mode::Nerd => match code {
            KeyCode::Esc | KeyCode::Char('/') | KeyCode::Char('q') => app.mode = Mode::Browse,
            _ => {}
        },
        Mode::Searching => match code {
            KeyCode::Esc => app.cancel_search(),
            KeyCode::Enter => app.submit_search(),
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Char(c) => app.query.push(c),
            _ => {}
        },
        Mode::OpenFile => match code {
            KeyCode::Esc => app.cancel_open_file(),
            KeyCode::Enter => app.submit_open_file(),
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Char(c) => app.query.push(c),
            _ => {}
        },
        Mode::YtPlaylistInput => match code {
            KeyCode::Esc => app.cancel_yt_playlist_input(),
            KeyCode::Enter => app.submit_yt_playlist(),
            KeyCode::Backspace => {
                app.query.pop();
            }
            KeyCode::Char(c) => app.query.push(c),
            _ => {}
        },
        Mode::Browse => {
            if mods.contains(KeyModifiers::CONTROL) {
                if let KeyCode::Char('c') = code {
                    app.should_quit = true;
                    return;
                }
            }
            match code {
                KeyCode::Char('q') => app.should_quit = true,
                KeyCode::Char('s') => app.enter_search(),
                KeyCode::Char('o') => app.enter_open_file(),
                KeyCode::Char('?') => app.mode = Mode::Help,
                KeyCode::Char('/') => app.toggle_nerd(),
                KeyCode::Char('c') => app.toggle_captions(),
                KeyCode::Char('y') => app.yank_selected_url(),
                KeyCode::Char('p') => app.enter_yt_playlist_input(),
                KeyCode::Char('`') => app.open_params(),
                KeyCode::Tab => app.switch_focus(),
                KeyCode::Char('+') => app.add_focused_to_playlist(),
                KeyCode::Char('-') | KeyCode::Backspace | KeyCode::Delete => {
                    app.remove_focused_from_playlist()
                }
                KeyCode::Char('L') | KeyCode::Char('l') => app.cycle_loop(),
                KeyCode::Char('H') | KeyCode::Char('h') => app.toggle_shuffle(),
                KeyCode::Char('.') => app.toggle_shortcuts(),
                KeyCode::Char('z') => app.volume_down(),
                KeyCode::Char('x') => app.volume_up(),
                KeyCode::Char(' ') => app.toggle_pause(),
                KeyCode::Enter => app.play_selected(),
                KeyCode::Char('n') => app.next_track(),
                KeyCode::Char('b') => app.prev_track(),
                KeyCode::Char('f') => app.seek(10.0),
                KeyCode::Char('F') => app.seek(60.0),
                KeyCode::Char('r') => app.seek(-10.0),
                KeyCode::Char('R') => app.seek(-60.0),
                KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
                KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
                _ => {}
            }
        }
        Mode::Params => match code {
            KeyCode::Esc | KeyCode::Char('`') | KeyCode::Char('q') => app.close_params(),
            KeyCode::Left | KeyCode::Char('h') => app.params_change(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter => app.params_change(1),
            KeyCode::Up | KeyCode::Char('k') => app.params_move(-1),
            KeyCode::Down | KeyCode::Char('j') => app.params_move(1),
            _ => {}
        },
    }
}
