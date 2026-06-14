#[cfg(target_os = "linux")]
mod linux {
    use crate::app::App;
    use crate::config::LoopMode;
    use std::borrow::Cow;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use zbus::conn::Builder;
    use zbus::fdo::Properties;
    use zbus::names::InterfaceName;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::{ObjectPath, OwnedValue, Value};

    const BUS_NAME: &str = "org.mpris.MediaPlayer2.ytmtui";
    const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

    pub fn spawn(app: Arc<Mutex<App>>) {
        thread::spawn(move || {
            let result: zbus::Result<()> = zbus::block_on(async move {
                let root = RootInterface;
                let player = PlayerInterface { app: app.clone() };
                let connection = Builder::session()?
                    .name(BUS_NAME)?
                    .serve_at(OBJECT_PATH, root)?
                    .serve_at(OBJECT_PATH, player)?
                    .build()
                    .await?;
                let emitter = SignalEmitter::new(&connection, OBJECT_PATH)?;
                let interface = InterfaceName::try_from("org.mpris.MediaPlayer2.Player")?;

                loop {
                    thread::sleep(Duration::from_secs(1));
                    let changed = changed_properties(&app);
                    Properties::properties_changed(
                        &emitter,
                        interface.clone(),
                        changed,
                        Cow::Borrowed(&[]),
                    )
                    .await?;
                }
            });

            if let Err(e) = result {
                eprintln!("MPRIS unavailable: {e}");
            }
        });
    }

    struct RootInterface;

    #[zbus::interface(name = "org.mpris.MediaPlayer2")]
    impl RootInterface {
        fn raise(&self) {}

        fn quit(&self) {}

        #[zbus(property, name = "CanQuit")]
        fn can_quit(&self) -> bool {
            false
        }

        #[zbus(property, name = "Fullscreen")]
        fn fullscreen(&self) -> bool {
            false
        }

        #[zbus(property, name = "CanSetFullscreen")]
        fn can_set_fullscreen(&self) -> bool {
            false
        }

        #[zbus(property, name = "CanRaise")]
        fn can_raise(&self) -> bool {
            false
        }

        #[zbus(property, name = "HasTrackList")]
        fn has_track_list(&self) -> bool {
            false
        }

        #[zbus(property, name = "Identity")]
        fn identity(&self) -> &str {
            "ytmtui"
        }

        #[zbus(property, name = "DesktopEntry")]
        fn desktop_entry(&self) -> &str {
            "ytmtui-gui"
        }

        #[zbus(property, name = "SupportedUriSchemes")]
        fn supported_uri_schemes(&self) -> Vec<&str> {
            vec!["file", "http", "https"]
        }

        #[zbus(property, name = "SupportedMimeTypes")]
        fn supported_mime_types(&self) -> Vec<&str> {
            Vec::new()
        }
    }

    struct PlayerInterface {
        app: Arc<Mutex<App>>,
    }

    #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
    impl PlayerInterface {
        #[zbus(name = "Next")]
        fn next(&self) {
            self.with_app(|app| app.next_track());
        }

        #[zbus(name = "Previous")]
        fn previous(&self) {
            self.with_app(|app| app.prev_track());
        }

        #[zbus(name = "Pause")]
        fn pause(&self) {
            self.with_app(|app| app.pause());
        }

        #[zbus(name = "PlayPause")]
        fn play_pause(&self) {
            self.with_app(|app| app.toggle_pause());
        }

        #[zbus(name = "Stop")]
        fn stop(&self) {
            self.with_app(|app| app.pause());
        }

        #[zbus(name = "Play")]
        fn play(&self) {
            self.with_app(|app| app.play());
        }

        #[zbus(name = "Seek")]
        fn seek(&self, offset: i64) {
            self.with_app(|app| app.seek(offset as f64 / 1_000_000.0));
        }

        #[zbus(name = "SetPosition")]
        fn set_position(&self, _track_id: ObjectPath<'_>, position: i64) {
            self.with_app(|app| {
                let state = app.player_state();
                let target = (position as f64 / 1_000_000.0).max(0.0);
                app.seek(target - state.position);
            });
        }

        #[zbus(name = "OpenUri")]
        fn open_uri(&self, _uri: &str) {}

        #[zbus(property, name = "PlaybackStatus")]
        fn playback_status(&self) -> String {
            self.read_app(playback_status_from)
        }

        #[zbus(property, name = "LoopStatus")]
        fn loop_status(&self) -> String {
            self.read_app(loop_status_from)
        }

        #[zbus(property, name = "LoopStatus")]
        fn set_loop_status(&self, value: &str) {
            self.with_app(|app| {
                app.config.loop_mode = match value {
                    "Track" => LoopMode::One,
                    "Playlist" => LoopMode::All,
                    _ => LoopMode::Off,
                };
                app.save_config();
            });
        }

        #[zbus(property, name = "Rate")]
        fn rate(&self) -> f64 {
            1.0
        }

        #[zbus(property, name = "Rate")]
        fn set_rate(&self, _value: f64) {}

        #[zbus(property, name = "Shuffle")]
        fn shuffle(&self) -> bool {
            self.read_app(|app| app.config.shuffle)
        }

        #[zbus(property, name = "Shuffle")]
        fn set_shuffle(&self, value: bool) {
            self.with_app(|app| {
                app.config.shuffle = value;
                app.save_config();
            });
        }

        #[zbus(property, name = "Metadata")]
        fn metadata(&self) -> HashMap<String, OwnedValue> {
            self.read_app(metadata_from)
        }

        #[zbus(property, name = "Volume")]
        fn volume(&self) -> f64 {
            self.read_app(|app| app.config.volume as f64 / 100.0)
        }

        #[zbus(property, name = "Volume")]
        fn set_volume(&self, value: f64) {
            self.with_app(|app| {
                let volume = (value.clamp(0.0, 1.0) * 100.0).round() as u8;
                app.set_volume(volume);
            });
        }

        #[zbus(property, name = "Position")]
        fn position(&self) -> i64 {
            self.read_app(|app| seconds_to_micros(app.player_state().position))
        }

        #[zbus(property, name = "MinimumRate")]
        fn minimum_rate(&self) -> f64 {
            1.0
        }

        #[zbus(property, name = "MaximumRate")]
        fn maximum_rate(&self) -> f64 {
            1.0
        }

        #[zbus(property, name = "CanGoNext")]
        fn can_go_next(&self) -> bool {
            self.read_app(|app| app.current_track().is_some())
        }

        #[zbus(property, name = "CanGoPrevious")]
        fn can_go_previous(&self) -> bool {
            self.read_app(|app| app.current_track().is_some())
        }

        #[zbus(property, name = "CanPlay")]
        fn can_play(&self) -> bool {
            self.read_app(|app| app.current_track().is_some())
        }

        #[zbus(property, name = "CanPause")]
        fn can_pause(&self) -> bool {
            self.read_app(|app| app.current_track().is_some())
        }

        #[zbus(property, name = "CanSeek")]
        fn can_seek(&self) -> bool {
            self.read_app(|app| app.player_state().duration > 0.0)
        }

        #[zbus(property, name = "CanControl")]
        fn can_control(&self) -> bool {
            true
        }
    }

    impl PlayerInterface {
        fn with_app(&self, f: impl FnOnce(&mut App)) {
            if let Ok(mut app) = self.app.lock() {
                f(&mut app);
            }
        }

        fn read_app<T>(&self, f: impl FnOnce(&App) -> T) -> T {
            let app = self.app.lock().unwrap();
            f(&app)
        }
    }

    fn metadata_from(app: &App) -> HashMap<String, OwnedValue> {
        let mut metadata = HashMap::new();
        let track_path = app
            .current_track()
            .map(|track| track_path(&track.id))
            .unwrap_or_else(|| "/org/mpris/MediaPlayer2/TrackList/NoTrack".to_string());
        insert_value(
            &mut metadata,
            "mpris:trackid",
            ObjectPath::try_from(track_path).unwrap(),
        );

        if let Some(track) = app.current_track() {
            insert_value(&mut metadata, "xesam:title", track.title.clone());
            if !track.uploader.is_empty() {
                insert_value(&mut metadata, "xesam:artist", vec![track.uploader.clone()]);
            }
            insert_value(&mut metadata, "xesam:url", track.url());
            if let Some(duration) = track.duration {
                insert_value(&mut metadata, "mpris:length", (duration as i64) * 1_000_000);
            } else {
                let state_duration = app.player_state().duration;
                if state_duration > 0.0 {
                    insert_value(
                        &mut metadata,
                        "mpris:length",
                        seconds_to_micros(state_duration),
                    );
                }
            }
        }

        metadata
    }

    fn changed_properties(app: &Arc<Mutex<App>>) -> HashMap<&'static str, Value<'static>> {
        let app = app.lock().unwrap();
        let mut changed = HashMap::new();
        changed.insert("PlaybackStatus", playback_status_from(&app).into());
        changed.insert("LoopStatus", loop_status_from(&app).into());
        changed.insert("Shuffle", app.config.shuffle.into());
        changed.insert("Metadata", metadata_from(&app).into());
        changed.insert("Volume", (app.config.volume as f64 / 100.0).into());
        changed.insert(
            "Position",
            seconds_to_micros(app.player_state().position).into(),
        );
        changed.insert("CanGoNext", app.current_track().is_some().into());
        changed.insert("CanGoPrevious", app.current_track().is_some().into());
        changed.insert("CanPlay", app.current_track().is_some().into());
        changed.insert("CanPause", app.current_track().is_some().into());
        changed.insert("CanSeek", (app.player_state().duration > 0.0).into());
        changed
    }

    fn playback_status_from(app: &App) -> String {
        let state = app.player_state();
        if app.current_track().is_none() || state.idle {
            "Stopped"
        } else if state.paused {
            "Paused"
        } else {
            "Playing"
        }
        .to_string()
    }

    fn loop_status_from(app: &App) -> String {
        match app.config.loop_mode {
            LoopMode::Off => "None",
            LoopMode::One => "Track",
            LoopMode::All => "Playlist",
        }
        .to_string()
    }

    fn insert_value<T>(metadata: &mut HashMap<String, OwnedValue>, key: &str, value: T)
    where
        T: Into<Value<'static>>,
    {
        let value = value.into().try_to_owned().expect("valid MPRIS metadata");
        metadata.insert(key.to_string(), value);
    }

    fn track_path(id: &str) -> String {
        let mut out = String::from("/org/mpris/MediaPlayer2/TrackList/");
        for byte in id.bytes() {
            match byte {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' => out.push(byte as char),
                _ => out.push('_'),
            }
        }
        out
    }

    fn seconds_to_micros(seconds: f64) -> i64 {
        (seconds.max(0.0) * 1_000_000.0).round() as i64
    }
}

#[cfg(target_os = "linux")]
pub use linux::spawn;

#[cfg(not(target_os = "linux"))]
pub fn spawn<T>(_app: T) {}
