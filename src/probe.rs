use std::path::Path;
use std::process::Command;

/// One-shot duration probe via `ffprobe` (ships with ffmpeg, which mpv
/// already depends on). Returns `None` if ffprobe is missing, the file
/// cannot be opened, or the output is unparseable. Whole seconds only.
pub fn duration(path: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let secs: f64 = s.trim().parse().ok()?;
    if !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    Some(secs.round() as u64)
}
