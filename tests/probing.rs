//! Folder scanning and per-file probing (design §5, §21).

mod common;

use audioedit::media::probe::{self, MediaInfo};

use common::Workspace;

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
