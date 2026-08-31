//! Automatic silence detection for the leading/trailing markers (design §11).

mod common;

use audioedit::config::Config;
use audioedit::media::autotrim;

use common::{probe_ok, Workspace};

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
