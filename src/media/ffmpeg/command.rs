//! Building and running the ffmpeg command for one save attempt, and
//! validating what came out the other end (design §13–§14).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};

use super::super::probe::{MediaInfo, METADATA_FIELDS};
use super::super::{ffmpeg_bin, missing_backend_hint, tail_of};
use super::{Attempt, Processing};

pub(super) fn describe(attempt: Attempt) -> String {
    let streams = if attempt.all_streams {
        "all streams"
    } else {
        "audio only"
    };
    format!("{} ({streams})", attempt.processing)
}

pub(super) fn run_attempt(
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
pub(super) fn metadata_scope(info: &MediaInfo) -> &'static str {
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
pub(super) fn validate_media(output: &MediaInfo, expected_span: f64) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info(tags: &[(&str, &str)], cover: bool, chapters: usize) -> MediaInfo {
        MediaInfo {
            has_cover_art: cover,
            chapter_count: chapters,
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..crate::media::probe::fixture()
        }
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
}
