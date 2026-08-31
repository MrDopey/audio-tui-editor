//! Waveform analysis (design §7).
//!
//! The file is decoded once to low-rate mono PCM and reduced to a bounded set
//! of peak/RMS buckets. Buckets are cached on disk and downsampled to the
//! terminal width on each draw, so playback updates never recompute anything.

mod cache;

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::{ffmpeg_bin, missing_backend_hint, tail_of};

/// Decode rate. Low enough to be quick on long files, high enough for a
/// faithful amplitude envelope.
const ANALYSIS_RATE: u32 = 8_000;
/// Upper bound on stored buckets; resolution halves whenever it is reached.
const MAX_BUCKETS: usize = 8_192;
/// Samples per bucket before any halving (~20 ms of audio).
const BASE_SAMPLES_PER_BUCKET: usize = 160;

#[derive(Debug, Clone)]
pub struct Waveform {
    pub duration: f64,
    /// Peak amplitude per bucket, normalised to `0.0..=1.0`.
    pub peaks: Vec<f32>,
    /// RMS amplitude per bucket, normalised to `0.0..=1.0`.
    pub rms: Vec<f32>,
}

impl Waveform {
    /// Reduce to exactly `width` columns of `(peak, rms)` for rendering.
    pub fn downsample(&self, width: usize) -> Vec<(f32, f32)> {
        if width == 0 {
            return Vec::new();
        }
        if self.peaks.is_empty() {
            return vec![(0.0, 0.0); width];
        }
        let n = self.peaks.len();
        (0..width)
            .map(|col| {
                let start = col * n / width;
                let end = (((col + 1) * n).div_ceil(width)).min(n).max(start + 1);
                let mut peak = 0.0f32;
                let mut square_sum = 0.0f64;
                for i in start..end {
                    peak = peak.max(self.peaks[i]);
                    square_sum += (self.rms[i] as f64).powi(2);
                }
                let rms = (square_sum / (end - start) as f64).sqrt() as f32;
                (peak, rms)
            })
            .collect()
    }
}

/// Compute the waveform, reading from (and populating) the on-disk cache.
pub fn analyse(path: &Path, duration: f64) -> Result<Waveform> {
    let cache_path = cache::cache_path_for(path);
    if let Some(cache_path) = &cache_path {
        if let Some(cached) = cache::read_cache(cache_path) {
            return Ok(cached);
        }
    }

    let waveform = decode(path, duration)?;

    if let Some(cache_path) = &cache_path {
        // A cache failure must never break analysis.
        let _ = cache::write_cache(cache_path, &waveform);
    }
    Ok(waveform)
}

fn decode(path: &Path, duration: f64) -> Result<Waveform> {
    let mut child = Command::new(ffmpeg_bin())
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-ac",
            "1",
            "-ar",
            &ANALYSIS_RATE.to_string(),
            "-f",
            "s16le",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .with_context(|| {
            format!(
                "decoding {} for waveform analysis. {}",
                path.display(),
                missing_backend_hint(&ffmpeg_bin())
            )
        })?;

    let mut stdout = child.stdout.take().expect("stdout was piped");

    let mut peaks: Vec<f32> = Vec::new();
    let mut squares: Vec<f64> = Vec::new();
    let mut samples_per_bucket = BASE_SAMPLES_PER_BUCKET;

    let mut bucket_peak = 0.0f32;
    let mut bucket_square = 0.0f64;
    let mut bucket_count = 0usize;

    let mut buf = vec![0u8; 64 * 1024];
    let mut carry: Option<u8> = None;

    loop {
        let read = stdout.read(&mut buf).context("reading decoded audio")?;
        if read == 0 {
            break;
        }
        let mut chunk = &buf[..read];

        // s16le frames can straddle read boundaries.
        if let Some(low) = carry.take() {
            if let Some((&high, rest)) = chunk.split_first() {
                push_sample(
                    i16::from_le_bytes([low, high]),
                    &mut bucket_peak,
                    &mut bucket_square,
                    &mut bucket_count,
                );
                chunk = rest;
            } else {
                carry = Some(low);
                continue;
            }
        }

        let pairs = chunk.len() / 2;
        for i in 0..pairs {
            let sample = i16::from_le_bytes([chunk[2 * i], chunk[2 * i + 1]]);
            push_sample(
                sample,
                &mut bucket_peak,
                &mut bucket_square,
                &mut bucket_count,
            );
            if bucket_count >= samples_per_bucket {
                peaks.push(bucket_peak);
                squares.push(bucket_square / bucket_count as f64);
                bucket_peak = 0.0;
                bucket_square = 0.0;
                bucket_count = 0;

                if peaks.len() >= MAX_BUCKETS {
                    halve(&mut peaks, &mut squares);
                    samples_per_bucket *= 2;
                }
            }
        }
        if chunk.len() % 2 == 1 {
            carry = Some(chunk[chunk.len() - 1]);
        }
    }

    if bucket_count > 0 {
        peaks.push(bucket_peak);
        squares.push(bucket_square / bucket_count as f64);
    }

    let status = child.wait().context("waiting for ffmpeg")?;
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut handle) = child.stderr.take() {
            let _ = handle.read_to_string(&mut stderr);
        }
        bail!(
            "ffmpeg could not decode {}: {}",
            path.display(),
            tail_of(&stderr, 3)
        );
    }

    let rms = squares.iter().map(|s| s.sqrt() as f32).collect();
    Ok(Waveform {
        duration,
        peaks,
        rms,
    })
}

#[inline]
fn push_sample(sample: i16, peak: &mut f32, square: &mut f64, count: &mut usize) {
    let value = (sample as f32 / i16::MAX as f32).abs().min(1.0);
    if value > *peak {
        *peak = value;
    }
    *square += (value as f64) * (value as f64);
    *count += 1;
}

/// Merge adjacent bucket pairs, halving resolution in place.
fn halve(peaks: &mut Vec<f32>, squares: &mut Vec<f64>) {
    let merged = peaks.len() / 2;
    for i in 0..merged {
        peaks[i] = peaks[2 * i].max(peaks[2 * i + 1]);
        squares[i] = (squares[2 * i] + squares[2 * i + 1]) / 2.0;
    }
    if peaks.len() % 2 == 1 {
        peaks[merged] = peaks[peaks.len() - 1];
        squares[merged] = squares[squares.len() - 1];
        peaks.truncate(merged + 1);
        squares.truncate(merged + 1);
    } else {
        peaks.truncate(merged);
        squares.truncate(merged);
    }
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
    fn downsample_produces_exactly_the_requested_width() {
        let w = waveform((0..1000).map(|i| i as f32 / 1000.0).collect());
        for width in [1usize, 7, 80, 1000, 1500] {
            assert_eq!(w.downsample(width).len(), width, "width {width}");
        }
    }

    #[test]
    fn downsample_keeps_peaks() {
        let mut peaks = vec![0.0f32; 100];
        peaks[42] = 1.0;
        let columns = waveform(peaks).downsample(10);
        assert_eq!(columns[4].0, 1.0);
        assert_eq!(columns[0].0, 0.0);
    }

    #[test]
    fn downsample_handles_empty_analysis() {
        let w = Waveform {
            duration: 0.0,
            peaks: vec![],
            rms: vec![],
        };
        assert_eq!(w.downsample(5), vec![(0.0, 0.0); 5]);
        assert!(w.downsample(0).is_empty());
    }

    #[test]
    fn halving_preserves_peaks_and_length() {
        let mut peaks = vec![0.1, 0.9, 0.2, 0.3, 0.7];
        let mut squares = vec![0.01, 0.81, 0.04, 0.09, 0.49];
        halve(&mut peaks, &mut squares);
        assert_eq!(peaks, vec![0.9, 0.3, 0.7]);
        assert_eq!(squares.len(), 3);
    }
}
