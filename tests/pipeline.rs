//! End-to-end tests against real audio files produced by ffmpeg.
//!
//! These exercise the guarantees that matter most: the original file is never
//! damaged, metadata claims are truthful, and no-ops are reported as such.

use std::path::{Path, PathBuf};
use std::process::Command;

use audioedit::batch::{self, ItemStatus, RunMode};
use audioedit::config::Config;
use audioedit::media::ffmpeg::{CoverArt, Processing, SaveRequest};
use audioedit::media::probe::MediaInfo;
use audioedit::media::{autotrim, ffmpeg, probe, waveform};

/// Standard tags applied to every fixture.
const TAGS: &[(&str, &str)] = &[
    ("title", "Interview with Jane"),
    ("artist", "Example Podcast"),
    ("album", "Episode 42"),
    ("album_artist", "Example Podcast"),
    ("date", "2026-08-21"),
    ("genre", "Podcast"),
    ("comment", "Recorded remotely"),
];

/// A temporary directory removed when the test finishes.
struct Workspace(PathBuf);

impl Workspace {
    fn new(name: &str) -> Workspace {
        let path = std::env::temp_dir().join(format!(
            "audioedit-it-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the workspace");
        Workspace(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Write a fixture: 4 s of silence, 6 s of tone, 7 s of silence. The
    /// silences comfortably clear the default min durations (3s / 5s).
    fn make(&self, name: &str, codec: &[&str]) -> PathBuf {
        let out = self.0.join(name);
        let mut command = Command::new("ffmpeg");
        command.args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc='0.5*sin(2*PI*440*t)*between(t,4,10)':s=48000:d=17:c=stereo",
        ]);
        command.args(codec);
        for (key, value) in TAGS {
            command.arg("-metadata").arg(format!("{key}={value}"));
        }
        command.arg(&out);
        let status = command.status().expect("running ffmpeg");
        assert!(status.success(), "could not build fixture {name}");
        out
    }

    /// A fixture with no silence anywhere, so nothing should be trimmed.
    fn make_continuous(&self, name: &str) -> PathBuf {
        let out = self.0.join(name);
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc='0.5*sin(2*PI*440*t)':s=48000:d=5:c=stereo",
                "-c:a",
                "libopus",
            ])
            .arg(&out)
            .status()
            .expect("running ffmpeg");
        assert!(status.success());
        out
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn probe_ok(path: &Path) -> MediaInfo {
    probe::probe(path)
        .unwrap_or_else(|e| panic!("probing {}: {e}", path.display()))
        .unwrap_or_else(|| panic!("{} has no audio stream", path.display()))
}

/// Files audioedit leaves behind while working, which must never survive.
fn temp_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("listing the workspace")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("audioedit-"))
        .collect()
}

// ---- probing ---------------------------------------------------------------

#[test]
fn scanning_finds_audio_in_every_supported_format_and_skips_the_rest() {
    let ws = Workspace::new("scan");
    ws.make("a.wav", &[]);
    ws.make("b.flac", &["-c:a", "flac"]);
    ws.make("c.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]);
    ws.make("d.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    ws.make("e.m4a", &["-c:a", "aac", "-b:a", "128k"]);
    ws.make("f.ogg", &["-c:a", "libvorbis"]);
    std::fs::write(ws.path().join("notes.txt"), b"not audio").unwrap();
    std::fs::write(ws.path().join("cover.png"), b"not audio either").unwrap();

    let files = probe::scan_folder(ws.path()).expect("scanning");
    let names: Vec<String> = files.iter().map(MediaInfo::file_name).collect();
    assert_eq!(
        names,
        vec!["a.wav", "b.flac", "c.mp3", "d.opus", "e.m4a", "f.ogg"]
    );

    let codecs: Vec<&str> = files.iter().map(|f| f.audio_codec.as_str()).collect();
    assert_eq!(
        codecs,
        vec!["pcm_s16le", "flac", "mp3", "opus", "aac", "vorbis"]
    );
    for file in &files {
        assert!((file.duration - 17.0).abs() < 0.2, "{}", file.file_name());
    }
}

#[test]
fn a_file_that_is_not_media_is_reported_as_such() {
    let ws = Workspace::new("notmedia");
    let path = ws.path().join("broken.opus");
    std::fs::write(&path, b"this is definitely not an ogg stream").unwrap();
    // Either an error or "no audio stream" is acceptable; a panic is not.
    if let Ok(result) = probe::probe(&path) {
        assert!(result.is_none());
    }
}

// ---- automatic markers (design §11) ---------------------------------------

#[test]
fn automatic_markers_find_the_leading_and_trailing_silence() {
    let ws = Workspace::new("auto");
    let path = ws.make("speech.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);

    let suggestion = autotrim::detect(&path, info.duration, &Config::default().auto_trim)
        .expect("detecting silence");

    assert!(suggestion.begin_detected && suggestion.end_detected);
    assert!(
        (suggestion.begin - 4.0).abs() < 0.3,
        "begin was {}",
        suggestion.begin
    );
    assert!(
        (suggestion.end - 10.0).abs() < 0.3,
        "end was {}",
        suggestion.end
    );
}

#[test]
fn a_file_without_silence_yields_no_detection() {
    let ws = Workspace::new("continuous");
    let path = ws.make_continuous("tone.opus");
    let info = probe_ok(&path);

    let suggestion = autotrim::detect(&path, info.duration, &Config::default().auto_trim)
        .expect("detecting silence");
    assert!(!suggestion.begin_detected);
    assert!(!suggestion.end_detected);
    assert_eq!(suggestion.begin, 0.0);
}

#[test]
fn thresholds_are_configurable_independently() {
    let ws = Workspace::new("thresholds");
    let path = ws.make("speech.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);

    let mut config = Config::default().auto_trim;
    // A minimum duration longer than the trailing silence suppresses it.
    config.end_min_duration = 8.0;
    let suggestion = autotrim::detect(&path, info.duration, &config).expect("detecting");
    assert!(
        suggestion.begin_detected,
        "the 4s leading silence still qualifies against the default 3s minimum"
    );
    assert!(
        !suggestion.end_detected,
        "the 7s trailing silence is now too short against an 8s minimum"
    );
}

// ---- saving (design §13–§16) ----------------------------------------------

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

// ---- waveform (design §7) --------------------------------------------------

#[test]
fn waveform_analysis_follows_the_amplitude_envelope() {
    let ws = Workspace::new("waveform");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);

    let analysis = waveform::analyse(&path, info.duration).expect("analysing");
    assert!(!analysis.peaks.is_empty());
    assert_eq!(analysis.peaks.len(), analysis.rms.len());

    // 11 s of audio: silent for 0-2 s, tone for 2-8 s, silent for 8-11 s.
    let columns = analysis.downsample(110);
    let quiet_start: f32 = columns[2..8].iter().map(|c| c.0).sum();
    let loud_middle: f32 = columns[40..60].iter().map(|c| c.0).sum::<f32>() / 20.0;
    let quiet_end: f32 = columns[100..108].iter().map(|c| c.0).sum();

    assert!(quiet_start < 0.05, "leading silence read as {quiet_start}");
    assert!(quiet_end < 0.05, "trailing silence read as {quiet_end}");
    assert!(loud_middle > 0.3, "the tone read as {loud_middle}");
}

#[test]
fn waveform_analysis_is_cached_and_returns_the_same_shape() {
    let ws = Workspace::new("wfcache");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let info = probe_ok(&path);

    let first = waveform::analyse(&path, info.duration).expect("analysing");
    let second = waveform::analyse(&path, info.duration).expect("re-analysing");
    assert_eq!(first.peaks, second.peaks);
    assert_eq!(first.rms, second.rms);
}

// ---- folder runs (design §17) ----------------------------------------------

#[test]
fn a_dry_run_reports_changes_without_touching_any_file() {
    let ws = Workspace::new("dryrun");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    ws.make_continuous("c.opus");

    let files = probe::scan_folder(ws.path()).expect("scanning");
    let before: Vec<Vec<u8>> = files
        .iter()
        .map(|f| std::fs::read(&f.path).unwrap())
        .collect();

    let report = batch::run(&files, &[], &Config::default(), RunMode::DryRun, |_| {});

    assert_eq!(report.processed(), 3);
    assert_eq!(report.changed(), 2, "two files have silence to trim");
    assert_eq!(report.noop(), 1);
    assert_eq!(report.failed(), 0);
    assert!(report
        .items
        .iter()
        .any(|i| matches!(i.status, ItemStatus::WouldChange { .. })));
    assert!(report
        .summary_lines()
        .iter()
        .any(|l| l.contains("no files were modified")));

    for (file, original) in files.iter().zip(&before) {
        assert_eq!(
            &std::fs::read(&file.path).unwrap(),
            original,
            "{} changed",
            file.file_name()
        );
    }
    assert!(temp_files(ws.path()).is_empty());
}

#[test]
fn an_applied_run_trims_each_file_independently_and_reports_noops() {
    let ws = Workspace::new("apply");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);
    ws.make_continuous("c.opus");

    let files = probe::scan_folder(ws.path()).expect("scanning");
    let mut seen = Vec::new();
    let report = batch::run(&files, &[], &Config::default(), RunMode::Apply, |progress| {
        if let batch::Progress::Item(item) = progress {
            seen.push(item.number);
        }
    });

    assert_eq!(seen, vec![1, 2, 3], "every file is processed in order");
    assert_eq!(report.changed(), 2);
    assert_eq!(report.noop(), 1);
    assert_eq!(report.failed(), 0);

    for file in probe::scan_folder(ws.path()).expect("rescanning") {
        let expected = if file.file_name() == "c.opus" {
            5.0
        } else {
            6.0
        };
        assert!(
            (file.duration - expected).abs() < 0.4,
            "{} is {}s, expected about {expected}s",
            file.file_name(),
            file.duration
        );
    }
    assert!(temp_files(ws.path()).is_empty());
}

#[test]
fn a_broken_file_fails_alone_without_stopping_the_run() {
    let ws = Workspace::new("resilient");
    let good = ws.make("a.flac", &["-c:a", "flac"]);
    let good_before = std::fs::read(&good).unwrap();

    let mut files = probe::scan_folder(ws.path()).expect("scanning");
    // A file that probes fine but whose declared duration is a lie.
    let mut broken = files[0].clone();
    broken.path = ws.path().join("missing.flac");
    broken.duration = 60.0;
    files.push(broken);

    let report = batch::run(&files, &[], &Config::default(), RunMode::Apply, |_| {});

    assert_eq!(report.processed(), 2);
    assert_eq!(report.failed(), 1, "the missing file must fail");
    assert_eq!(report.changed(), 1, "the good file must still be trimmed");
    assert_ne!(std::fs::read(&good).unwrap(), good_before);
    assert!(temp_files(ws.path()).is_empty());
}

// ---- startup: configuration and CLI overrides (design §2, §3, §12) --------

/// Run the built binary and return its stdout.
fn run_binary(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(args)
        .output()
        .expect("running audioedit");
    assert!(
        output.status.success(),
        "audioedit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_binary_reads_a_config_file_and_lets_the_cli_override_it() {
    let ws = Workspace::new("config");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);

    // Minimum durations longer than the fixture's silences (4s / 7s): nothing qualifies.
    let config = ws.path().join("config.toml");
    std::fs::write(
        &config,
        "[auto_trim]\nbegin_min_duration = 8\nend_min_duration = 8\n",
    )
    .unwrap();

    let folder = ws.path().to_str().unwrap();
    let config_path = config.to_str().unwrap();

    let with_config = run_binary(&["--folder", folder, "--config", config_path, "--dry-run"]);
    assert!(
        with_config.contains("Would change: 0"),
        "config ignored:\n{with_config}"
    );
    assert!(with_config.contains("No-op:     2"));

    // The CLI wins over the file.
    let overridden = run_binary(&[
        "--folder",
        folder,
        "--config",
        config_path,
        "--dry-run",
        "--begin-min-duration",
        "1",
        "--end-min-duration",
        "1",
    ]);
    assert!(
        overridden.contains("Would change: 2"),
        "override ignored:\n{overridden}"
    );
    assert!(overridden.contains("no files were modified"));
}

#[test]
fn the_binary_rejects_an_invalid_configuration_instead_of_guessing() {
    let ws = Workspace::new("badconfig");
    let config = ws.path().join("config.toml");
    std::fs::write(&config, "[playback]\nsmall_seek_seconds = 0\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(["--folder", ws.path().to_str().unwrap()])
        .args(["--config", config.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("running audioedit");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("small_seek_seconds"),
        "unhelpful error: {stderr}"
    );
}

#[test]
fn a_non_interactive_apply_refuses_without_explicit_consent() {
    let ws = Workspace::new("consent");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let before = std::fs::read(&path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(["--folder", ws.path().to_str().unwrap(), "--apply-defaults"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running audioedit");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--yes"), "unhelpful error: {stderr}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file was rewritten anyway"
    );
}

#[test]
fn a_folder_with_no_audio_is_reported_rather_than_failing() {
    let ws = Workspace::new("empty");
    std::fs::write(ws.path().join("notes.txt"), b"nothing here").unwrap();
    let stdout = run_binary(&["--folder", ws.path().to_str().unwrap(), "--dry-run"]);
    assert!(stdout.contains("No supported audio files"), "{stdout}");
}
