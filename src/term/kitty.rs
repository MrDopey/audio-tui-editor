//! Hand-rolled Kitty terminal graphics protocol: just enough to transmit a
//! PNG and display it scaled into a fixed cell area, and to delete it again.
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/>.
//!
//! Every command sets `q=2`, which suppresses the terminal's OK/error
//! response entirely — callers never read anything back, so the response
//! can never be misread as a keystroke by `crossterm`'s input loop.

use ratatui::layout::Rect;

/// A single, fixed placement id: this app only ever shows one cover-art
/// image at a time, so there is no need for per-process randomisation.
const IMAGE_ID: u32 = 1;

/// Max bytes of (already base64-encoded) payload per escape-sequence chunk,
/// per the protocol's own limit.
const CHUNK_SIZE: usize = 4096;

const APC_START: &[u8] = b"\x1b_G";
const APC_END: &[u8] = b"\x1b\\";

/// Builds the full multi-chunk escape sequence to transmit `png_bytes` and
/// display it scaled to fill exactly `area`'s cell dimensions. The cursor is
/// saved, moved to `area`'s top-left corner (1-indexed, as terminal cursor
/// positioning requires), and restored afterward, so this never disturbs
/// wherever ratatui's own cursor bookkeeping expects the cursor to be.
pub fn transmit_and_display(png_bytes: &[u8], area: Rect) -> Vec<u8> {
    let payload = crate::base64::encode(png_bytes);
    let cols = area.width.max(1);
    let rows = area.height.max(1);

    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b7"); // DECSC: save cursor position
    out.extend_from_slice(format!("\x1b[{};{}H", area.y + 1, area.x + 1).as_bytes());

    let chunks = payload_chunks(&payload);
    let last = chunks.len().saturating_sub(1);
    for (index, chunk) in chunks.into_iter().enumerate() {
        let more = if index == last { 0 } else { 1 };
        out.extend_from_slice(APC_START);
        if index == 0 {
            out.extend_from_slice(
                format!("a=T,f=100,i={IMAGE_ID},q=2,C=1,c={cols},r={rows},m={more}").as_bytes(),
            );
        } else {
            out.extend_from_slice(format!("m={more},q=2").as_bytes());
        }
        out.push(b';');
        out.extend_from_slice(chunk);
        out.extend_from_slice(APC_END);
    }

    out.extend_from_slice(b"\x1b8"); // DECRC: restore cursor position
    out
}

/// Deletes only this app's own placement (`d=i`, scoped by `i=IMAGE_ID`),
/// never other programs' images sharing the same terminal. Safe to call
/// speculatively even if nothing is currently placed.
pub fn delete_own_placement() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(APC_START);
    out.extend_from_slice(format!("a=d,d=i,i={IMAGE_ID},q=2").as_bytes());
    out.extend_from_slice(APC_END);
    out
}

/// Splits an already-base64-encoded payload into `CHUNK_SIZE`-byte pieces,
/// always yielding at least one (possibly empty) chunk so a degenerate
/// empty image still produces one well-formed transmit command.
fn payload_chunks(payload: &str) -> Vec<&[u8]> {
    if payload.is_empty() {
        return vec![&[]];
    }
    payload.as_bytes().chunks(CHUNK_SIZE).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: &[u8]) -> &str {
        std::str::from_utf8(bytes).expect("escape sequences are ASCII")
    }

    #[test]
    fn delete_own_placement_is_exact() {
        assert_eq!(text(&delete_own_placement()), "\x1b_Ga=d,d=i,i=1,q=2\x1b\\");
    }

    #[test]
    fn a_small_image_fits_in_a_single_chunk() {
        let area = Rect::new(2, 3, 10, 5);
        let bytes = transmit_and_display(b"tiny", area);
        let expected_payload = crate::base64::encode(b"tiny");
        let expected = format!(
            "\x1b7\x1b[4;3H\x1b_Ga=T,f=100,i=1,q=2,C=1,c=10,r=5,m=0;{expected_payload}\x1b\\\x1b8"
        );
        assert_eq!(text(&bytes), expected);
    }

    #[test]
    fn a_large_image_splits_into_multiple_chunks_with_correct_more_flags() {
        // 7000 raw bytes base64-encodes to well over 8192 characters, so
        // this must split into 3 chunks at CHUNK_SIZE=4096.
        let data = vec![0xABu8; 7000];
        let area = Rect::new(0, 0, 20, 8);
        let bytes = transmit_and_display(&data, area);
        let text = text(&bytes);

        assert!(text.starts_with("\x1b7\x1b[1;1H"), "cursor save+position");
        assert!(text.ends_with("\x1b8"), "cursor restore");

        let full_payload = crate::base64::encode(&data);
        assert!(full_payload.len() > 8192, "test fixture too small");

        let first_header = "\x1b_Ga=T,f=100,i=1,q=2,C=1,c=20,r=8,m=1;";
        assert!(
            text.contains(first_header),
            "first chunk must carry the full key set with m=1"
        );

        // Every payload segment, concatenated in order, must reconstruct
        // the exact base64 of the input.
        let mut reconstructed = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("\x1b_G") {
            rest = &rest[start + 3..];
            let Some(semi) = rest.find(';') else {
                break;
            };
            let after_header = &rest[semi + 1..];
            let Some(end) = after_header.find("\x1b\\") else {
                break;
            };
            reconstructed.push_str(&after_header[..end]);
            rest = &after_header[end..];
        }
        assert_eq!(reconstructed, full_payload);

        let last_header = "\x1b_Gm=0,q=2;";
        assert!(text.contains(last_header), "last chunk must carry m=0");
        let middle_header = "\x1b_Gm=1,q=2;";
        assert!(
            text.contains(middle_header),
            "a middle chunk must carry m=1 and no key set beyond m/q"
        );
    }

    #[test]
    fn an_empty_image_still_produces_one_well_formed_command() {
        let area = Rect::new(0, 0, 4, 4);
        let bytes = transmit_and_display(b"", area);
        assert_eq!(
            text(&bytes),
            "\x1b7\x1b[1;1H\x1b_Ga=T,f=100,i=1,q=2,C=1,c=4,r=4,m=0;\x1b\\\x1b8"
        );
    }
}
