//! The processing and save pipeline (design §13–§16).
//!
//! Saving is in place, but the original is only ever replaced by an atomic
//! rename over a temporary file that has already been produced, probed and
//! checked for metadata loss. Any failure leaves the original untouched.
//!
//! Split by concern: [`command`] builds and runs the actual ffmpeg
//! invocation and validates its output, and [`metadata`] compares what
//! survived a save against what was there before.

mod command;
mod metadata;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::probe::{probe, MediaInfo};

pub use metadata::{compare_metadata, CoverArt, MetadataReport};

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
        match command::run_attempt(info, &temp.path, begin, span, &edits, attempt) {
            Ok(()) => {}
            Err(err) => {
                failures.push(format!("{}: {err}", command::describe(attempt)));
                continue;
            }
        }

        let output = match probe(&temp.path) {
            Ok(Some(output)) => output,
            Ok(None) => {
                failures.push(format!(
                    "{}: output has no audio stream",
                    command::describe(attempt)
                ));
                continue;
            }
            Err(err) => {
                failures.push(format!(
                    "{}: output could not be probed: {err}",
                    command::describe(attempt)
                ));
                continue;
            }
        };

        if let Err(err) = command::validate_media(&output, span) {
            failures.push(format!("{}: {err}", command::describe(attempt)));
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
                command::describe(attempt),
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

#[cfg(test)]
mod tests {
    use super::*;

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
