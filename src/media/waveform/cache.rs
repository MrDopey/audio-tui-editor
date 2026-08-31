//! On-disk cache for waveform analysis, keyed by path, size and mtime so a
//! changed file never reads back stale buckets.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::{Waveform, ANALYSIS_RATE, MAX_BUCKETS};

const CACHE_MAGIC: &[u8; 4] = b"AEWF";
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
    ANALYSIS_RATE.hash(&mut hasher);
    MAX_BUCKETS.hash(&mut hasher);
    CACHE_VERSION.hash(&mut hasher);

    let dir = dirs::cache_dir()?.join("audioedit").join("waveform");
    Some(dir.join(format!("{:016x}.wf", hasher.finish())))
}

pub(super) fn read_cache(path: &Path) -> Option<Waveform> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 20 || &bytes[0..4] != CACHE_MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != CACHE_VERSION {
        return None;
    }
    let duration = f64::from_le_bytes(bytes[8..16].try_into().ok()?);
    let count = u32::from_le_bytes(bytes[16..20].try_into().ok()?) as usize;
    if bytes.len() != 20 + count * 8 {
        return None;
    }
    let mut peaks = Vec::with_capacity(count);
    let mut rms = Vec::with_capacity(count);
    for i in 0..count {
        let off = 20 + i * 8;
        peaks.push(f32::from_le_bytes(bytes[off..off + 4].try_into().ok()?));
        rms.push(f32::from_le_bytes(bytes[off + 4..off + 8].try_into().ok()?));
    }
    Some(Waveform {
        duration,
        peaks,
        rms,
    })
}

pub(super) fn write_cache(path: &Path, waveform: &Waveform) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut bytes = Vec::with_capacity(20 + waveform.peaks.len() * 8);
    bytes.extend_from_slice(CACHE_MAGIC);
    bytes.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    bytes.extend_from_slice(&waveform.duration.to_le_bytes());
    bytes.extend_from_slice(&(waveform.peaks.len() as u32).to_le_bytes());
    for (peak, rms) in waveform.peaks.iter().zip(&waveform.rms) {
        bytes.extend_from_slice(&peak.to_le_bytes());
        bytes.extend_from_slice(&rms.to_le_bytes());
    }
    // Write via a temporary so a crash cannot leave a truncated cache entry.
    // The process id keeps two concurrent instances analysing the same file
    // from racing on the same temporary path.
    let tmp = path.with_extension(format!("wf.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waveform(peaks: Vec<f32>) -> Waveform {
        let rms = peaks.iter().map(|p| p * 0.5).collect();
        Waveform {
            duration: 10.0,
            peaks,
            rms,
        }
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join(format!("audioedit-wf-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.wf");
        let original = waveform(vec![0.0, 0.25, 0.5, 1.0]);
        write_cache(&path, &original).unwrap();
        let restored = read_cache(&path).unwrap();
        assert_eq!(restored.peaks, original.peaks);
        assert_eq!(restored.rms, original.rms);
        assert_eq!(restored.duration, original.duration);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_cache_is_ignored() {
        let dir = std::env::temp_dir().join(format!("audioedit-wf-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.wf");
        std::fs::write(&path, b"not a waveform cache").unwrap();
        assert!(read_cache(&path).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
