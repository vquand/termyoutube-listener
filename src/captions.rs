use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Cue {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct CaptionTrack {
    pub cues: Vec<Cue>,
}

#[derive(Debug, Clone, Default)]
pub struct FetchOptions {
    pub cookies_from_browser: Option<String>,
    pub cookies: Option<String>,
}

/// Fetch up to two caption tracks for a YouTube track: the original caption
/// track when yt-dlp exposes one, and the first configured preferred language
/// that differs from that original.
pub fn fetch(
    track_id: &str,
    preferred_langs: &[String],
    options: &FetchOptions,
) -> Result<Vec<CaptionTrack>> {
    let available = available_langs(track_id, options)?;
    let wanted = select_langs(&available, preferred_langs);
    let mut out = Vec::new();
    let mut last_err = None;
    for lang in wanted.into_iter().take(2) {
        match fetch_lang(track_id, &lang, options) {
            Ok(cues) if !cues.is_empty() => out.push(CaptionTrack { cues }),
            Ok(_) => {}
            Err(err) => last_err = Some(err),
        }
    }

    if out.is_empty() {
        if let Some(err) = last_err {
            return Err(err);
        }
    }
    Ok(out)
}

fn select_langs(available: &AvailableLangs, preferred_langs: &[String]) -> Vec<String> {
    let mut wanted = Vec::new();
    if let Some(original) = available.original_lang.clone() {
        wanted.push(original);
    }

    let mut preferred: Vec<&str> = preferred_langs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if preferred.is_empty() {
        preferred.push("en");
    }

    for lang in preferred {
        if available.has(lang)
            && wanted
                .iter()
                .all(|wanted_lang| canonical_lang(wanted_lang) != canonical_lang(lang))
        {
            wanted.push(lang.to_string());
            break;
        }
    }
    wanted.truncate(2);
    wanted
}

#[derive(Debug)]
struct AvailableLangs {
    original_lang: Option<String>,
    langs: HashSet<String>,
}

impl AvailableLangs {
    fn has(&self, lang: &str) -> bool {
        self.langs.contains(lang)
    }
}

fn available_langs(track_id: &str, options: &FetchOptions) -> Result<AvailableLangs> {
    let url = format!("https://www.youtube.com/watch?v={}", track_id);
    let mut cmd = Command::new("yt-dlp");
    apply_ytdlp_options(&mut cmd, options);
    let output = cmd
        .args([
            "--skip-download",
            "--dump-single-json",
            "--no-warnings",
            &url,
        ])
        .output()
        .context("failed to run yt-dlp for caption metadata")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("yt-dlp caption metadata failed: {}", stderr.trim());
    }

    let info: Value = serde_json::from_slice(&output.stdout).context("parse yt-dlp metadata")?;
    let subtitles = subtitle_lang_keys(info.get("subtitles"));
    let auto = subtitle_lang_keys(info.get("automatic_captions"));
    let original_lang = auto
        .iter()
        .find(|lang| lang.ends_with("-orig"))
        .cloned()
        .or_else(|| {
            info.get("language")
                .and_then(Value::as_str)
                .filter(|lang| subtitles.iter().chain(auto.iter()).any(|l| l == *lang))
                .map(str::to_string)
        })
        .or_else(|| {
            info.get("title")
                .and_then(Value::as_str)
                .and_then(infer_lang_from_text)
                .filter(|lang| subtitles.iter().chain(auto.iter()).any(|l| l == lang))
                .map(str::to_string)
        })
        .or_else(|| {
            if subtitles.len() == 1 {
                subtitles.first().cloned()
            } else {
                None
            }
        });
    let langs = subtitles.into_iter().chain(auto).collect();

    Ok(AvailableLangs {
        original_lang,
        langs,
    })
}

fn subtitle_lang_keys(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_object)
        .map(|obj| {
            obj.iter()
                .filter(|(_, tracks)| has_vtt_track(tracks))
                .map(|(lang, _)| lang.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn has_vtt_track(value: &Value) -> bool {
    value
        .as_array()
        .map(|tracks| {
            tracks
                .iter()
                .any(|track| track.get("ext").and_then(Value::as_str) == Some("vtt"))
        })
        .unwrap_or(false)
}

fn infer_lang_from_text(text: &str) -> Option<&'static str> {
    if text.chars().any(|ch| matches!(ch, 'À'..='ỹ')) {
        return Some("vi");
    }
    if text
        .chars()
        .any(|ch| ('\u{ac00}'..='\u{d7af}').contains(&ch))
    {
        return Some("ko");
    }
    if text
        .chars()
        .any(|ch| ('\u{3040}'..='\u{30ff}').contains(&ch))
    {
        return Some("ja");
    }
    if text
        .chars()
        .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch))
    {
        return Some("zh");
    }
    None
}

fn canonical_lang(lang: &str) -> &str {
    lang.strip_suffix("-orig").unwrap_or(lang)
}

fn fetch_lang(track_id: &str, lang: &str, options: &FetchOptions) -> Result<Vec<Cue>> {
    let safe_lang = lang
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>();
    let stem = std::env::temp_dir().join(format!("ytmtui-cap-{}-{}", track_id, safe_lang));
    purge_stale(&stem);

    let url = format!("https://www.youtube.com/watch?v={}", track_id);
    // yt-dlp's --sub-lang values are regex-matched with re.match (anchored
    // at the start only). Without the trailing `$`, "en" also matches
    // "en-zh", "en-de", etc. — and asking for ten translation tracks at
    // once trips YouTube's HTTP 429 rate limiter. Anchor the language so
    // we ask for exactly one track.
    let lang_arg = format!("{}$", lang);
    let mut cmd = Command::new("yt-dlp");
    apply_ytdlp_options(&mut cmd, options);
    let output = cmd
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
            let raw =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
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

fn apply_ytdlp_options(cmd: &mut Command, options: &FetchOptions) {
    if let Some(browser) = non_empty(options.cookies_from_browser.as_deref()) {
        cmd.args(["--cookies-from-browser", browser]);
    }
    if let Some(path) = non_empty(options.cookies.as_deref()) {
        cmd.args(["--cookies", path]);
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
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
            out.push(Cue {
                start: s,
                end: e,
                text,
            });
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
        Some((a, b)) => (
            a.parse::<f64>().ok()?,
            b.parse::<f64>().ok()? / 10f64.powi(b.len() as i32),
        ),
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
            Cue {
                start: 1.0,
                end: 3.0,
                text: "a".into(),
            },
            Cue {
                start: 4.0,
                end: 6.0,
                text: "b".into(),
            },
        ];
        assert_eq!(active_cue(&cues, 0.5), None);
        assert_eq!(active_cue(&cues, 2.0), Some("a"));
        assert_eq!(active_cue(&cues, 3.5), None);
        assert_eq!(active_cue(&cues, 5.0), Some("b"));
        assert_eq!(active_cue(&cues, 10.0), None);
    }

    #[test]
    fn selects_original_plus_preferred_translation() {
        let available = AvailableLangs {
            original_lang: Some("zh-Hans-orig".into()),
            langs: ["zh-Hans-orig", "en"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        };
        assert_eq!(
            select_langs(&available, &["en".into()]),
            vec!["zh-Hans-orig".to_string(), "en".to_string()]
        );
    }

    #[test]
    fn does_not_duplicate_original_base_language() {
        let available = AvailableLangs {
            original_lang: Some("vi-orig".into()),
            langs: ["vi-orig", "vi", "en"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        };
        assert_eq!(select_langs(&available, &["vi".into()]), vec!["vi-orig"]);
    }

    #[test]
    fn falls_back_to_preferred_when_original_is_unknown() {
        let available = AvailableLangs {
            original_lang: None,
            langs: ["en"].into_iter().map(str::to_string).collect(),
        };
        assert_eq!(select_langs(&available, &["en".into()]), vec!["en"]);
    }

    #[test]
    fn tries_later_preferred_languages() {
        let available = AvailableLangs {
            original_lang: Some("vi-orig".into()),
            langs: ["vi-orig", "en"].into_iter().map(str::to_string).collect(),
        };
        assert_eq!(
            select_langs(&available, &["vi".into(), "en".into()]),
            vec!["vi-orig".to_string(), "en".to_string()]
        );
    }

    #[test]
    fn infers_vietnamese_from_title_text() {
        assert_eq!(
            infer_lang_from_text("Đen - một triệu like ft. Thành Đồng"),
            Some("vi")
        );
    }

    #[test]
    fn ignores_subtitle_entries_without_vtt_tracks() {
        let raw = serde_json::json!({
            "vi": [{ "ext": "vtt" }],
            "live_chat": [{ "ext": "json" }]
        });
        assert_eq!(subtitle_lang_keys(Some(&raw)), vec!["vi"]);
    }
}
