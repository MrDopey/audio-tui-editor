//! Re-attaching cover art to containers that cannot mux it as a stream.
//!
//! Ogg (opus, vorbis) can never carry a video stream — ffmpeg's Ogg muxer
//! rejects one outright — but its demuxer already reconstructs an
//! `attached_pic` stream from a `METADATA_BLOCK_PICTURE` Vorbis comment when
//! reading one back. So instead of mapping the picture as a stream, it is
//! extracted once up front and carried as that tag on every attempt.

use std::path::Path;
use std::process::Stdio;

use super::super::{backend_command, ffmpeg_bin};

/// Extracts the attached picture from `path` and returns it as a base64
/// `METADATA_BLOCK_PICTURE` value, ready to pass straight to a
/// `-metadata:s:a:0` argument. Best-effort: any failure (no picture,
/// an image format we don't recognise, ffmpeg erroring) yields `None`
/// rather than failing the save — losing cover art beats losing the file.
pub(super) fn extract_metadata_block_picture(path: &Path) -> Option<String> {
    let output = backend_command(&ffmpeg_bin())
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-c:v",
            "copy",
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.is_empty() {
        return None;
    }
    let mime = sniff_mime(&output.stdout)?;
    Some(base64_encode(&metadata_block_picture(mime, &output.stdout)))
}

/// Identifies an image buffer by its magic bytes rather than trusting the
/// source codec name, since that's what actually determines the MIME type a
/// reader needs to decode it.
fn sniff_mime(data: &[u8]) -> Option<&'static str> {
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

/// Builds a FLAC-style `METADATA_BLOCK_PICTURE` block (type 3 = front cover),
/// as required by the Xiph Vorbis-comment convention for cover art.
///
/// Width, height, colour depth and colour count are left at zero. They are
/// only a hint: readers that care (including ffmpeg's own Ogg demuxer, per
/// end-to-end testing) decode the embedded image itself to get the real
/// values, so zeroing them here is not lossy.
fn metadata_block_picture(mime: &str, data: &[u8]) -> Vec<u8> {
    let mut block = Vec::with_capacity(32 + mime.len() + data.len());
    block.extend_from_slice(&3u32.to_be_bytes()); // picture type: front cover
    block.extend_from_slice(&(mime.len() as u32).to_be_bytes());
    block.extend_from_slice(mime.as_bytes());
    block.extend_from_slice(&0u32.to_be_bytes()); // description length
    block.extend_from_slice(&0u32.to_be_bytes()); // width
    block.extend_from_slice(&0u32.to_be_bytes()); // height
    block.extend_from_slice(&0u32.to_be_bytes()); // colour depth
    block.extend_from_slice(&0u32.to_be_bytes()); // colours used (0 = not palette-indexed)
    block.extend_from_slice(&(data.len() as u32).to_be_bytes());
    block.extend_from_slice(data);
    block
}

/// A standard (RFC 4648) base64 encoder with padding, since pulling in a
/// crate for one call site isn't worth it.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (b0 as u32) << 16 | (b1 as u32) << 8 | b2 as u32;
        out.push(ALPHABET[(n >> 18 & 0x3F) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
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

    #[test]
    fn builds_a_picture_block_with_the_front_cover_type_and_given_mime() {
        let block = metadata_block_picture("image/png", b"fakepngbytes");
        assert_eq!(
            &block[0..4],
            &3u32.to_be_bytes(),
            "picture type: front cover"
        );
        assert_eq!(&block[4..8], &9u32.to_be_bytes(), "mime length");
        assert_eq!(&block[8..17], b"image/png");
        assert!(block.ends_with(b"fakepngbytes"));
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
