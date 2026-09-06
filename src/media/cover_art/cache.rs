//! On-disk cache for extracted cover-art bytes, keyed by path, size and
//! mtime so a changed file never reads back a stale image — mirrors
//! `media::waveform::cache` exactly.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::RawCoverArt;

const CACHE_MAGIC: &[u8; 4] = b"AECA";
const CACHE_VERSION: u32 = 1;

pub(super) fn cache_path_for(path: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .hash(&mut hasher);
    modified.hash(&mut hasher);
    meta.len().hash(&mut hasher);
    CACHE_VERSION.hash(&mut hasher);

    let dir = dirs::cache_dir()?.join("audioedit").join("cover_art");
    Some(dir.join(format!("{:016x}.art", hasher.finish())))
}

pub(super) fn read_cache(path: &Path) -> Option<RawCoverArt> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 16 || &bytes[0..4] != CACHE_MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != CACHE_VERSION {
        return None;
    }
    let mime_len = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let mime_start: usize = 12;
    let mime_end = mime_start.checked_add(mime_len)?;
    let mime_str = std::str::from_utf8(bytes.get(mime_start..mime_end)?).ok()?;
    let mime = static_mime(mime_str)?;

    let len_start = mime_end;
    let len_end = len_start.checked_add(8)?;
    let data_len = u64::from_le_bytes(bytes.get(len_start..len_end)?.try_into().ok()?) as usize;
    let data = bytes.get(len_end..)?;
    if data.len() != data_len {
        return None;
    }
    Some(RawCoverArt {
        bytes: data.to_vec(),
        mime,
    })
}

pub(super) fn write_cache(path: &Path, art: &RawCoverArt) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mime_bytes = art.mime.as_bytes();
    let mut bytes = Vec::with_capacity(16 + mime_bytes.len() + art.bytes.len());
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(mime_bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(mime_bytes);
    bytes.extend_from_slice(&(art.bytes.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&art.bytes);

    // Write via a temporary so a crash cannot leave a truncated cache entry.
    // The process id keeps two concurrent instances analysing the same file
    // from racing on the same temporary path.
    let tmp = path.with_extension(format!("art.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Maps a stored MIME string back to one of `RawCoverArt::mime`'s `&'static`
/// values, so a corrupt/foreign cache entry (an unrecognised MIME string)
/// is rejected rather than fabricating a `&'static str` from stored bytes.
fn static_mime(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art(bytes: Vec<u8>) -> RawCoverArt {
        RawCoverArt {
            bytes,
            mime: "image/png",
        }
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("audioedit-art-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.art");
        let original = art(vec![1, 2, 3, 4, 5]);
        write_cache(&path, &original).unwrap();
        let restored = read_cache(&path).unwrap();
        assert_eq!(restored, original);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_cache_is_ignored() {
        let dir = std::env::temp_dir().join(format!("audioedit-art-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.art");
        std::fs::write(&path, b"not a cover art cache").unwrap();
        assert!(read_cache(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
