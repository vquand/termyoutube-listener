use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Fetch captions (manual if present, otherwise auto-generated) for a
/// YouTube track in `lang`. Returns an empty Vec if the track has no
/// captions in that language.
pub fn fetch(track_id: &str, lang: &str) -> Result<Vec<Cue>> {
    let stem = std::env::temp_dir().join(format!("ytmtui-cap-{}", track_id));
    purge_stale(&stem);

    let url = format!("https://www.youtube.com/watch?v={}", track_id);
    // yt-dlp's --sub-lang values are regex-matched with re.match (anchored
    // at the start only). Without the trailing `$`, "en" also matches
    // "en-zh", "en-de", etc. — and asking for ten translation tracks at
    // once trips YouTube's HTTP 429 rate limiter. Anchor the language so
    // we ask for exactly one track.
    let lang_arg = format!("{}$", lang);
    let output = Command::new("yt-dlp")
        .args([
            "--skip-download",
            "--write-subs",
            "--write-auto-subs",
            "--sub-lang",
            &lang_arg,
            "--sub-format",
            "vtt",
            "--no-warnings",
            "-o",
            &stem.display().to_string(),
            &url,
        ])
        .output()
        .context("failed to run yt-dlp for captions")?;

    // Look at the filesystem first: yt-dlp can exit non-zero when a
    // *secondary* subtitle variant fails (e.g. 429) even though the
    // primary .vtt landed fine. If we got the file, use it.
    let vtt_path = find_vtt(&stem)?;
    match vtt_path {
        Some(path) => {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let _ = fs::remove_file(&path);
            Ok(parse_vtt(&raw))
        }
        None => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                anyhow::bail!("yt-dlp captions failed: {}", stderr.trim());
            }
            Ok(Vec::new())
        }
    }
}

fn purge_stale(stem: &PathBuf) {
    let Some(parent) = stem.parent() else { return };
    let Some(prefix) = stem.file_name().and_then(|s| s.to_str()) else {
        return;
    };
    if let Ok(rd) = fs::read_dir(parent) {
        for entry in rd.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(prefix) && name.ends_with(".vtt") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

fn find_vtt(stem: &PathBuf) -> Result<Option<PathBuf>> {
    let dir = stem.parent().unwrap_or_else(|| std::path::Path::new("."));
    let prefix = stem.file_name().and_then(|s| s.to_str()).unwrap_or("");
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(prefix) && name.ends_with(".vtt") {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Lookup the cue text active at `pos` (seconds). Binary search on start time.
pub fn active_cue<'a>(cues: &'a [Cue], pos: f64) -> Option<&'a str> {
    if cues.is_empty() {
        return None;
    }
    // Find rightmost cue whose start <= pos.
    let idx = match cues.binary_search_by(|c| {
        c.start
            .partial_cmp(&pos)
            .unwrap_or(std::cmp::Ordering::Equal)
    }) {
        Ok(i) => i,
        Err(0) => return None,
        Err(i) => i - 1,
    };
    let c = &cues[idx];
    if pos <= c.end {
        Some(&c.text)
    } else {
        None
    }
}

fn parse_vtt(raw: &str) -> Vec<Cue> {
    let mut out = Vec::new();
    let mut lines = raw.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim_end_matches('\r');
        let Some((s, e)) = parse_timing(line) else {
            continue;
        };
        let mut text_lines = Vec::new();
        while let Some(next) = lines.peek() {
            let next = next.trim_end_matches('\r');
            if next.is_empty() {
                lines.next();
                break;
            }
            text_lines.push(strip_tags(next));
            lines.next();
        }
        let text = text_lines.join(" ").trim().to_string();
        if !text.is_empty() {
            out.push(Cue { start: s, end: e, text });
        }
    }
    out
}

fn parse_timing(line: &str) -> Option<(f64, f64)> {
    let arrow = line.find("-->")?;
    let lhs = line[..arrow].trim();
    let rhs_raw = line[arrow + 3..].trim();
    // strip any trailing settings (align:..., position:..., etc.)
    let rhs = rhs_raw.split_whitespace().next()?;
    Some((parse_ts(lhs)?, parse_ts(rhs)?))
}

fn parse_ts(s: &str) -> Option<f64> {
    // formats: HH:MM:SS.mmm or MM:SS.mmm
    let mut parts = s.split(':').collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    let secs_part = parts.pop()?;
    let (sec_int, ms) = match secs_part.split_once('.') {
        Some((a, b)) => (a.parse::<f64>().ok()?, b.parse::<f64>().ok()? / 10f64.powi(b.len() as i32)),
        None => (secs_part.parse::<f64>().ok()?, 0.0),
    };
    let mut total = sec_int + ms;
    if let Some(m) = parts.pop() {
        total += m.parse::<f64>().ok()? * 60.0;
    }
    if let Some(h) = parts.pop() {
        total += h.parse::<f64>().ok()? * 3600.0;
    }
    Some(total)
}

fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_tag = false;
    for ch in line.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_vtt() {
        let raw = "WEBVTT\n\n00:00:01.000 --> 00:00:03.500\nHello world\n\n00:00:04.000 --> 00:00:06.000\nSecond line\n";
        let cues = parse_vtt(raw);
        assert_eq!(cues.len(), 2);
        assert!((cues[0].start - 1.0).abs() < 1e-6);
        assert!((cues[0].end - 3.5).abs() < 1e-6);
        assert_eq!(cues[0].text, "Hello world");
    }

    #[test]
    fn strips_inline_timing_tags() {
        let raw = "WEBVTT\n\n00:00:01.000 --> 00:00:03.000 align:start position:0%\nHello<00:00:01.500><c> world</c>\n";
        let cues = parse_vtt(raw);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Hello world");
    }

    #[test]
    fn active_cue_lookup() {
        let cues = vec![
            Cue { start: 1.0, end: 3.0, text: "a".into() },
            Cue { start: 4.0, end: 6.0, text: "b".into() },
        ];
        assert_eq!(active_cue(&cues, 0.5), None);
        assert_eq!(active_cue(&cues, 2.0), Some("a"));
        assert_eq!(active_cue(&cues, 3.5), None);
        assert_eq!(active_cue(&cues, 5.0), Some("b"));
        assert_eq!(active_cue(&cues, 10.0), None);
    }
}
