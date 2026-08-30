//! The media backend: everything that shells out to ffmpeg/ffprobe.

pub mod autotrim;
pub mod ffmpeg;
pub mod probe;
pub mod waveform;

use std::process::Command;

use anyhow::{Context, Result};

/// Path to the `ffmpeg` binary, overridable for unusual installs.
pub fn ffmpeg_bin() -> String {
    std::env::var("AUDIOEDIT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// Path to the `ffprobe` binary, overridable for unusual installs.
pub fn ffprobe_bin() -> String {
    std::env::var("AUDIOEDIT_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string())
}

/// Verify both tools are present before the TUI takes over the terminal.
pub fn ensure_backend_available() -> Result<()> {
    for bin in [ffmpeg_bin(), ffprobe_bin()] {
        Command::new(&bin)
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| {
                format!(
                    "could not run `{bin}`. audioedit needs ffmpeg and ffprobe on PATH \
                     (set AUDIOEDIT_FFMPEG / AUDIOEDIT_FFPROBE to override)"
                )
            })?;
    }
    Ok(())
}

/// Trim ffmpeg's stderr down to something worth showing a user.
pub fn tail_of(stderr: &str, lines: usize) -> String {
    let collected: Vec<&str> = stderr
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join("\n")
}
