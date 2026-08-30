//! Automatic beginning/end marker detection (design §11).
//!
//! Detection is delegated to ffmpeg's `silencedetect` filter so the thresholds
//! the user configures in dB mean exactly what they mean in ffmpeg. Beginning
//! and ending are configured independently, so two passes may be needed.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::{ffmpeg_bin, tail_of};
use crate::config::AutoTrim as AutoTrimConfig;

/// How close to a boundary a silence must be to count as leading/trailing.
const EDGE_TOLERANCE: f64 = 0.25;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimSuggestion {
    pub begin: f64,
    pub end: f64,
    /// False when no leading silence was found, i.e. `begin` is just 0.
    pub begin_detected: bool,
    /// False when no trailing silence was found, i.e. `end` is the duration.
    pub end_detected: bool,
}

impl TrimSuggestion {
    pub fn none(duration: f64) -> Self {
        TrimSuggestion {
            begin: 0.0,
            end: duration,
            begin_detected: false,
            end_detected: false,
        }
    }
}

/// A detected silent interval; `end` is `None` when it runs to end of file.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Silence {
    start: f64,
    end: Option<f64>,
}

/// Detect sensible markers for a file.
pub fn detect(path: &Path, duration: f64, config: &AutoTrimConfig) -> Result<TrimSuggestion> {
    if duration <= 0.0 {
        return Ok(TrimSuggestion::none(duration));
    }

    let begin_silences =
        run_silencedetect(path, config.begin_threshold_db, config.begin_min_duration)?;

    // The two passes are identical unless the user configured them apart.
    let same_settings = config.begin_threshold_db == config.end_threshold_db
        && config.begin_min_duration == config.end_min_duration;
    let end_silences = if same_settings {
        begin_silences.clone()
    } else {
        run_silencedetect(path, config.end_threshold_db, config.end_min_duration)?
    };

    let (begin, begin_detected) = leading_edge(&begin_silences, duration);
    let (end, end_detected) = trailing_edge(&end_silences, duration);

    // A file that is silent throughout would otherwise collapse to nothing;
    // fall back to keeping everything rather than proposing an empty edit.
    if end <= begin {
        return Ok(TrimSuggestion::none(duration));
    }

    Ok(TrimSuggestion {
        begin,
        end,
        begin_detected,
        end_detected,
    })
}

fn leading_edge(silences: &[Silence], duration: f64) -> (f64, bool) {
    match silences.first() {
        Some(first) if first.start <= EDGE_TOLERANCE => match first.end {
            Some(end) if end < duration => (end, true),
            // Silence covers the whole file.
            _ => (0.0, false),
        },
        _ => (0.0, false),
    }
}

fn trailing_edge(silences: &[Silence], duration: f64) -> (f64, bool) {
    match silences.last() {
        Some(last) => {
            let runs_to_eof = match last.end {
                None => true,
                Some(end) => end >= duration - EDGE_TOLERANCE,
            };
            if runs_to_eof && last.start > 0.0 {
                (last.start, true)
            } else {
                (duration, false)
            }
        }
        None => (duration, false),
    }
}

fn run_silencedetect(path: &Path, threshold_db: f64, min_duration: f64) -> Result<Vec<Silence>> {
    let filter = format!("silencedetect=noise={threshold_db}dB:d={min_duration}");
    let output = Command::new(ffmpeg_bin())
        .args(["-v", "info", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-af", &filter, "-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("running silence detection on {}", path.display()))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        bail!(
            "silence detection failed for {}: {}",
            path.display(),
            tail_of(&stderr, 3)
        );
    }
    Ok(parse_silences(&stderr))
}

/// Parse `silencedetect` output into intervals.
fn parse_silences(stderr: &str) -> Vec<Silence> {
    let mut silences: Vec<Silence> = Vec::new();
    for line in stderr.lines() {
        if let Some(value) = field_after(line, "silence_start:") {
            silences.push(Silence {
                start: value,
                end: None,
            });
        } else if let Some(value) = field_after(line, "silence_end:") {
            // An end without a start would be malformed output; ignore it.
            if let Some(open) = silences.last_mut() {
                if open.end.is_none() {
                    open.end = Some(value);
                }
            }
        }
    }
    silences
}

/// Read the number that follows `key` on a log line.
fn field_after(line: &str, key: &str) -> Option<f64> {
    let rest = line.split(key).nth(1)?;
    let token = rest.split_whitespace().next()?;
    token.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
[silencedetect @ 0x55d1] silence_start: 0
[silencedetect @ 0x55d1] silence_end: 12.0032 | silence_duration: 12.0032
[silencedetect @ 0x55d1] silence_start: 101.5
[silencedetect @ 0x55d1] silence_end: 112.5 | silence_duration: 11
";

    #[test]
    fn parses_silence_intervals() {
        let silences = parse_silences(SAMPLE);
        assert_eq!(silences.len(), 2);
        assert_eq!(
            silences[0],
            Silence {
                start: 0.0,
                end: Some(12.0032)
            }
        );
        assert_eq!(
            silences[1],
            Silence {
                start: 101.5,
                end: Some(112.5)
            }
        );
    }

    #[test]
    fn parses_a_silence_that_runs_to_end_of_file() {
        let silences = parse_silences("[silencedetect @ 0x1] silence_start: 90.25\n");
        assert_eq!(
            silences,
            vec![Silence {
                start: 90.25,
                end: None
            }]
        );
    }

    #[test]
    fn ignores_unrelated_log_noise() {
        let noise = "Input #0, ogg, from 'a.opus':\n  Duration: 00:01:52.50, bitrate: 64 kb/s\n";
        assert!(parse_silences(noise).is_empty());
    }

    #[test]
    fn finds_leading_and_trailing_edges() {
        let silences = parse_silences(SAMPLE);
        assert_eq!(leading_edge(&silences, 112.5), (12.0032, true));
        assert_eq!(trailing_edge(&silences, 112.5), (101.5, true));
    }

    #[test]
    fn no_leading_silence_keeps_the_start() {
        let silences = parse_silences("[silencedetect @ 0x1] silence_start: 50\n[silencedetect @ 0x1] silence_end: 55 | silence_duration: 5\n");
        assert_eq!(leading_edge(&silences, 120.0), (0.0, false));
        // A silence in the middle is not a trailing silence either.
        assert_eq!(trailing_edge(&silences, 120.0), (120.0, false));
    }

    #[test]
    fn silence_running_to_eof_sets_the_end() {
        let silences = parse_silences("[silencedetect @ 0x1] silence_start: 90.25\n");
        assert_eq!(trailing_edge(&silences, 100.0), (90.25, true));
    }

    #[test]
    fn a_fully_silent_file_is_left_alone() {
        let silences = vec![Silence {
            start: 0.0,
            end: None,
        }];
        assert_eq!(leading_edge(&silences, 60.0), (0.0, false));
        assert_eq!(trailing_edge(&silences, 60.0), (60.0, false));
    }

    #[test]
    fn no_detection_is_a_full_length_suggestion() {
        let s = TrimSuggestion::none(42.0);
        assert_eq!(s.begin, 0.0);
        assert_eq!(s.end, 42.0);
        assert!(!s.begin_detected && !s.end_detected);
    }
}
