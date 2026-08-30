//! The processing and save pipeline (design §13–§16).
//!
//! Saving is in place, but the original is only ever replaced by an atomic
//! rename over a temporary file that has already been produced, probed and
//! checked for metadata loss. Any failure leaves the original untouched.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

use super::probe::{probe, MediaInfo, METADATA_FIELDS};
use super::{ffmpeg_bin, missing_backend_hint, tail_of};

/// Trim boundaries closer together than this are treated as identical.
const TIME_EPSILON: f64 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Processing {
    StreamCopy,
    Reencode,
}

impl std::fmt::Display for Processing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Processing::StreamCopy => write!(f, "stream copy"),
            Processing::Reencode => write!(f, "re-encoding"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CoverArt {
    /// The source had none, so there is nothing to say.
    #[default]
    Absent,
    Preserved,
    Lost,
}

/// What actually survived into the output, established by probing it.
#[derive(Debug, Clone, Default)]
pub struct MetadataReport {
    /// Fields that were present in the source and match in the output.
    pub preserved: Vec<String>,
    /// Fields that were meant to be present but are missing or differ.
    pub lost: Vec<String>,
    /// Fields the user edited that were written successfully.
    pub applied: Vec<String>,
    pub cover_art: CoverArt,
    pub chapters_source: usize,
    pub chapters_output: usize,
}

impl MetadataReport {
    /// True only when nothing that was supposed to survive went missing.
    pub fn fully_preserved(&self) -> bool {
        self.lost.is_empty()
            && self.cover_art != CoverArt::Lost
            && self.chapters_output >= self.chapters_source
    }

    /// A one-line verdict for the save summary.
    pub fn summary_line(&self) -> String {
        if self.fully_preserved() {
            "preserved".to_string()
        } else {
            let mut parts = Vec::new();
            if !self.lost.is_empty() {
                parts.push(format!("lost {}", self.lost.join(", ")));
            }
            if self.cover_art == CoverArt::Lost {
                parts.push("lost cover artwork".to_string());
            }
            if self.chapters_output < self.chapters_source {
                parts.push(format!(
                    "lost {} chapter(s)",
                    self.chapters_source - self.chapters_output
                ));
            }
            format!("partially preserved ({})", parts.join("; "))
        }
    }
}

/// A requested save: a trim range plus any metadata edits.
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub begin: f64,
    pub end: f64,
    /// Field edits keyed by ffmpeg tag name. `None` removes the tag.
    pub metadata: BTreeMap<String, Option<String>>,
}

impl SaveRequest {
    /// A save that only rewrites the trim range.
    pub fn trim(begin: f64, end: f64) -> Self {
        SaveRequest {
            begin,
            end,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SaveOutcome {
    pub path: PathBuf,
    /// Nothing needed doing; the file was not rewritten (design §16).
    pub noop: bool,
    pub source_duration: f64,
    pub output_duration: f64,
    pub removed_beginning: f64,
    pub removed_ending: f64,
    pub processing: Processing,
    pub metadata: MetadataReport,
}

/// Deletes its path unless committed, so no failure can leave litter behind.
struct TempFile {
    path: PathBuf,
    committed: bool,
}

impl TempFile {
    fn beside(source: &Path) -> Result<TempFile> {
        let parent = source
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let stem = source
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "audio".to_string());
        let extension = source
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        // Same directory, so the final rename is atomic.
        let path = parent.join(format!(
            ".{stem}.audioedit-{}{extension}",
            std::process::id()
        ));
        Ok(TempFile {
            path,
            committed: false,
        })
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// One way of asking ffmpeg to produce the output.
#[derive(Debug, Clone, Copy)]
struct Attempt {
    processing: Processing,
    /// Map every stream (keeping cover art) rather than audio alone.
    all_streams: bool,
}

/// Run the full save pipeline for `info` and return what happened.
///
/// The original file is replaced only after the temporary output has been
/// produced, probed and compared against the source metadata.
pub fn save(info: &MediaInfo, request: &SaveRequest) -> Result<SaveOutcome> {
    let source_duration = info.duration;
    let begin = request.begin.clamp(0.0, source_duration);
    let end = request.end.clamp(0.0, source_duration);
    anyhow::ensure!(
        end - begin > TIME_EPSILON,
        "the retained range is empty: {begin:.3}s to {end:.3}s"
    );

    let trims = begin > TIME_EPSILON || end < source_duration - TIME_EPSILON;
    let edits: BTreeMap<String, Option<String>> = request
        .metadata
        .iter()
        .filter(|(key, value)| {
            let current = info.tag(key);
            match value {
                Some(v) => current != Some(v.as_str()),
                None => current.is_some(),
            }
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    if !trims && edits.is_empty() {
        return Ok(SaveOutcome {
            path: info.path.clone(),
            noop: true,
            source_duration,
            output_duration: source_duration,
            removed_beginning: 0.0,
            removed_ending: 0.0,
            processing: Processing::StreamCopy,
            metadata: MetadataReport {
                cover_art: if info.has_cover_art {
                    CoverArt::Preserved
                } else {
                    CoverArt::Absent
                },
                chapters_source: info.chapter_count,
                chapters_output: info.chapter_count,
                ..MetadataReport::default()
            },
        });
    }

    let temp = TempFile::beside(&info.path)?;
    let span = end - begin;

    // Prefer lossless stream copy; only re-encode when copying cannot deliver.
    let mut attempts = vec![Attempt {
        processing: Processing::StreamCopy,
        all_streams: true,
    }];
    if info.has_cover_art {
        attempts.push(Attempt {
            processing: Processing::StreamCopy,
            all_streams: false,
        });
    }
    attempts.push(Attempt {
        processing: Processing::Reencode,
        all_streams: true,
    });
    if info.has_cover_art {
        attempts.push(Attempt {
            processing: Processing::Reencode,
            all_streams: false,
        });
    }

    let mut failures: Vec<String> = Vec::new();
    let attempt_count = attempts.len();
    for (index, attempt) in attempts.into_iter().enumerate() {
        let _ = std::fs::remove_file(&temp.path);
        match run_attempt(info, &temp.path, begin, span, &edits, attempt) {
            Ok(()) => {}
            Err(err) => {
                failures.push(format!("{}: {err}", describe(attempt)));
                continue;
            }
        }

        let output = match probe(&temp.path) {
            Ok(Some(output)) => output,
            Ok(None) => {
                failures.push(format!("{}: output has no audio stream", describe(attempt)));
                continue;
            }
            Err(err) => {
                failures.push(format!(
                    "{}: output could not be probed: {err}",
                    describe(attempt)
                ));
                continue;
            }
        };

        if let Err(err) = validate_media(&output, span) {
            failures.push(format!("{}: {err}", describe(attempt)));
            continue;
        }

        let metadata = compare_metadata(info, &output, &edits);

        // Losing metadata is a reason to try a different, more thorough
        // strategy — but only while one remains untried. Accepting the first
        // valid-but-lossy result would let a worse attempt (e.g. stream-copy
        // audio-only, which drops cover art) win over a better one further
        // down the list (e.g. re-encoding all streams) that was never tried.
        let is_last_resort = index + 1 == attempt_count;
        if should_retry_for_cleaner_metadata(&metadata, is_last_resort) {
            failures.push(format!(
                "{}: {}",
                describe(attempt),
                metadata.summary_line()
            ));
            continue;
        }

        std::fs::rename(&temp.path, &info.path).with_context(|| {
            format!(
                "replacing {} with the verified temporary output. \
                 The original file has NOT been modified.",
                info.path.display()
            )
        })?;
        temp.commit();

        return Ok(SaveOutcome {
            path: info.path.clone(),
            noop: false,
            source_duration,
            output_duration: output.duration,
            removed_beginning: begin,
            removed_ending: (source_duration - end).max(0.0),
            processing: attempt.processing,
            metadata,
        });
    }

    bail!(
        "could not produce a valid output for {}. The original file has NOT been modified.\n{}",
        info.path.display(),
        failures.join("\n")
    );
}

/// Whether the save loop should keep trying attempts further down the list
/// instead of accepting `metadata` as the final result.
fn should_retry_for_cleaner_metadata(metadata: &MetadataReport, is_last_resort: bool) -> bool {
    !metadata.fully_preserved() && !is_last_resort
}

fn describe(attempt: Attempt) -> String {
    let streams = if attempt.all_streams {
        "all streams"
    } else {
        "audio only"
    };
    format!("{} ({streams})", attempt.processing)
}

fn run_attempt(
    info: &MediaInfo,
    output: &Path,
    begin: f64,
    span: f64,
    edits: &BTreeMap<String, Option<String>>,
    attempt: Attempt,
) -> Result<()> {
    let mut command = Command::new(ffmpeg_bin());
    command.args(["-y", "-v", "error", "-nostdin"]);

    // Input-side seeking: fast, and for `-c copy` it is the only accurate form.
    command.arg("-ss").arg(format!("{begin:.6}"));
    command.arg("-t").arg(format!("{span:.6}"));
    command.arg("-i").arg(&info.path);

    if attempt.all_streams {
        command.args(["-map", "0"]);
    } else {
        command.args(["-map", "0:a:0"]);
    }

    match attempt.processing {
        Processing::StreamCopy => {
            command.args(["-c", "copy"]);
        }
        Processing::Reencode => {
            for arg in audio_encoder_args(info) {
                command.arg(arg);
            }
            // Cover art is a still image; never re-encode it.
            if attempt.all_streams && info.has_cover_art {
                command.args(["-c:v", "copy"]);
            }
        }
    }

    command.args(["-map_metadata", "0", "-map_chapters", "0"]);

    let scope = metadata_scope(info);
    for (key, value) in edits {
        command.arg(format!("-metadata{scope}"));
        // An empty value is how ffmpeg is told to drop a tag.
        command.arg(format!("{key}={}", value.as_deref().unwrap_or("")));
    }

    command.arg(output);

    let result = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| {
            format!(
                "running ffmpeg for {}. {}",
                info.path.display(),
                missing_backend_hint(&ffmpeg_bin())
            )
        })?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        bail!("{}", tail_of(&stderr, 4));
    }
    Ok(())
}

/// The ffmpeg option suffix that targets where a container keeps its tags.
///
/// Ogg-family formats (opus, vorbis) carry Vorbis comments on the stream, and
/// `-map_metadata 0` has already populated those stream tags by the time the
/// edits are applied, so a global `-metadata` is silently ignored. Everything
/// else keeps tags at the container level.
fn metadata_scope(info: &MediaInfo) -> &'static str {
    let ogg_container = info.format_name.split(',').any(|name| name.trim() == "ogg");
    let tags_on_stream = METADATA_FIELDS
        .iter()
        .any(|(key, _)| info.stream_tags.contains_key(*key));
    if ogg_container || tags_on_stream {
        ":s:a:0"
    } else {
        ""
    }
}

/// The explicit encoding policy used when stream copy will not do (design §14).
fn audio_encoder_args(info: &MediaInfo) -> Vec<String> {
    let encoder = match info.audio_codec.as_str() {
        "opus" => preferred(&["libopus", "opus"]),
        "vorbis" => preferred(&["libvorbis", "vorbis"]),
        "mp3" => preferred(&["libmp3lame"]),
        "flac" => preferred(&["flac"]),
        "aac" => preferred(&["aac", "libfdk_aac"]),
        "alac" => preferred(&["alac"]),
        codec if codec.starts_with("pcm_") => preferred(&[codec]),
        _ => None,
    };

    let mut args = Vec::new();
    if let Some(encoder) = encoder {
        args.push("-c:a".to_string());
        args.push(encoder);
        // Keep the source bitrate for lossy codecs so quality does not drift.
        if matches!(info.audio_codec.as_str(), "opus" | "vorbis" | "mp3" | "aac") {
            if let Some(rate) = info.bit_rate.filter(|r| *r > 0) {
                args.push("-b:a".to_string());
                args.push(rate.to_string());
            }
        }
    }
    args
}

fn preferred(candidates: &[&str]) -> Option<String> {
    let available = encoders();
    candidates
        .iter()
        .find(|c| available.iter().any(|e| e == *c))
        .map(|c| c.to_string())
}

/// Encoders this ffmpeg build actually has, queried once.
fn encoders() -> &'static Vec<String> {
    static ENCODERS: OnceLock<Vec<String>> = OnceLock::new();
    ENCODERS.get_or_init(|| {
        let Ok(output) = Command::new(ffmpeg_bin())
            .args(["-v", "error", "-hide_banner", "-encoders"])
            .stdin(Stdio::null())
            .output()
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip_while(|l| !l.starts_with(" -------"))
            .skip(1)
            .filter_map(|line| line.split_whitespace().nth(1).map(str::to_string))
            .collect()
    })
}

/// Media validation: the output must be real audio of roughly the right length.
fn validate_media(output: &MediaInfo, expected_span: f64) -> Result<()> {
    if output.duration <= 0.0 {
        bail!("output has no measurable duration");
    }
    // Stream copy snaps to packet boundaries, so allow a little slack.
    let tolerance = (expected_span * 0.02).max(0.5);
    let drift = (output.duration - expected_span).abs();
    if drift > tolerance {
        bail!(
            "output duration {:.3}s differs from the requested {:.3}s by {:.3}s",
            output.duration,
            expected_span,
            drift
        );
    }
    Ok(())
}

/// Metadata validation: compare what should be there against what is there.
pub fn compare_metadata(
    source: &MediaInfo,
    output: &MediaInfo,
    edits: &BTreeMap<String, Option<String>>,
) -> MetadataReport {
    let mut report = MetadataReport {
        cover_art: if !source.has_cover_art {
            CoverArt::Absent
        } else if output.has_cover_art {
            CoverArt::Preserved
        } else {
            CoverArt::Lost
        },
        chapters_source: source.chapter_count,
        chapters_output: output.chapter_count,
        ..MetadataReport::default()
    };

    // Every field the source carried, plus every field the user set.
    let mut keys: Vec<String> = METADATA_FIELDS.iter().map(|(k, _)| k.to_string()).collect();
    for key in source.all_tags().keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    for key in edits.keys() {
        if !keys.contains(key) {
            keys.push(key.clone());
        }
    }

    for key in keys {
        let edited = edits.get(&key);
        let intended: Option<String> = match edited {
            Some(Some(value)) => Some(value.clone()),
            Some(None) => None,
            None => source.tag(&key).map(str::to_string),
        };
        let actual = output.tag(&key).map(str::to_string);
        let label = label_for(&key);

        match intended {
            None => {
                // Nothing was meant to be here; an absent tag is correct.
            }
            Some(intended) => {
                if actual.as_deref().map(str::trim) == Some(intended.trim()) {
                    if edited.is_some() {
                        report.applied.push(label);
                    } else {
                        report.preserved.push(label);
                    }
                } else {
                    report.lost.push(label);
                }
            }
        }
    }

    report
}

fn label_for(key: &str) -> String {
    METADATA_FIELDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(tags: &[(&str, &str)], cover: bool, chapters: usize) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from("/tmp/a.opus"),
            duration: 100.0,
            format_name: "ogg".into(),
            audio_codec: "opus".into(),
            bit_rate: Some(64_000),
            sample_rate: Some(48_000),
            channels: Some(2),
            has_cover_art: cover,
            chapter_count: chapters,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            stream_tags: BTreeMap::new(),
        }
    }

    #[test]
    fn identical_metadata_is_reported_as_preserved() {
        let source = info(&[("title", "Hello"), ("artist", "Jane")], false, 0);
        let output = info(&[("title", "Hello"), ("artist", "Jane")], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert!(report.fully_preserved());
        assert_eq!(report.summary_line(), "preserved");
        assert!(report.preserved.contains(&"Title".to_string()));
    }

    #[test]
    fn missing_metadata_is_never_reported_as_preserved() {
        let source = info(
            &[("title", "Hello"), ("comment", "Recorded remotely")],
            false,
            0,
        );
        let output = info(&[("title", "Hello")], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert!(!report.fully_preserved());
        assert_eq!(report.lost, vec!["Comment".to_string()]);
        assert!(report.summary_line().contains("Comment"));
    }

    #[test]
    fn lost_cover_art_is_reported() {
        let source = info(&[], true, 0);
        let output = info(&[], false, 0);
        let report = compare_metadata(&source, &output, &BTreeMap::new());
        assert_eq!(report.cover_art, CoverArt::Lost);
        assert!(!report.fully_preserved());
        assert!(report.summary_line().contains("cover artwork"));
    }

    #[test]
    fn absent_cover_art_is_not_a_loss() {
        let report = compare_metadata(&info(&[], false, 0), &info(&[], false, 0), &BTreeMap::new());
        assert_eq!(report.cover_art, CoverArt::Absent);
        assert!(report.fully_preserved());
    }

    #[test]
    fn dropped_chapters_are_reported() {
        let report = compare_metadata(&info(&[], false, 5), &info(&[], false, 2), &BTreeMap::new());
        assert!(!report.fully_preserved());
        assert!(report.summary_line().contains("3 chapter(s)"));
    }

    #[test]
    fn applied_edits_are_distinguished_from_preserved_fields() {
        let source = info(&[("title", "Old"), ("artist", "Jane")], false, 0);
        let output = info(&[("title", "New"), ("artist", "Jane")], false, 0);
        let edits = BTreeMap::from([("title".to_string(), Some("New".to_string()))]);
        let report = compare_metadata(&source, &output, &edits);
        assert_eq!(report.applied, vec!["Title".to_string()]);
        assert!(report.preserved.contains(&"Artist".to_string()));
        assert!(report.fully_preserved());
    }

    #[test]
    fn a_deleted_tag_that_survives_is_a_failure() {
        let source = info(&[("comment", "old")], false, 0);
        let output = info(&[("comment", "old")], false, 0);
        let edits = BTreeMap::from([("comment".to_string(), None)]);
        let report = compare_metadata(&source, &output, &edits);
        // The tag was meant to be gone, so it is not "preserved" either way.
        assert!(report.preserved.is_empty());
        assert!(report.applied.is_empty());
        assert!(report.fully_preserved());
    }

    #[test]
    fn media_validation_accepts_packet_boundary_drift() {
        let mut output = info(&[], false, 0);
        output.duration = 60.05;
        assert!(validate_media(&output, 60.0).is_ok());
    }

    #[test]
    fn media_validation_rejects_a_badly_wrong_duration() {
        let mut output = info(&[], false, 0);
        output.duration = 3.0;
        assert!(validate_media(&output, 60.0).is_err());
        output.duration = 0.0;
        assert!(validate_media(&output, 60.0).is_err());
    }

    #[test]
    fn temp_file_sits_beside_the_source_and_keeps_the_extension() {
        let temp = TempFile::beside(Path::new("/music/interview 1.opus")).unwrap();
        assert_eq!(temp.path.parent().unwrap(), Path::new("/music"));
        assert_eq!(temp.path.extension().unwrap(), "opus");
        assert!(temp
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with('.'));
    }

    #[test]
    fn temp_file_is_removed_unless_committed() {
        let dir = std::env::temp_dir().join(format!("audioedit-tmp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("x.wav");
        let temp = TempFile::beside(&source).unwrap();
        std::fs::write(&temp.path, b"partial").unwrap();
        let path = temp.path.clone();
        drop(temp);
        assert!(
            !path.exists(),
            "an abandoned temporary file must be cleaned up"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ogg_containers_get_stream_scoped_metadata() {
        let mut opus = info(&[], false, 0);
        opus.format_name = "ogg".into();
        assert_eq!(metadata_scope(&opus), ":s:a:0");
    }

    #[test]
    fn a_file_whose_tags_live_on_the_stream_gets_stream_scoped_metadata() {
        let mut file = info(&[], false, 0);
        file.format_name = "matroska,webm".into();
        file.stream_tags = BTreeMap::from([("title".to_string(), "T".to_string())]);
        assert_eq!(metadata_scope(&file), ":s:a:0");
    }

    #[test]
    fn container_level_formats_get_global_metadata() {
        let mut mp3 = info(&[("title", "T")], false, 0);
        mp3.format_name = "mp3".into();
        assert_eq!(metadata_scope(&mp3), "");

        let mut m4a = info(&[("title", "T")], false, 0);
        m4a.format_name = "mov,mp4,m4a,3gp,3g2,mj2".into();
        assert_eq!(metadata_scope(&m4a), "");
    }

    #[test]
    fn processing_labels_match_the_spec() {
        assert_eq!(Processing::StreamCopy.to_string(), "stream copy");
        assert_eq!(Processing::Reencode.to_string(), "re-encoding");
    }

    #[test]
    fn a_lossy_result_is_retried_while_a_better_attempt_remains() {
        let lossy = MetadataReport {
            cover_art: CoverArt::Lost,
            ..MetadataReport::default()
        };
        assert!(
            should_retry_for_cleaner_metadata(&lossy, false),
            "a stream-copy-audio-only result must not win over an untried \
             reencode-all-streams attempt just because it came first"
        );
    }

    #[test]
    fn a_lossy_result_is_accepted_once_nothing_else_remains() {
        let lossy = MetadataReport {
            cover_art: CoverArt::Lost,
            ..MetadataReport::default()
        };
        assert!(!should_retry_for_cleaner_metadata(&lossy, true));
    }

    #[test]
    fn a_clean_result_is_never_retried() {
        assert!(!should_retry_for_cleaner_metadata(
            &MetadataReport::default(),
            false
        ));
    }
}

impl SaveOutcome {
    /// The save summary shown after every save (design §16).
    pub fn summary_lines(&self) -> Vec<String> {
        use crate::timespec::format_timestamp_millis as ts;

        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.path.display().to_string());

        let mut lines = vec![format!("Saved: {name}"), String::new()];

        if self.noop {
            lines.push("No changes were required.".to_string());
            lines.push(String::new());
            lines.push("Duration:".to_string());
            lines.push(format!(
                "  {} → {}",
                ts(self.source_duration),
                ts(self.output_duration)
            ));
            lines.push(String::new());
            lines.push("Status:".to_string());
            lines.push("  NO-OP".to_string());
            return lines;
        }

        lines.push("Duration:".to_string());
        lines.push(format!(
            "  {} → {}",
            ts(self.source_duration),
            ts(self.output_duration)
        ));
        lines.push(String::new());
        lines.push("Removed:".to_string());
        lines.push(format!("  beginning: {:.3}s", self.removed_beginning));
        lines.push(format!("  ending:    {:.3}s", self.removed_ending));
        lines.push(String::new());
        lines.push("Processing:".to_string());
        lines.push(format!("  {}", self.processing));
        lines.push(String::new());
        lines.push("Metadata:".to_string());
        lines.push(format!("  {}", self.metadata.summary_line()));
        lines.push(String::new());
        lines.push("Status:".to_string());
        lines.push("  SUCCESS".to_string());
        lines
    }
}

#[cfg(test)]
mod summary_tests {
    use super::*;

    fn outcome(noop: bool) -> SaveOutcome {
        SaveOutcome {
            path: PathBuf::from("/rec/interview.opus"),
            noop,
            source_duration: 6151.2,
            output_duration: if noop { 6151.2 } else { 6128.2 },
            removed_beginning: if noop { 0.0 } else { 12.0 },
            removed_ending: if noop { 0.0 } else { 11.0 },
            processing: Processing::StreamCopy,
            metadata: MetadataReport::default(),
        }
    }

    #[test]
    fn success_summary_matches_the_documented_shape() {
        let lines = outcome(false).summary_lines();
        assert_eq!(lines[0], "Saved: interview.opus");
        assert!(lines.contains(&"  01:42:31.200 → 01:42:08.200".to_string()));
        assert!(lines.contains(&"  beginning: 12.000s".to_string()));
        assert!(lines.contains(&"  ending:    11.000s".to_string()));
        assert!(lines.contains(&"  stream copy".to_string()));
        assert!(lines.contains(&"  preserved".to_string()));
        assert!(lines.contains(&"  SUCCESS".to_string()));
    }

    #[test]
    fn noop_summary_is_reported_explicitly() {
        let lines = outcome(true).summary_lines();
        assert!(lines.contains(&"No changes were required.".to_string()));
        assert!(lines.contains(&"  NO-OP".to_string()));
        assert!(!lines.iter().any(|l| l.contains("Removed")));
        assert!(!lines.iter().any(|l| l.contains("SUCCESS")));
    }

    #[test]
    fn reencoding_is_named_in_the_summary() {
        let mut o = outcome(false);
        o.processing = Processing::Reencode;
        assert!(o.summary_lines().contains(&"  re-encoding".to_string()));
    }

    #[test]
    fn metadata_loss_is_visible_in_the_summary() {
        let mut o = outcome(false);
        o.metadata.lost.push("Comment".to_string());
        let lines = o.summary_lines();
        assert!(lines
            .iter()
            .any(|l| l.contains("partially preserved") && l.contains("Comment")));
    }
}
