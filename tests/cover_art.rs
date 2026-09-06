//! Cover-art extraction for display (see `.tmp/cover-art.plan`): unlike the
//! save pipeline's `-c:v copy`, this path forces a PNG re-encode, since the
//! Kitty graphics protocol can only pass an image straight through
//! (`f=100`) for PNG — there is no equivalent for JPEG, which embedded
//! cover art very commonly is.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use audioedit::media::cover_art;

use common::Workspace;

/// Builds a single-frame cover image. The extension in `name` picks the
/// encoder ffmpeg infers from it (`.png` -> PNG, `.jpg` -> MJPEG/JPEG).
fn build_cover(ws: &Workspace, name: &str, color: &str) -> PathBuf {
    let path = ws.path().join(name);
    let ok = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=32x32:d=1"),
            "-frames:v",
            "1",
        ])
        .arg(&path)
        .status()
        .expect("running ffmpeg")
        .success();
    assert!(ok, "could not build cover image {name}");
    path
}

/// Attaches `cover` to a freshly built mp3 fixture as an `attached_pic`
/// video stream, the same way real tagging tools do it.
fn attach_cover(ws: &Workspace, audio_name: &str, cover: &Path) -> PathBuf {
    let audio = ws.make(audio_name, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    let with_cover = ws.path().join(format!("with-{audio_name}"));
    let ok = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-i"])
        .arg(&audio)
        .arg("-i")
        .arg(cover)
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
    assert!(ok, "could not attach cover art to {audio_name}");
    with_cover
}

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

#[test]
fn a_jpeg_cover_is_returned_as_png_for_display() {
    // Regression test for the forced-re-encode finding: Kitty's PNG
    // passthrough can't handle JPEG, so the display path must never hand
    // back the original JPEG bytes verbatim.
    let ws = Workspace::new("cover_jpeg_display");
    let cover = build_cover(&ws, "cover.jpg", "green");
    let with_cover = attach_cover(&ws, "a.mp3", &cover);

    let art = cover_art::fetch(&with_cover).expect("should extract cover art");
    assert_eq!(art.mime, "image/png");
    assert!(art.bytes.starts_with(PNG_MAGIC), "bytes are not PNG-magic");
}

#[test]
fn a_png_cover_is_returned_as_png_for_display() {
    let ws = Workspace::new("cover_png_display");
    let cover = build_cover(&ws, "cover.png", "blue");
    let with_cover = attach_cover(&ws, "b.mp3", &cover);

    let art = cover_art::fetch(&with_cover).expect("should extract cover art");
    assert_eq!(art.mime, "image/png");
    assert!(art.bytes.starts_with(PNG_MAGIC), "bytes are not PNG-magic");
}

#[test]
fn a_file_with_no_cover_art_returns_none() {
    let ws = Workspace::new("cover_none");
    let audio = ws.make("plain.mp3", &["-c:a", "libmp3lame", "-b:a", "128k"]);
    assert!(cover_art::fetch(&audio).is_none());
}

#[test]
fn fetching_the_same_file_twice_is_deterministic() {
    let ws = Workspace::new("cover_cache");
    let cover = build_cover(&ws, "cover.png", "red");
    let with_cover = attach_cover(&ws, "c.mp3", &cover);

    let first = cover_art::fetch(&with_cover).expect("first fetch");
    let second = cover_art::fetch(&with_cover).expect("second fetch, from the on-disk cache");
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.mime, second.mime);
}
