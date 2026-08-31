//! Folder scanning (design §5, §21): finding candidate files and probing
//! each one to decide whether it is usable audio.

use std::path::{Path, PathBuf};

use anyhow::Context;

use super::probe::{probe, MediaInfo};

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
pub fn scan_folder(folder: &Path) -> anyhow::Result<Vec<MediaInfo>> {
    Ok(scan_folder_detailed(folder)?.files)
}

/// As [`scan_folder`], but also reports candidates that could not be read, so
/// a folder-wide run can account for every file rather than silently dropping
/// the ones it could not probe (design §17).
pub fn scan_folder_detailed(folder: &Path) -> anyhow::Result<ScanResult> {
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
}
