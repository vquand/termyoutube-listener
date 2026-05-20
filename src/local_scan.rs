use crate::ytdlp::Track;
use std::path::Path;

const PLAYABLE: &[&str] = &[
    // audio
    "mp3", "m4a", "aac", "opus", "ogg", "flac", "wav", "wma", "alac",
    // video (mpv plays the audio track)
    "mp4", "webm", "mkv", "avi", "mov", "m4v", "ts", "flv",
];

pub const MAX_DEPTH: u8 = 4;

/// Walks `root` up to MAX_DEPTH levels deep and collects playable files
/// into Tracks. Hidden entries (starting with `.`) and symlinks pointing
/// outside `root` are skipped to avoid surprises.
pub fn scan_folder(root: &Path) -> Vec<Track> {
    let mut tracks = Vec::new();
    walk(root, 0, &mut tracks);
    tracks.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    tracks
}

fn walk(dir: &Path, depth: u8, out: &mut Vec<Track>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let name_os = entry.file_name();
        let name = name_os.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if file_type.is_dir() {
            walk(&path, depth + 1, out);
        } else if file_type.is_file() && is_playable(&path) {
            let p = path.to_string_lossy().to_string();
            let title = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| p.clone());
            out.push(Track {
                id: p.clone(),
                title,
                uploader: String::new(),
                duration: None,
                source: Some(p),
                local_depth: Some(depth),
            });
        }
    }
}

fn is_playable(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| PLAYABLE.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// `R:` for the root level, `R/` for one level below, `R//` for two, etc.
/// Used in list rows so the user can see how deep a scanned file sits
/// without us spelling out every intermediate folder name.
pub fn depth_marker(depth: u8) -> String {
    if depth == 0 {
        "R:".to_string()
    } else {
        format!("R{}", "/".repeat(depth as usize))
    }
}
