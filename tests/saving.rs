//! The save pipeline: trimming, metadata edits and cover art (design §13–§16).

mod common;

use std::process::Command;

use audioedit::media::ffmpeg;
use audioedit::media::ffmpeg::{CoverArt, Processing, SaveRequest};

use common::{probe_ok, temp_files, Workspace, TAGS};

#[test]
fn trimming_uses_stream_copy_and_preserves_metadata() {
    let ws = Workspace::new("trim");
    // FLAC is the exception: ffmpeg copies the source STREAMINFO block, so a
    // stream-copied trim declares the *original* length in its header while
    // holding the trimmed audio. Validation rejects that and re-encodes, which
    // is still lossless for FLAC.
    for (name, codec, expected) in [
        (
            "a.opus",
            vec!["-c:a", "libopus", "-b:a", "64k"],
            Processing::StreamCopy,
        ),
        ("b.flac", vec!["-c:a", "flac"], Processing::Reencode),
        (
            "c.mp3",
            vec!["-c:a", "libmp3lame", "-b:a", "128k"],
            Processing::StreamCopy,
        ),
        (
            "d.m4a",
            vec!["-c:a", "aac", "-b:a", "128k"],
            Processing::StreamCopy,
        ),
    ] {
        let path = ws.make(name, &codec);
        let info = probe_ok(&path);

        let outcome = ffmpeg::save(&info, &SaveRequest::trim(4.0, 10.0))
            .unwrap_or_else(|e| panic!("saving {name}: {e:#}"));

        assert!(!outcome.noop, "{name} should have changed");
        assert_eq!(
            outcome.processing, expected,
            "{name} used the wrong strategy"
        );
        assert!(
            (outcome.output_duration - 6.0).abs() < 0.3,
            "{name} became {}s",
            outcome.output_duration
        );
        assert!((outcome.removed_beginning - 4.0).abs() < 0.001);
        // Encoders pad slightly, so the source is not exactly 17.000 s.
        let expected_ending = info.duration - 10.0;
        assert!((outcome.removed_ending - expected_ending).abs() < 0.001);
        assert!(
            outcome.metadata.fully_preserved(),
            "{name}: {}",
            outcome.metadata.summary_line()
        );

        // The claim must hold when the file is read back from disk.
        let saved = probe_ok(&path);
        assert!(
            (saved.duration - outcome.output_duration).abs() < 0.001,
            "{name} reports {}s but reads back as {}s",
            outcome.output_duration,
            saved.duration
        );
        assert!(
            (saved.duration - 6.0).abs() < 0.3,
            "{name} is {}s on disk, expected about 6s",
            saved.duration
        );
        for (key, value) in TAGS {
            if info.tag(key).is_some() {
                assert_eq!(saved.tag(key), Some(*value), "{name} lost {key}");
            }
        }
    }
    assert!(
        temp_files(ws.path()).is_empty(),
        "temporary files were left behind"
    );
}

#[test]
fn saving_an_unchanged_file_is_a_reported_noop_and_does_not_rewrite_it() {
    let ws = Workspace::new("noop");
    let path = ws.make("a.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    let info = probe_ok(&path);
    let before = std::fs::read(&path).unwrap();

    let outcome = ffmpeg::save(&info, &SaveRequest::trim(0.0, info.duration)).expect("saving");

    assert!(outcome.noop);
    assert_eq!(outcome.removed_beginning, 0.0);
    assert_eq!(outcome.removed_ending, 0.0);
    assert_eq!(outcome.source_duration, outcome.output_duration);

    let summary = outcome.summary_lines().join("\n");
    assert!(summary.contains("No changes were required."));
    assert!(summary.contains("NO-OP"));

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "a no-op must not rewrite the file"
    );
}

#[test]
fn a_failed_save_leaves_the_original_byte_identical_and_cleans_up() {
    let ws = Workspace::new("failure");
    let path = ws.make("a.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    let before = std::fs::read(&path).unwrap();

    // Claim the file is far longer than it is, then ask for a range that lies
    // beyond its real end. Every strategy must fail validation.
    let mut info = probe_ok(&path);
    info.duration = 600.0;

    let error = ffmpeg::save(&info, &SaveRequest::trim(500.0, 590.0))
        .expect_err("a range past the end of the file must not succeed");
    let text = format!("{error:#}");
    assert!(
        text.contains("NOT been modified"),
        "unhelpful error: {text}"
    );

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the original was damaged"
    );
    assert!(
        temp_files(ws.path()).is_empty(),
        "a failure left temporary files behind"
    );
}

#[test]
fn an_empty_retained_range_is_refused_before_anything_runs() {
    let ws = Workspace::new("emptyrange");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);
    let before = std::fs::read(&path).unwrap();

    assert!(ffmpeg::save(&info, &SaveRequest::trim(5.0, 5.0)).is_err());
    assert!(ffmpeg::save(&info, &SaveRequest::trim(8.0, 2.0)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn metadata_edits_are_written_for_every_container() {
    let ws = Workspace::new("metaformats");
    // Ogg-family containers keep tags on the stream rather than the container,
    // so an edit has to be scoped correctly or it is silently dropped.
    for (name, codec) in [
        ("a.opus", vec!["-c:a", "libopus", "-b:a", "64k"]),
        ("b.ogg", vec!["-c:a", "libvorbis"]),
        ("c.flac", vec!["-c:a", "flac"]),
        ("d.mp3", vec!["-c:a", "libmp3lame", "-b:a", "128k"]),
        ("e.m4a", vec!["-c:a", "aac", "-b:a", "128k"]),
    ] {
        let path = ws.make(name, &codec);
        let info = probe_ok(&path);
        assert_eq!(info.tag("title"), Some("Interview with Jane"));

        let request = SaveRequest {
            begin: 0.0,
            end: info.duration,
            metadata: [("title".to_string(), Some("Renamed Episode".to_string()))]
                .into_iter()
                .collect(),
        };
        let outcome =
            ffmpeg::save(&info, &request).unwrap_or_else(|e| panic!("saving {name}: {e:#}"));

        assert!(!outcome.noop, "{name}: an edit is a real change");
        assert!(
            outcome.metadata.applied.contains(&"Title".to_string()),
            "{name}: {}",
            outcome.metadata.summary_line()
        );
        assert!(
            outcome.metadata.fully_preserved(),
            "{name}: {}",
            outcome.metadata.summary_line()
        );

        let saved = probe_ok(&path);
        assert_eq!(
            saved.tag("title"),
            Some("Renamed Episode"),
            "{name} kept the old title"
        );
        assert_eq!(
            saved.tag("artist"),
            Some("Example Podcast"),
            "{name} lost other tags"
        );
    }
    assert!(temp_files(ws.path()).is_empty());
}

#[test]
fn metadata_edits_are_written_and_verified() {
    let ws = Workspace::new("metadata");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);

    let request = SaveRequest {
        begin: 0.0,
        end: info.duration,
        metadata: [
            ("title".to_string(), Some("A New Title".to_string())),
            ("comment".to_string(), None),
        ]
        .into_iter()
        .collect(),
    };

    let outcome = ffmpeg::save(&info, &request).expect("saving metadata");
    assert!(!outcome.noop, "a metadata edit is a real change");
    assert!(outcome.metadata.applied.contains(&"Title".to_string()));
    assert!(
        outcome.metadata.fully_preserved(),
        "{}",
        outcome.metadata.summary_line()
    );

    let saved = probe_ok(&path);
    assert_eq!(saved.tag("title"), Some("A New Title"));
    assert_eq!(
        saved.tag("comment"),
        None,
        "the comment should have been removed"
    );
    assert_eq!(
        saved.tag("artist"),
        Some("Example Podcast"),
        "other tags survive"
    );
    // Untrimmed audio keeps its full length.
    assert!((saved.duration - info.duration).abs() < 0.2);
}

#[test]
fn a_metadata_edit_that_matches_the_current_value_is_a_noop() {
    let ws = Workspace::new("sametag");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);
    let before = std::fs::read(&path).unwrap();

    let request = SaveRequest {
        begin: 0.0,
        end: info.duration,
        metadata: [("title".to_string(), Some("Interview with Jane".to_string()))]
            .into_iter()
            .collect(),
    };
    let outcome = ffmpeg::save(&info, &request).expect("saving");
    assert!(outcome.noop);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn cover_art_survives_a_trim_or_is_reported_as_lost() {
    let ws = Workspace::new("cover");
    let cover = ws.path().join("cover.png");
    let ok = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=64x64:d=1",
            "-frames:v",
            "1",
        ])
        .arg(&cover)
        .status()
        .expect("running ffmpeg")
        .success();
    assert!(ok, "could not build a cover image");

    let audio = ws.make("plain.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let with_cover = ws.path().join("withcover.mp3");
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&audio)
        .arg("-i")
        .arg(&cover)
        .args([
            "-map",
            "0:a",
            "-map",
            "1:v",
            "-c",
            "copy",
            "-id3v2_version",
            "3",
            "-disposition:v",
            "attached_pic",
        ])
        .arg(&with_cover)
        .status()
        .expect("running ffmpeg")
        .success();
    assert!(ok, "could not attach cover art");

    let info = probe_ok(&with_cover);
    assert!(info.has_cover_art, "the fixture should carry cover art");

    let outcome = ffmpeg::save(&info, &SaveRequest::trim(2.0, 8.0)).expect("saving");
    let saved = probe_ok(&with_cover);

    // Whatever happened, the report must match what is actually on disk.
    match outcome.metadata.cover_art {
        CoverArt::Preserved => assert!(saved.has_cover_art, "cover art was falsely reported kept"),
        CoverArt::Lost => assert!(!saved.has_cover_art),
        CoverArt::Absent => panic!("the source had cover art"),
    }
    assert!(temp_files(ws.path()).is_empty());
}
