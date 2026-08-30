//! Playback (design §6).
//!
//! Decoding and mixing are delegated to existing libraries: ffmpeg decodes to
//! raw PCM (so every format ffprobe accepts is playable) and rodio owns the
//! device, mixing and volume. Seeking restarts the decoder at the new offset.

use std::io::Read;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use rodio::source::Source;

use crate::media::ffmpeg_bin;

const OUTPUT_RATE: u32 = 48_000;
const OUTPUT_CHANNELS: u16 = 2;
/// Samples pulled from the decoder at a time.
const CHUNK_SAMPLES: usize = 8_192;

/// The audio device, opened once for the lifetime of the application.
///
/// Opening can legitimately fail (a headless machine, no sound server). The
/// application stays fully usable in that case: playback becomes a silent
/// transport so browsing, marker editing and saving all still work.
pub struct AudioOutput {
    sink: Option<rodio::MixerDeviceSink>,
    error: Option<String>,
}

impl AudioOutput {
    pub fn open() -> Self {
        match rodio::DeviceSinkBuilder::open_default_sink() {
            Ok(sink) => AudioOutput {
                sink: Some(sink),
                error: None,
            },
            Err(err) => AudioOutput {
                sink: None,
                error: Some(err.to_string()),
            },
        }
    }

    /// An output with no device, used on headless machines and in tests.
    pub fn silent() -> Self {
        AudioOutput {
            sink: None,
            error: Some("no audio device was requested".to_string()),
        }
    }

    pub fn is_available(&self) -> bool {
        self.sink.is_some()
    }

    /// Why the device could not be opened, if it could not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// Transport state for one file.
pub struct AudioPlayer {
    path: PathBuf,
    duration: f64,
    /// Volume as a percentage, matching the configured `volume_step`.
    volume: f64,
    playing: bool,
    /// Offset at which the current decoder was started.
    base: f64,
    /// Position bookkeeping used when there is no audio device.
    silent_position: f64,
    silent_since: Option<Instant>,
    sink: Option<rodio::Player>,
}

impl AudioPlayer {
    pub fn new(output: &AudioOutput, path: &Path, duration: f64, volume: f64) -> Self {
        let sink = output.sink.as_ref().map(|device| {
            let player = rodio::Player::connect_new(device.mixer());
            player.pause();
            player
        });
        let mut player = AudioPlayer {
            path: path.to_path_buf(),
            duration,
            volume,
            playing: false,
            base: 0.0,
            silent_position: 0.0,
            silent_since: None,
            sink,
        };
        player.apply_volume();
        player.load_from(0.0);
        player
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Current playback position in seconds.
    pub fn position(&self) -> f64 {
        // While paused the stored position is authoritative: the device's own
        // counter is only updated by the audio thread and reads stale for a
        // few milliseconds after a seek replaces the decoder.
        let raw = if !self.playing {
            self.silent_position
        } else {
            match &self.sink {
                Some(sink) => self.base + sink.get_pos().as_secs_f64(),
                None => {
                    let elapsed = self.silent_since.map_or(0.0, |t| t.elapsed().as_secs_f64());
                    self.silent_position + elapsed
                }
            }
        };
        raw.clamp(0.0, self.duration)
    }

    /// True once playback has run past the end of the file (design §6).
    pub fn at_end(&self) -> bool {
        match &self.sink {
            Some(sink) => sink.empty() || self.position() >= self.duration - 0.05,
            None => self.position() >= self.duration - 0.05,
        }
    }

    pub fn play(&mut self) {
        if self.playing {
            return;
        }
        // Restarting from the end is friendlier than refusing to play.
        if self.at_end() {
            self.seek_to(0.0);
        }
        self.playing = true;
        self.silent_since = Some(Instant::now());
        if let Some(sink) = &self.sink {
            sink.play();
        }
    }

    pub fn pause(&mut self) {
        if !self.playing {
            return;
        }
        self.silent_position = self.position();
        self.silent_since = None;
        self.playing = false;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
    }

    pub fn toggle(&mut self) {
        if self.playing {
            self.pause();
        } else {
            self.play();
        }
    }

    /// Seek to an absolute position, clamped to the file.
    pub fn seek_to(&mut self, seconds: f64) {
        let target = seconds.clamp(0.0, self.duration);
        let was_playing = self.playing;
        self.load_from(target);
        if was_playing {
            self.playing = true;
            self.silent_since = Some(Instant::now());
            if let Some(sink) = &self.sink {
                sink.play();
            }
        }
    }

    /// Seek by a relative amount.
    pub fn seek_by(&mut self, delta: f64) {
        self.seek_to(self.position() + delta);
    }

    pub fn volume(&self) -> f64 {
        self.volume
    }

    /// Set volume as a percentage, clamped to `0..=150`.
    pub fn set_volume(&mut self, percent: f64) {
        self.volume = percent.clamp(0.0, 150.0);
        self.apply_volume();
    }

    pub fn adjust_volume(&mut self, delta: f64) {
        self.set_volume(self.volume + delta);
    }

    fn apply_volume(&self) {
        if let Some(sink) = &self.sink {
            sink.set_volume((self.volume / 100.0) as f32);
        }
    }

    /// Point the decoder at `offset` and leave the transport paused.
    fn load_from(&mut self, offset: f64) {
        self.base = offset;
        self.silent_position = offset;
        self.silent_since = None;
        self.playing = false;

        let Some(sink) = &self.sink else {
            return;
        };
        sink.clear();
        let remaining = (self.duration - offset).max(0.0);
        if let Some(source) = FfmpegSource::spawn(&self.path, offset, remaining) {
            sink.append(source);
        }
        sink.pause();
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        if let Some(sink) = &self.sink {
            sink.stop();
        }
    }
}

/// A rodio [`Source`] fed by an ffmpeg process decoding to `f32le`.
struct FfmpegSource {
    child: Child,
    stdout: ChildStdout,
    buffer: Vec<f32>,
    cursor: usize,
    finished: bool,
    remaining: Duration,
}

impl FfmpegSource {
    fn spawn(path: &Path, offset: f64, remaining: f64) -> Option<FfmpegSource> {
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

    #[test]
    fn a_silent_player_still_tracks_position_and_volume() {
        let path = fixture("silent", 2);
        let output = AudioOutput::silent();
        let mut player = AudioPlayer::new(&output, &path, 600.0, 100.0);

        assert_eq!(player.position(), 0.0);
        assert!(!player.is_playing());

        player.seek_to(120.0);
        assert_eq!(player.position(), 120.0);
        player.seek_by(-30.0);
        assert_eq!(player.position(), 90.0);

        // Seeking is clamped to the file.
        player.seek_by(-1000.0);
        assert_eq!(player.position(), 0.0);
        player.seek_to(10_000.0);
        assert_eq!(player.position(), 600.0);
        assert!(player.at_end());

        player.set_volume(50.0);
        assert_eq!(player.volume(), 50.0);
        player.adjust_volume(-100.0);
        assert_eq!(player.volume(), 0.0, "volume is clamped at zero");
        player.adjust_volume(1000.0);
        assert_eq!(player.volume(), 150.0, "volume is capped");
        cleanup(&path);
    }

    #[test]
    fn playing_from_the_end_restarts_from_the_beginning() {
        let path = fixture("restart", 2);
        let output = AudioOutput::silent();
        let mut player = AudioPlayer::new(&output, &path, 60.0, 100.0);
        player.seek_to(60.0);
        assert!(player.at_end());
        player.play();
        assert!(player.is_playing());
        assert!(
            player.position() < 1.0,
            "playback should restart from the start"
        );
        player.pause();
        assert!(!player.is_playing());
        cleanup(&path);
    }
}
