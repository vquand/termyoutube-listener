use crate::app::{ActiveLibrary, App, CaptionStatus, ListFocus, QueueSource};
use crate::config::CAPTION_LANGS;
use crate::library::PlaylistEntry;
use crate::stats::fmt_bytes;
use crate::ui::fmt_total_duration;
use crate::ytdlp::{Platform, Track};
use slint::{ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

slint::include_modules!();

const TICK: Duration = Duration::from_millis(200);

/// Drive the Slint window against a shared App. Blocks until the window
/// closes (or `quit` is requested).
pub fn run(app: App) -> anyhow::Result<()> {
    let font_family = register_fallback_fonts();
    let app = Arc::new(Mutex::new(app));
    crate::mpris::spawn(app.clone());
    let window = MainWindow::new()?;
    slint::set_xdg_app_id("ytmtui-gui")?;
    if let Some(family) = font_family {
        window.set_ui_font_family(family.into());
    }

    bind_callbacks(&window, &app);
    push_state(&window, &app.lock().unwrap());

    let timer = Timer::default();
    {
        let app = app.clone();
        let weak = window.as_weak();
        timer.start(TimerMode::Repeated, TICK, move || {
            let mut a = app.lock().unwrap();
            if a.should_quit {
                return;
            }
            a.drain_events();
            a.refresh_stats();
            if a.should_quit {
                if let Some(w) = weak.upgrade() {
                    let _ = w.hide();
                }
                return;
            }
            if let Some(w) = weak.upgrade() {
                push_state(&w, &a);
            }
        });
    }

    window.run()?;

    app.lock().unwrap().player.shutdown();
    Ok(())
}

fn bind_callbacks(
    window: &MainWindow,
    app: &Arc<Mutex<App>>,
) {
    // Search
    {
        let app = app.clone();
        window.on_submit_search(move |q, chip| {
            let prefix = match chip.as_str() {
                "youtube" => "#Y ",
                "bilibili" => "#B ",
                "local" => "#H ",
                _ => "",
            };
            let mut a = app.lock().unwrap();
            let full = format!("{}{}", prefix, q);
            if q.trim().is_empty() { return; }
            a.query = full;
            a.submit_search();
        });
    }

    // Open file or folder
    {
        let app = app.clone();
        window.on_open_file(move |q| {
            let mut a = app.lock().unwrap();
            if q.trim().is_empty() { return; }
            a.query = q.to_string();
            a.submit_open_file();
        });
    }

    // rfd has no combined file-or-folder picker; this picks folders, the
    // typed-path input above covers single files.
    {
        let app = app.clone();
        window.on_browse_file(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Select a folder to scan")
                .pick_folder();
            let Some(path) = picked else { return };
            let mut a = app.lock().unwrap();
            a.query = path.to_string_lossy().to_string();
            a.submit_open_file();
        });
    }

    // Open YT/Bilibili playlist URL
    {
        let app = app.clone();
        window.on_open_playlist_url(move |q| {
            let mut a = app.lock().unwrap();
            if q.trim().is_empty() { return; }
            a.query = q.to_string();
            a.submit_yt_playlist();
        });
    }

    // Save current Unsaved playlist with the name from the save dialog.
    {
        let app = app.clone();
        window.on_save_playlist(move |name| {
            let trimmed = name.trim();
            if trimmed.is_empty() { return; }
            let mut a = app.lock().unwrap();
            a.focus = ListFocus::YtLibrary;
            if a.playlist.is_empty() {
                a.status = "nothing to save - Unsaved is empty".into();
                return;
            }
            a.query = trimmed.to_string();
            a.submit_save_playlist();
        });
    }

    // Playback selectors — set the right focus/selection then play.
    {
        let app = app.clone();
        window.on_select_result(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.results.len()) {
                a.focus = ListFocus::Results;
                a.selected = idx;
            }
        });
    }
    {
        let app = app.clone();
        window.on_select_track(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.active_tracks().len()) {
                a.focus = ListFocus::YtPlaylist;
                a.yt_playlist_selected = idx;
            }
        });
    }
    {
        let app = app.clone();
        window.on_select_local(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.local_folder.len()) {
                a.focus = ListFocus::LocalFolder;
                a.local_folder_selected = idx;
            }
        });
    }
    {
        let app = app.clone();
        window.on_select_library_item(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.saved_playlist_row_count()) {
                a.focus = ListFocus::YtLibrary;
                a.library_selected = idx;
            }
        });
    }
    {
        let app = app.clone();
        window.on_play_result(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.results.len()) {
                a.focus = ListFocus::Results;
                a.selected = idx;
                a.play_selected();
            }
        });
    }
    {
        let app = app.clone();
        window.on_play_track(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.active_tracks().len()) {
                a.focus = ListFocus::YtPlaylist;
                a.yt_playlist_selected = idx;
                a.play_selected();
            }
        });
    }
    {
        let app = app.clone();
        window.on_play_local(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.local_folder.len()) {
                a.focus = ListFocus::LocalFolder;
                a.local_folder_selected = idx;
                a.play_selected();
            }
        });
    }

    // Add to Unsaved
    {
        let app = app.clone();
        window.on_add_result_to_playlist(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.results.len()) {
                a.focus = ListFocus::Results;
                a.selected = idx;
                a.add_focused_to_playlist();
            }
        });
    }
    {
        let app = app.clone();
        window.on_add_local_to_playlist(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.local_folder.len()) {
                a.focus = ListFocus::LocalFolder;
                a.local_folder_selected = idx;
                a.add_focused_to_playlist();
            }
        });
    }
    {
        let app = app.clone();
        window.on_remove_track(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.active_tracks().len()) {
                a.focus = ListFocus::YtPlaylist;
                a.yt_playlist_selected = idx;
                a.remove_focused_from_playlist();
            }
        });
    }

    // Library
    {
        let app = app.clone();
        window.on_activate_library_item(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.saved_playlist_row_count()) {
                a.focus = ListFocus::YtLibrary;
                a.library_selected = idx;
                a.activate_library_entry();
            }
        });
    }
    {
        let app = app.clone();
        window.on_toggle_favorite(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.saved_playlist_row_count()) {
                a.focus = ListFocus::YtLibrary;
                a.library_selected = idx;
                a.toggle_library_favorite();
            }
        });
    }
    {
        let app = app.clone();
        window.on_remove_library_entry(move |i| {
            let mut a = app.lock().unwrap();
            if let Some(idx) = clamp(i, a.saved_playlist_row_count()) {
                a.focus = ListFocus::YtLibrary;
                a.library_selected = idx;
                a.remove_library_entry();
            }
        });
    }

    // Transport
    {
        let app = app.clone();
        window.on_toggle_pause(move || {
            app.lock().unwrap().toggle_pause();
        });
    }
    {
        let app = app.clone();
        window.on_next_track(move || {
            app.lock().unwrap().next_track();
        });
    }
    {
        let app = app.clone();
        window.on_prev_track(move || {
            app.lock().unwrap().prev_track();
        });
    }
    {
        let app = app.clone();
        window.on_seek(move |seconds: f32| {
            app.lock().unwrap().seek(seconds as f64);
        });
    }
    {
        let app = app.clone();
        window.on_seek_absolute(move |ratio: f32| {
            let mut a = app.lock().unwrap();
            let ratio = (ratio as f64).clamp(0.0, 1.0);
            let st = a.player_state();
            if st.duration > 0.0 {
                let target = st.duration * ratio;
                a.seek(target - st.position);
            }
        });
    }

    // Volume slider
    {
        let app = app.clone();
        window.on_set_volume(move |v: f32| {
            let clamped = v.round().clamp(0.0, 100.0) as u8;
            let mut a = app.lock().unwrap();
            if a.config.volume != clamped {
                a.config.volume = clamped;
                let _ = a.player.set_volume(clamped);
                if let Err(e) = crate::config::save(&a.config) {
                    a.status = format!("config write failed: {e}");
                }
            }
        });
    }

    // Tab switch — keep the right focus inside `Library` so the Tracks/
    // Saved-Playlists list stays usable.
    {
        let app = app.clone();
        window.on_set_tab(move |i| {
            let mut a = app.lock().unwrap();
            a.focus = match i {
                0 => ListFocus::Results,
                1 => match a.focus {
                    ListFocus::YtLibrary | ListFocus::YtPlaylist => a.focus,
                    _ => ListFocus::YtLibrary,
                },
                2 => ListFocus::LocalFolder,
                _ => a.focus,
            };
        });
    }

    {
        let app = app.clone();
        window.on_cycle_loop(move || {
            app.lock().unwrap().cycle_loop();
        });
    }
    {
        let app = app.clone();
        window.on_toggle_shuffle(move || {
            app.lock().unwrap().toggle_shuffle();
        });
    }
    {
        let app = app.clone();
        window.on_toggle_captions(move || {
            app.lock().unwrap().toggle_captions();
        });
    }

    // Overlays
    {
        let weak = window.as_weak();
        window.on_toggle_nerd(move || {
            if let Some(w) = weak.upgrade() {
                w.set_show_nerd(!w.get_show_nerd());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_toggle_help(move || {
            if let Some(w) = weak.upgrade() {
                w.set_show_help(!w.get_show_help());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_toggle_params(move || {
            if let Some(w) = weak.upgrade() {
                w.set_show_params(!w.get_show_params());
            }
        });
    }

    // Params: sprite + caption lang
    {
        let app = app.clone();
        window.on_cycle_sprite(move |delta| {
            let mut a = app.lock().unwrap();
            a.params_row = 0;
            a.params_change(delta);
        });
    }
    {
        let app = app.clone();
        window.on_cycle_caption_lang(move |delta| {
            let mut a = app.lock().unwrap();
            a.params_row = 1;
            a.params_change(delta);
        });
    }

    // Clipboard
    {
        let app = app.clone();
        window.on_yank_url(move || {
            app.lock().unwrap().yank_selected_url();
        });
    }

    // Quit
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_quit(move || {
            app.lock().unwrap().should_quit = true;
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
    }
}

fn clamp(i: i32, len: usize) -> Option<usize> {
    if i < 0 || len == 0 {
        return None;
    }
    let i = i as usize;
    if i >= len {
        None
    } else {
        Some(i)
    }
}

fn push_state(
    window: &MainWindow,
    app: &App,
) {
    let current = app.current_track();
    let title = match current {
        Some(t) => format!("{} - {}", t.title, t.uploader),
        None => "ytmtui".into(),
    };
    window.set_current_title(title.into());

    // Lists
    window.set_results(rows(&app.results, current_id(app)));
    window.set_tracks(rows(app.active_tracks(), current_id(app)));
    window.set_local_tracks(rows(&app.local_folder, current_id(app)));
    window.set_library_rows(library_rows(app));

    // Counts / labels
    window.set_results_count(app.results.len() as i32);
    window.set_tracks_count(app.active_tracks().len() as i32);
    window.set_local_count(app.local_folder.len() as i32);
    window.set_library_count(app.saved_playlist_row_count() as i32);
    window.set_active_playlist_title(app.active_title().into());
    window.set_is_searching(app.searching);
    window.set_is_loading_playlist(app.yt_playlist_loading);
    window.set_is_scanning_local(app.local_folder_scanning);

    // Tab
    let tab = match app.focus {
        ListFocus::Results => 0,
        ListFocus::YtLibrary | ListFocus::YtPlaylist => 1,
        ListFocus::LocalFolder => 2,
    };
    window.set_active_tab(tab);

    // Selections
    window.set_results_selected(app.selected as i32);
    window.set_tracks_selected(app.yt_playlist_selected as i32);
    window.set_local_selected(app.local_folder_selected as i32);
    window.set_library_selected(app.library_selected as i32);

    // Now playing
    let st = app.player_state();
    window.set_has_current(current.is_some());
    window.set_is_paused(st.paused);
    let (title, uploader) = match current {
        Some(t) => (t.title.clone(), t.uploader.clone()),
        None => (String::new(), String::new()),
    };
    window.set_current_title(title.into());
    window.set_current_uploader(uploader.into());
    window.set_current_source_label(queue_source_label(app).into());
    window.set_position_str(fmt_secs(st.position).into());
    window.set_duration_str(fmt_secs(st.duration).into());
    let ratio = if st.duration > 0.0 {
        (st.position / st.duration).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    window.set_playback_ratio(ratio);

    // Captions
    window.set_show_captions(app.show_captions);
    let (status_label, text) = caption_strings(app);
    window.set_caption_status_label(status_label.into());
    window.set_caption_text(text.into());

    // Audio / status
    window.set_volume(app.config.volume as i32);
    window.set_loop_mode_label(app.config.loop_mode.label().into());
    window.set_shuffle_on(app.config.shuffle);
    window.set_device_name(
        app.output_device
            .as_ref()
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "(default audio)".into())
            .into(),
    );
    window.set_status_text(app.status.clone().into());

    // Nerd stats (cheap regardless of overlay visibility)
    let stats = app.stats();
    window.set_ytmtui_version(env!("CARGO_PKG_VERSION").into());
    window.set_mpv_version(app.mpv_version.clone().unwrap_or_else(|| "—".into()).into());
    window.set_ytdlp_version(
        app.ytdlp_version
            .clone()
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_cpu_summary(
        format!(
            "{:.1}%  (ui {:.1} + mpv {:.1})",
            stats.total_cpu(),
            stats.self_proc.cpu_percent,
            stats.mpv.cpu_percent
        )
        .into(),
    );
    window.set_ram_summary(
        format!(
            "{}  (ui {} + mpv {})",
            fmt_bytes(stats.total_rss()),
            fmt_bytes(stats.self_proc.rss_bytes),
            fmt_bytes(stats.mpv.rss_bytes),
        )
        .into(),
    );
    window.set_codec_label(st.audio_codec.clone().unwrap_or_else(|| "—".into()).into());
    window.set_bitrate_label(
        match st.audio_bitrate {
            Some(b) if b > 0.0 => format!("{:.0} kbps", b / 1000.0),
            _ => "—".into(),
        }
        .into(),
    );
    window.set_samplerate_label(
        st.samplerate
            .map(|r| format!("{} Hz", r))
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_channels_label(
        st.channels
            .map(|c| format!("{} ch", c))
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    window.set_device_detail(
        app.output_device
            .as_ref()
            .map(|d| format!("{}, {}", d.kind.label(), d.transport))
            .unwrap_or_else(|| "(generic audio)".into())
            .into(),
    );

    // Params overlay state
    let sprite = app.current_sprite();
    let registry = app.sprites();
    window.set_sprite_name(sprite.name.clone().into());
    window.set_sprite_preview(sprite.frame(0).to_string().into());
    window.set_sprite_index(registry.index_of(&sprite.id) as i32);
    window.set_sprite_total(registry.all().len() as i32);
    let lang_idx = CAPTION_LANGS
        .iter()
        .position(|l| *l == app.config.caption_lang)
        .unwrap_or(0);
    window.set_caption_lang(app.config.caption_lang.clone().into());
    window.set_caption_lang_index(lang_idx as i32);
    window.set_caption_lang_total(CAPTION_LANGS.len() as i32);
}

fn current_id(app: &App) -> Option<String> {
    app.current_track().map(|t| t.id.clone())
}

fn rows(tracks: &[Track], playing_id: Option<String>) -> ModelRc<TrackRow> {
    let vec: Vec<TrackRow> = tracks
        .iter()
        .map(|t| TrackRow {
            id: t.id.clone().into(),
            title: t.title.clone().into(),
            uploader: t.uploader.clone().into(),
            duration: t.duration_str().into(),
            platform_glyph: t.source_glyph().to_string().into(),
            depth: match t.local_depth {
                Some(d) => crate::local_scan::depth_marker(d).into(),
                None => SharedString::default(),
            },
            is_playing: playing_id.as_deref() == Some(t.id.as_str()),
        })
        .collect();
    ModelRc::new(VecModel::from(Rc::new(vec).as_ref().clone()))
}

fn library_rows(app: &App) -> ModelRc<LibraryRow> {
    let mut out: Vec<LibraryRow> = Vec::new();
    if app.unsaved_visible() {
        let total: u64 = app.playlist.iter().filter_map(|t| t.duration).sum();
        let dur = fmt_total_duration(total).unwrap_or_else(|| "--".into());
        out.push(LibraryRow {
            title: "Unsaved".into(),
            count_label: format!("({} tracks)", app.playlist.len()).into(),
            duration_label: dur.into(),
            favorite: false,
            glyph: "✎".into(),
            is_active: matches!(app.active_library, ActiveLibrary::Unsaved),
            is_unsaved: true,
        });
    }
    for (i, e) in app.library.entries.iter().enumerate() {
        out.push(library_row_from(e, i, app));
    }
    ModelRc::new(VecModel::from(out))
}

fn library_row_from(e: &PlaylistEntry, i: usize, app: &App) -> LibraryRow {
    let dur = e
        .total_duration
        .and_then(fmt_total_duration)
        .unwrap_or_else(|| "--".into());
    let glyph = match e.platform {
        Platform::Bilibili => "B",
        Platform::Local => "⌂",
        _ => "Y",
    };
    LibraryRow {
        title: e.title.clone().into(),
        count_label: format!("({} tracks)", e.track_count).into(),
        duration_label: dur.into(),
        favorite: e.favorite,
        glyph: glyph.into(),
        is_active: matches!(&app.active_library, ActiveLibrary::Saved(j) if *j == i),
        is_unsaved: false,
    }
}

fn queue_source_label(app: &App) -> String {
    match &app.queue_source {
        QueueSource::Results => "search results".into(),
        QueueSource::Unsaved => "Unsaved".into(),
        QueueSource::Saved { name } => name.clone(),
        QueueSource::LocalFolder => {
            let name = app.config.local_folder_label.as_deref().unwrap_or("folder");
            format!("⌂: {}", name)
        }
    }
}

fn caption_strings(app: &App) -> (String, String) {
    let label = match app.caption_status {
        CaptionStatus::Idle => "CC idle",
        CaptionStatus::Loading => "CC loading…",
        CaptionStatus::None => "CC none",
        CaptionStatus::Error => "CC error",
        CaptionStatus::Ready => "CC ready",
    };
    let text = if matches!(app.caption_status, CaptionStatus::Ready) {
        app.current_captions().join("\n")
    } else {
        String::new()
    };
    (label.into(), text)
}

fn fmt_secs(s: f64) -> String {
    let s = s.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Register a broad-coverage system font into Slint's runtime font
/// collection and return the family name to use as the default UI font.
///
/// Slint's femtovg backend says it auto-discovers system fonts via
/// fontique, but in practice the fallback chain doesn't reliably reach
/// CJK families on macOS — CJK glyphs render as tofu boxes even when a
/// Pan-CJK system font is present. Hardcoding a family name like
/// "PingFang SC" in the .slint file is also fragile, because the actual
/// CJK fonts installed differ from one Mac to the next (PingFang on
/// recent macOS, Hiragino Sans GB on older ones, STHeiti / Songti
/// elsewhere).
///
/// What we do: try each candidate path in priority order, read the
/// font's bytes, pull its real family name out of the `name` table
/// via ttf-parser, and push the bytes into the shared fontique
/// collection that the femtovg renderer queries. The caller then sets
/// the window's default font family to the name we returned, so the
/// UI uses a family fontique can actually resolve.
///
/// Per-OS candidate priority (all broad Pan-CJK fonts that ship by
/// default, no install step required):
///   macOS  : Hiragino Sans GB, Arial Unicode, STHeiti, Songti
///   Windows: Microsoft YaHei (SC), Yu Gothic (JP), Malgun Gothic (KR)
///   Linux  : Noto Sans CJK (covers all four), WenQuanYi, DejaVu
///
/// All candidates ship under permissive licences (Apple / Microsoft
/// system fonts, SIL Open Font License for Noto / WenQuanYi / DejaVu).
fn register_fallback_fonts() -> Option<String> {
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",         // newer macOS
        "/System/Library/Fonts/Hiragino Sans GB.ttc", // older macOS, ships Simplified Chinese
        "/Library/Fonts/Arial Unicode.ttf",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
    ];
    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\malgun.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates: &[&str] = &[
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/wqy-microhei/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ];

    let mut chosen_family: Option<String> = None;
    for path in candidates {
        let p = std::path::Path::new(path);
        if !p.exists() {
            continue;
        }
        let bytes = match std::fs::read(p) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("warning: could not read {}: {}", path, e);
                continue;
            }
        };
        // Pull the real family name out of the font's name-table
        // before we hand the bytes to fontique. Hardcoding "PingFang
        // SC" or "Noto Sans CJK SC" is fragile because:
        //   - macOS installs vary (some have PingFang, some don't),
        //   - the family name shown in Font Book isn't always the one
        //     stored in the name table.
        // Picking it from the file means we always set
        // default-font-family to something fontique can actually
        // resolve.
        let family_from_face = read_family_name(&bytes);
        i_slint_common::sharedfontique::get_collection().register_fonts(bytes.into(), None);
        if chosen_family.is_none() {
            chosen_family = family_from_face;
        }
    }

    if chosen_family.is_none() {
        eprintln!(
            "ytmtui-gui: no CJK-capable system font found. CJK / non-Latin \
             titles may render as empty boxes. Install Noto Sans CJK \
             (Debian/Ubuntu: `apt install fonts-noto-cjk`; Fedora: `dnf \
             install google-noto-sans-cjk-fonts`) and re-run."
        );
    }
    chosen_family
}

/// Return the first Latin-script family name we can find in a font's
/// `name` table. Tries the English typographic family (name id 16)
/// first, then the English family (name id 1), so TTCs with multiple
/// weight variants still resolve to a single useful family name like
/// "Hiragino Sans GB" rather than the weight-specific
/// "Hiragino Sans GB W3".
fn read_family_name(bytes: &[u8]) -> Option<String> {
    use i_slint_common::sharedfontique::ttf_parser;
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let pick = |target_id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .find(|n| n.name_id == target_id && n.is_unicode())
            .and_then(|n| n.to_string())
    };
    pick(ttf_parser::name_id::TYPOGRAPHIC_FAMILY).or_else(|| pick(ttf_parser::name_id::FAMILY))
}
