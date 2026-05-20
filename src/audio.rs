use serde_json::Value;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Speaker,
    Headphones,
    Earbuds,
    Unknown,
}

impl DeviceKind {
    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::Speaker => "speaker",
            DeviceKind::Headphones => "headphones",
            DeviceKind::Earbuds => "earbuds",
            DeviceKind::Unknown => "audio",
        }
    }

    /// 8-char kaomoji whose side glyphs hint at the device shape.
    pub fn kaomoji(self) -> &'static str {
        match self {
            DeviceKind::Speaker => "<(˃ᴗ˂)>",
            DeviceKind::Headphones => "Ω(˃ᴗ˂)Ω",
            DeviceKind::Earbuds => "ɞ(˃ᴗ˂)ʚ",
            DeviceKind::Unknown => "?(˃ᴗ˂)?",
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutputDevice {
    pub name: String,
    pub kind: DeviceKind,
    pub bluetooth: bool,
    /// Simplified transport label: "built-in", "bluetooth", "usb",
    /// "displayport", "hdmi", or "audio".
    pub transport: String,
}

#[cfg(target_os = "macos")]
pub fn detect() -> Option<OutputDevice> {
    let out = std::process::Command::new("system_profiler")
        .args(["SPAudioDataType", "-json"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(&raw).ok()?;
    let items = v.get("SPAudioDataType")?.as_array()?;
    for top in items {
        if let Some(inner) = top.get("_items").and_then(|x| x.as_array()) {
            for d in inner {
                if d.get("coreaudio_default_audio_output_device")
                    .and_then(|v| v.as_str())
                    == Some("spaudio_yes")
                {
                    return device_from(d);
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "macos"))]
pub fn detect() -> Option<OutputDevice> {
    None
}

fn device_from(d: &Value) -> Option<OutputDevice> {
    let name = d.get("_name")?.as_str()?.to_string();
    let raw_transport = d
        .get("coreaudio_device_transport")
        .and_then(|s| s.as_str())
        .unwrap_or("");
    let transport = simplify_transport(raw_transport).to_string();
    let bluetooth = raw_transport == "coreaudio_device_type_bluetooth";
    let kind = classify(&name, raw_transport);
    Some(OutputDevice {
        name,
        kind,
        bluetooth,
        transport,
    })
}

fn simplify_transport(t: &str) -> &'static str {
    if let Some(s) = t.strip_prefix("coreaudio_device_type_") {
        match s {
            "builtin" => "built-in",
            "bluetooth" => "bluetooth",
            "displayport" => "displayport",
            "hdmi" => "hdmi",
            "usb" => "usb",
            "airplay" => "airplay",
            _ => "audio",
        }
    } else {
        "audio"
    }
}

fn classify(name: &str, transport: &str) -> DeviceKind {
    let n = name.to_lowercase();
    // Earbuds: AirPods (including Pro) and bud-style devices.
    if n.contains("airpods") || n.contains("buds") || n.contains("earbuds") {
        return DeviceKind::Earbuds;
    }
    // Headphones: well-known over/on-ear product lines.
    if n.contains("headphone")
        || n.contains("wh-")
        || n.contains("qc")
        || n.contains("bose")
        || n.contains("xm")
        || n.contains("studio")
        || n.contains("beats")
    {
        return DeviceKind::Headphones;
    }
    // Speakers: explicit "speaker" in name or hard-wired output ports.
    if n.contains("speaker")
        || transport == "coreaudio_device_type_builtin"
        || transport == "coreaudio_device_type_displayport"
        || transport == "coreaudio_device_type_hdmi"
    {
        return DeviceKind::Speaker;
    }
    // Bluetooth devices with cute custom names default to earbuds since
    // that is by far the most common BT audio device today.
    if transport == "coreaudio_device_type_bluetooth" {
        return DeviceKind::Earbuds;
    }
    DeviceKind::Unknown
}

pub fn spawn_poller(tx: Sender<Option<OutputDevice>>) {
    thread::spawn(move || loop {
        let dev = detect();
        if tx.send(dev).is_err() {
            break;
        }
        thread::sleep(Duration::from_secs(5));
    });
}

/// 8-step block icon mirroring volume level 0..=100.
pub fn volume_block(v: u8) -> &'static str {
    const BLOCKS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let idx = ((v as f64 / 100.0) * 7.0).round() as usize;
    BLOCKS[idx.min(7)]
}
