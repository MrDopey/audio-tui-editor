//! The media backend: everything that shells out to ffmpeg/ffprobe.

pub mod autotrim;
pub mod ffmpeg;
pub mod probe;
pub mod scan;
pub mod waveform;

use std::process::Command;

use anyhow::{ensure, Context, Result};

/// Path to the `ffmpeg` binary, overridable for unusual installs.
pub fn ffmpeg_bin() -> String {
    std::env::var("AUDIOEDIT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// Path to the `ffprobe` binary, overridable for unusual installs.
pub fn ffprobe_bin() -> String {
    std::env::var("AUDIOEDIT_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string())
}

/// The actionable hint shown whenever a backend binary cannot be run at all,
/// whether that is discovered at startup or partway through a session (e.g.
/// the binary was removed, or PATH changed under a long-running process).
pub fn missing_backend_hint(bin: &str) -> String {
    format!(
        "could not run `{bin}`. audioedit needs ffmpeg and ffprobe on PATH \
         (set AUDIOEDIT_FFMPEG / AUDIOEDIT_FFPROBE to override)"
    )
}

/// Verify both tools are present, and actually runnable, before the TUI takes
/// over the terminal.
pub fn ensure_backend_available() -> Result<()> {
    check_runnable(&ffmpeg_bin())?;
    check_runnable(&ffprobe_bin())?;
    Ok(())
}

/// A binary is only "available" if it both spawns and exits successfully; a
/// binary that spawns but immediately errors out is not usable either.
fn check_runnable(bin: &str) -> Result<()> {
    let status = Command::new(bin)
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| missing_backend_hint(bin))?;
    ensure!(status.success(), "{}", missing_backend_hint(bin));
    Ok(())
}

/// The first line of a (possibly multi-line) error or status message, for
/// contexts — a report row, a status line — that must stay on one line.
pub fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_spawns_but_exits_nonzero_is_not_available() {
        let err = check_runnable("false").expect_err("`false` always exits non-zero");
        assert!(format!("{err:#}").contains("PATH"));
    }

    #[test]
    fn a_binary_that_exits_zero_is_available() {
        assert!(check_runnable("true").is_ok());
    }

    #[test]
    fn a_binary_that_cannot_even_spawn_is_not_available() {
        assert!(check_runnable("definitely-not-a-real-audioedit-binary").is_err());
    }
}
