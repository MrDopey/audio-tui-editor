//! Waveform analysis against real audio (design §7).

mod common;

use audioedit::media::waveform;

use common::{probe_ok, Workspace};

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
