use anyhow::Result;
use ytmtui::{app::App, config, gui, library, player::Player, playlist, sprites, ytdlp};

fn main() -> Result<()> {
    if let Err(e) = ytdlp::check_installed() {
        eprintln!("{e}");
        std::process::exit(1);
    }
    if let Err(e) = ytmtui::player::check_installed() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let player = Player::spawn()?;
    let cfg = config::load();
    let registry = sprites::Registry::load();
    let pl = playlist::load();
    let yt_pl = playlist::load_yt();
    let local_pl = playlist::load_local();
    let library = library::load();
    let app = App::new(player, cfg, registry, pl, yt_pl, local_pl, library);

    gui::run(app)
}
