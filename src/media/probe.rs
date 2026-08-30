//! Media probing (design §5, §21): support is determined by asking ffprobe
//! what a file actually contains, not by trusting its extension.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::{ffprobe_bin, tail_of};

/// Metadata fields the application surfaces and tries to preserve (design §15).
/// Keys are ffmpeg tag names, lowercased.
pub const METADATA_FIELDS: &[(&str, &str)] = &[
    ("title", "Title"),
    ("artist", "Artist"),
    ("album", "Album"),
    ("album_artist", "Album Artist"),
    ("date", "Date"),
    ("genre", "Genre"),
    ("track", "Track"),
    ("disc", "Disc"),
    ("comment", "Comment"),
    ("copyright", "Copyright"),
    ("composer", "Composer"),
    ("lyrics", "Lyrics"),
];

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub duration: f64,
    /// ffmpeg's container name, e.g. `ogg` or `mov,mp4,m4a,3gp,3g2,mj2`.
    pub format_name: String,
    /// Codec of the first audio stream, e.g. `opus`. Shown as "format".
    pub audio_codec: String,
    pub bit_rate: Option<u64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// A cover-art stream (a video stream marked `attached_pic`).
    pub has_cover_art: bool,
    pub chapter_count: usize,
    /// Container-level tags, keys lowercased.
    pub tags: BTreeMap<String, String>,
    /// Tags carried on the audio stream itself.
    pub stream_tags: BTreeMap<String, String>,
}

impl MediaInfo {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string())
    }

    /// A tag looked up case-insensitively across container and stream tags.
    pub fn tag(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        self.tags
            .get(&key)
            .or_else(|| self.stream_tags.get(&key))
            .map(String::as_str)
    }

    /// Every tag that matters for preservation checks.
    pub fn all_tags(&self) -> BTreeMap<String, String> {
        let mut merged = self.stream_tags.clone();
        merged.extend(self.tags.clone());
        merged
    }
}

// ---- ffprobe JSON shapes ------------------------------------------------

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    format: Option<ProbeFormat>,
    #[serde(default)]
    chapters: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    #[serde(default)]
    codec_type: String,
    #[serde(default)]
    codec_name: String,
    #[serde(default)]
    sample_rate: Option<String>,
    #[serde(default)]
    channels: Option<u32>,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    disposition: BTreeMap<String, i64>,
    #[serde(default)]
    tags: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    #[serde(default)]
    format_name: String,
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: BTreeMap<String, serde_json::Value>,
}

fn normalise_tags(raw: &BTreeMap<String, serde_json::Value>) -> BTreeMap<String, String> {
    raw.iter()
        .filter_map(|(k, v)| {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => return None,
            };
            Some((k.to_ascii_lowercase(), value))
        })
        .collect()
}

/// Probe a file. Returns `Ok(None)` when the file is readable but holds no
/// audio stream, and `Err` when ffprobe could not read it at all.
pub fn probe(path: &Path) -> Result<Option<MediaInfo>> {
    let output = Command::new(ffprobe_bin())
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("running ffprobe on {}", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "ffprobe failed for {}: {}",
            path.display(),
            tail_of(&stderr, 3)
        );
    }

    let parsed: ProbeOutput = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing ffprobe output for {}", path.display()))?;

    let Some(audio) = parsed.streams.iter().find(|s| s.codec_type == "audio") else {
        return Ok(None);
    };

    let format = parsed.format.as_ref();
    let duration = format
        .and_then(|f| f.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| {
            audio
                .duration
                .as_deref()
                .and_then(|d| d.parse::<f64>().ok())
        })
        .unwrap_or(0.0);

    let bit_rate = format
        .and_then(|f| f.bit_rate.as_deref())
        .and_then(|b| b.parse::<u64>().ok())
        .or_else(|| {
            audio
                .bit_rate
                .as_deref()
                .and_then(|b| b.parse::<u64>().ok())
        });

    let has_cover_art = parsed.streams.iter().any(|s| {
        s.codec_type == "video" && s.disposition.get("attached_pic").copied().unwrap_or(0) == 1
    });

    Ok(Some(MediaInfo {
        path: path.to_path_buf(),
        duration: if duration.is_finite() && duration > 0.0 {
            duration
        } else {
            0.0
        },
        format_name: format.map(|f| f.format_name.clone()).unwrap_or_default(),
        audio_codec: audio.codec_name.clone(),
        bit_rate,
        sample_rate: audio.sample_rate.as_deref().and_then(|r| r.parse().ok()),
        channels: audio.channels,
        has_cover_art,
        chapter_count: parsed.chapters.len(),
        tags: format.map(|f| normalise_tags(&f.tags)).unwrap_or_default(),
        stream_tags: normalise_tags(&audio.tags),
    }))
}

/// A candidate file that could not be offered for editing, and why.
#[derive(Debug, Clone)]
pub struct SkippedFile {
    pub name: String,
    pub reason: String,
}

/// The outcome of scanning a folder: files ready to edit, plus anything that
/// looked like it might be audio but could not be probed successfully.
#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub files: Vec<MediaInfo>,
    pub skipped: Vec<SkippedFile>,
}

/// Scan a folder for files that probe as audio, sorted by name.
///
/// Extensions are used only to skip obvious non-media files cheaply; anything
/// plausible is confirmed by probing.
pub fn scan_folder(folder: &Path) -> Result<Vec<MediaInfo>> {
    Ok(scan_folder_detailed(folder)?.files)
}

/// As [`scan_folder`], but also reports candidates that could not be read, so
/// a folder-wide run can account for every file rather than silently dropping
/// the ones it could not probe (design §17).
pub fn scan_folder_detailed(folder: &Path) -> Result<ScanResult> {
    let entries = std::fs::read_dir(folder)
        .with_context(|| format!("reading folder {}", folder.display()))?;

    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("reading folder {}", folder.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('.'))
        {
            continue;
        }
        paths.push(path);
    }
    paths.sort();

    let mut result = ScanResult::default();
    for path in paths {
        if !worth_probing(&path) {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        match probe(&path) {
            Ok(Some(info)) if info.duration > 0.0 => result.files.push(info),
            Ok(Some(_)) => result.skipped.push(SkippedFile {
                name,
                reason: "no measurable duration".to_string(),
            }),
            Ok(None) => result.skipped.push(SkippedFile {
                name,
                reason: "no audio stream".to_string(),
            }),
            Err(err) => result.skipped.push(SkippedFile {
                name,
                reason: format!("{err:#}"),
            }),
        }
    }
    Ok(result)
}

/// Cheap pre-filter. Extensionless files are still probed, so unusual names
/// are not excluded; only known non-audio extensions are skipped outright.
fn worth_probing(path: &Path) -> bool {
    const SKIP: &[&str] = &[
        "txt", "md", "json", "toml", "yaml", "yml", "png", "jpg", "jpeg", "gif", "webp", "pdf",
        "zip", "gz", "tar", "xz", "rs", "py", "sh", "html", "css", "js", "log", "csv", "svg",
        "exe", "dll", "so", "o", "a", "bin", "lock",
    ];
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => !SKIP.contains(&ext.to_ascii_lowercase().as_str()),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_obvious_non_media() {
        assert!(!worth_probing(Path::new("notes.md")));
        assert!(!worth_probing(Path::new("cover.JPG")));
        assert!(worth_probing(Path::new("interview.opus")));
        assert!(worth_probing(Path::new("recording.m4a")));
        assert!(worth_probing(Path::new("no-extension")));
    }

    #[test]
    fn tags_are_looked_up_case_insensitively() {
        let mut tags = BTreeMap::new();
        tags.insert("title".to_string(), "Hello".to_string());
        let info = MediaInfo {
            path: PathBuf::from("/tmp/a.opus"),
            duration: 1.0,
            format_name: "ogg".into(),
            audio_codec: "opus".into(),
            bit_rate: None,
            sample_rate: None,
            channels: None,
            has_cover_art: false,
            chapter_count: 0,
            tags,
            stream_tags: BTreeMap::new(),
        };
        assert_eq!(info.tag("TITLE"), Some("Hello"));
        assert_eq!(info.tag("artist"), None);
    }
}
