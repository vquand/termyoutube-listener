use ratatui::style::Color;
use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

include!(concat!(env!("OUT_DIR"), "/builtin_sprites.rs"));

/// A two-frame (or more) progress-bar cursor. `trail_left` is repeated behind
/// the sprite (already-played territory) and `trail_right` ahead of it.
/// Trails may be multi-character patterns; renderer keeps them flush against
/// the sprite so the pattern visually "scrolls" as the cursor moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimateOn {
    /// Frame advances on a fixed 250 ms timer.
    Tick,
    /// Frame advances only when the cursor moves to a new cell on the bar.
    Move,
}

#[derive(Debug, Clone)]
pub struct Sprite {
    pub id: String,
    pub name: String,
    pub frames: Vec<String>,
    pub trail_left: String,
    pub trail_right: String,
    pub accent: Color,
    pub order: i32,
    pub animate_on: AnimateOn,
}

impl Sprite {
    pub fn frame(&self, idx: usize) -> &str {
        if self.frames.is_empty() {
            ""
        } else {
            self.frames[idx % self.frames.len()].as_str()
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len().max(1)
    }
}

#[derive(Debug, Deserialize)]
struct SpriteFile {
    name: Option<String>,
    frames: Vec<String>,
    trail_left: String,
    trail_right: String,
    accent: String,
    #[serde(default = "default_order")]
    order: i32,
    #[serde(default)]
    animate_on: Option<String>,
}

fn default_order() -> i32 {
    1000
}

impl SpriteFile {
    fn into_sprite(self, id: String) -> Sprite {
        let animate_on = match self.animate_on.as_deref() {
            Some("move") => AnimateOn::Move,
            _ => AnimateOn::Tick,
        };
        Sprite {
            name: self.name.unwrap_or_else(|| id.clone()),
            id,
            frames: self.frames,
            trail_left: self.trail_left,
            trail_right: self.trail_right,
            accent: color_from_str(&self.accent),
            order: self.order,
            animate_on,
        }
    }
}

fn color_from_str(s: &str) -> Color {
    match s.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" | "darkgray" | "dark_gray" | "darkgrey" => Color::DarkGray,
        "lightred" | "light_red" => Color::LightRed,
        "lightgreen" | "light_green" => Color::LightGreen,
        "lightyellow" | "light_yellow" => Color::LightYellow,
        "lightblue" | "light_blue" => Color::LightBlue,
        "lightmagenta" | "light_magenta" => Color::LightMagenta,
        "lightcyan" | "light_cyan" => Color::LightCyan,
        _ => Color::Reset,
    }
}

pub struct Registry {
    sprites: Vec<Sprite>,
}

impl Registry {
    pub fn load() -> Self {
        let mut sprites: Vec<Sprite> = Vec::new();
        for (id, raw) in BUILTIN_SPRITES {
            if let Ok(sf) = serde_json::from_str::<SpriteFile>(raw) {
                sprites.push(sf.into_sprite((*id).to_string()));
            }
        }
        if let Some(dir) = user_sprites_dir() {
            if let Ok(rd) = fs::read_dir(&dir) {
                for entry in rd.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("json") {
                        continue;
                    }
                    let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    let id = id.to_string();
                    let Ok(raw) = fs::read_to_string(&path) else { continue };
                    let Ok(sf) = serde_json::from_str::<SpriteFile>(&raw) else {
                        continue;
                    };
                    let sprite = sf.into_sprite(id);
                    if let Some(pos) = sprites.iter().position(|s| s.id == sprite.id) {
                        sprites[pos] = sprite;
                    } else {
                        sprites.push(sprite);
                    }
                }
            }
        }
        sprites.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
        if sprites.is_empty() {
            sprites.push(fallback_sprite());
        }
        Self { sprites }
    }

    pub fn get(&self, id: &str) -> &Sprite {
        self.sprites
            .iter()
            .find(|s| s.id == id)
            .unwrap_or(&self.sprites[0])
    }

    pub fn next_id(&self, current: &str) -> String {
        let i = self.index_of(current);
        self.sprites[(i + 1) % self.sprites.len()].id.clone()
    }

    pub fn prev_id(&self, current: &str) -> String {
        let i = self.index_of(current);
        self.sprites[(i + self.sprites.len() - 1) % self.sprites.len()]
            .id
            .clone()
    }

    pub fn all(&self) -> &[Sprite] {
        &self.sprites
    }

    pub fn index_of(&self, id: &str) -> usize {
        self.sprites.iter().position(|s| s.id == id).unwrap_or(0)
    }
}

fn user_sprites_dir() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".config"),
    };
    Some(base.join("ytmtui").join("sprites"))
}

fn fallback_sprite() -> Sprite {
    Sprite {
        id: "fallback".into(),
        name: "fallback".into(),
        frames: vec!["[ ]".into()],
        trail_left: "=".into(),
        trail_right: " ".into(),
        accent: Color::Cyan,
        order: 0,
        animate_on: AnimateOn::Tick,
    }
}
