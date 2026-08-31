//! The ffmpeg-backed decoder that feeds [`super::AudioPlayer`] raw samples.

use std::io::Read;
use std::num::NonZero;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

use rodio::source::Source;

use crate::media::ffmpeg_bin;

use super::{CHUNK_SAMPLES, OUTPUT_CHANNELS, OUTPUT_RATE};

/// A rodio [`Source`] fed by an ffmpeg process decoding to `f32le`.
pub(super) struct FfmpegSource {
    child: Child,
    stdout: ChildStdout,
    buffer: Vec<f32>,
    cursor: usize,
    finished: bool,
    remaining: Duration,
}

impl FfmpegSource {
    pub(super) fn spawn(path: &Path, offset: f64, remaining: f64) -> Option<FfmpegSource> {
        let mut child = Command::new(ffmpeg_bin())
            .args(["-v", "error", "-nostdin"])
            .arg("-ss")
            .arg(format!("{offset:.6}"))
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                "0:a:0",
                "-f",
                "f32le",
                "-ac",
                &OUTPUT_CHANNELS.to_string(),
                "-ar",
                &OUTPUT_RATE.to_string(),
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        Some(FfmpegSource {
            child,
            stdout,
            buffer: Vec::with_capacity(CHUNK_SAMPLES),
            cursor: 0,
            finished: false,
            remaining: Duration::from_secs_f64(remaining.max(0.0)),
        })
    }

    /// Refill `buffer` from the decoder. Returns false at end of stream.
    fn refill(&mut self) -> bool {
        let mut bytes = vec![0u8; CHUNK_SAMPLES * 4];
        let mut filled = 0;
        // Read until a whole number of samples is available or the pipe ends.
        while filled < bytes.len() {
            match self.stdout.read(&mut bytes[filled..]) {
                Ok(0) => break,
                Ok(n) => {
                    filled += n;
                    if filled % 4 == 0 {
                        break;
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let samples = filled / 4;
        if samples == 0 {
            self.finished = true;
            return false;
        }
        self.buffer.clear();
        let (frames, _) = bytes[..samples * 4].as_chunks::<4>();
        self.buffer
            .extend(frames.iter().copied().map(f32::from_le_bytes));
        self.cursor = 0;
        true
    }
}

impl Iterator for FfmpegSource {
    type Item = f32;

    #[inline]
    fn next(&mut self) -> Option<f32> {
        if self.cursor >= self.buffer.len() && (self.finished || !self.refill()) {
            return None;
        }
        let sample = self.buffer[self.cursor];
        self.cursor += 1;
        Some(sample)
    }
}

impl Source for FfmpegSource {
    fn current_span_len(&self) -> Option<usize> {
        // A continuous stream with fixed rate and channel count.
        None
    }

    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(OUTPUT_CHANNELS).expect("channel count is non-zero")
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(OUTPUT_RATE).expect("sample rate is non-zero")
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.remaining)
    }
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        // The decoder must not outlive the source that was reading it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command as Proc;

    /// A short stereo file to decode. Returns its path inside a fresh folder.
    fn fixture(name: &str, seconds: u32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("audioedit-player-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creating the fixture folder");
        let path = dir.join("tone.wav");
        let status = Proc::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "sine=frequency=440:sample_rate=48000:duration={seconds}"
            ))
            .args(["-ac", "2"])
            .arg(&path)
            .status()
            .expect("running ffmpeg");
        assert!(status.success());
        path
    }

    fn cleanup(path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn the_decoder_yields_the_expected_number_of_samples() {
        let path = fixture("full", 2);
        let source = FfmpegSource::spawn(&path, 0.0, 2.0).expect("spawning the decoder");
        assert_eq!(source.channels().get(), OUTPUT_CHANNELS);
        assert_eq!(source.sample_rate().get(), OUTPUT_RATE);

        let count = source.count();
        let expected = 2 * OUTPUT_RATE as usize * OUTPUT_CHANNELS as usize;
        let drift = count.abs_diff(expected);
        assert!(
            drift * 100 < expected,
            "decoded {count} samples, expected about {expected}"
        );
        cleanup(&path);
    }

    #[test]
    fn seeking_starts_the_decoder_at_the_requested_offset() {
        let path = fixture("offset", 2);
        let source = FfmpegSource::spawn(&path, 1.5, 0.5).expect("spawning the decoder");
        let count = source.count();
        let expected = OUTPUT_RATE as usize * OUTPUT_CHANNELS as usize / 2;
        let drift = count.abs_diff(expected);
        assert!(
            drift * 20 < expected,
            "decoded {count} samples, expected about {expected}"
        );
        cleanup(&path);
    }

    #[test]
    fn the_decoder_reports_its_remaining_duration() {
        let path = fixture("duration", 2);
        let source = FfmpegSource::spawn(&path, 0.5, 1.5).expect("spawning the decoder");
        assert_eq!(source.total_duration(), Some(Duration::from_secs_f64(1.5)));
        cleanup(&path);
    }

    #[test]
    fn dropping_the_source_stops_the_decoder() {
        let path = fixture("drop", 30);
        let source = FfmpegSource::spawn(&path, 0.0, 30.0).expect("spawning the decoder");
        let pid = source.child.id();
        drop(source);
        // The process must be reaped, not left running against a closed pipe.
        let alive = Proc::new("kill").args(["-0", &pid.to_string()]).status();
        assert!(
            alive.map(|s| !s.success()).unwrap_or(true),
            "ffmpeg outlived its source"
        );
        cleanup(&path);
    }
}
