//! Extracting an embedded cover-art picture as raw image bytes, for anything
//! that needs to look at it rather than just report its presence
//! (`MediaInfo::has_cover_art`).
//!
//! Two extraction strategies exist because they serve different callers.
//! [`extract_original`] copies the picture's bytes verbatim (`-c:v copy`) —
//! used only by the Ogg re-attach save path (`ffmpeg::cover_art`), which
//! must never lose quality by re-encoding a user's cover art on every save.
//! [`fetch`] (and the uncached [`extract_as_png`] beneath it) forces a PNG
//! re-encode instead, because the terminal-rendering path
//! (`term::kitty::transmit_and_display`) can only pass an image straight
//! through to the terminal for PNG — there is no equivalent passthrough
//! format code for JPEG, which embedded cover art very commonly is.

mod cache;

use std::path::Path;
use std::process::Stdio;

use super::{backend_command, ffmpeg_bin};

/// A cover-art picture as raw, already-encoded image bytes plus its MIME type.
#[derive(Debug, Clone, PartialEq)]
pub struct RawCoverArt {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

/// Cached, display-facing entry point: always PNG (see the module docs),
/// so callers can feed the bytes straight into Kitty's `f=100` passthrough
/// with no further decoding. Best-effort like the rest of cover-art
/// handling: any failure yields `None` rather than propagating an error —
/// a missing thumbnail is never worth interrupting anything over.
pub fn fetch(path: &Path) -> Option<RawCoverArt> {
    let cache_path = cache::cache_path_for(path);
    if let Some(cache_path) = &cache_path {
        if let Some(cached) = cache::read_cache(cache_path) {
            return Some(cached);
        }
    }

    let art = extract_as_png(path)?;

    if let Some(cache_path) = &cache_path {
        // A cache failure must never break extraction.
        let _ = cache::write_cache(cache_path, &art);
    }
    Some(art)
}

/// Uncached: shells out to ffmpeg, forcing a PNG re-encode of the first
/// attached-picture stream.
pub(crate) fn extract_as_png(path: &Path) -> Option<RawCoverArt> {
    let output = run_extraction(path, &["-c:v", "png"])?;
    Some(RawCoverArt {
        bytes: output,
        mime: "image/png",
    })
}

/// Uncached: shells out to ffmpeg, copying the first attached-picture
/// stream's bytes verbatim (no re-encode). Used only by the Ogg re-attach
/// save path, where preserving the user's original picture losslessly
/// matters more than a uniform output format.
pub(crate) fn extract_original(path: &Path) -> Option<RawCoverArt> {
    let output = run_extraction(path, &["-c:v", "copy"])?;
    let mime = sniff_mime(&output)?;
    Some(RawCoverArt {
        bytes: output,
        mime,
    })
}

fn run_extraction(path: &Path, video_codec_args: &[&str]) -> Option<Vec<u8>> {
    let mut command = backend_command(&ffmpeg_bin());
    command
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-map", "0:v:0"])
        .args(video_codec_args)
        .args(["-frames:v", "1", "-f", "image2pipe", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    crate::debug::log_command(&command);
    let output = command.output().ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    Some(output.stdout)
}

/// Identifies an image buffer by its magic bytes rather than trusting the
/// source codec name, since that's what actually determines the MIME type a
/// reader needs to decode it.
pub(crate) fn sniff_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_known_image_formats() {
        assert_eq!(
            sniff_mime(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 0, 0]),
            Some("image/png")
        );
        assert_eq!(sniff_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"GIF89afoo"), Some("image/gif"));
        assert_eq!(sniff_mime(b"not an image"), None);
    }
}
