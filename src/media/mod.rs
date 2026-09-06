//! The media backend: everything that shells out to ffmpeg/ffprobe.

pub mod autotrim;
pub mod ffmpeg;
pub mod probe;
pub mod scan;
pub mod waveform;

use std::process::{Command, ExitStatus};

use anyhow::{bail, ensure, Result};

/// Path to the `ffmpeg` binary, overridable for unusual installs.
pub fn ffmpeg_bin() -> String {
    std::env::var("AUDIOEDIT_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string())
}

/// Path to the `ffprobe` binary, overridable for unusual installs.
pub fn ffprobe_bin() -> String {
    std::env::var("AUDIOEDIT_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string())
}

/// Builds a `Command` for `bin` with a minimal, fixed environment instead of
/// blindly inheriting the calling process's whole one.
///
/// ffmpeg/ffprobe need nothing from the environment beyond `PATH` (to find
/// themselves, when `bin` is a bare name rather than an absolute override).
/// Passing every inherited variable through anyway counts them against the
/// same kernel limit as the command's own arguments, so a process with an
/// unusually large environment — one launched by an agent/orchestration
/// wrapper rather than a plain interactive shell, say — can make a perfectly
/// ordinary ffmpeg invocation fail with `E2BIG` ("Argument list too long"),
/// more easily the longer the file path being processed happens to be.
pub fn backend_command(bin: &str) -> Command {
    let mut command = Command::new(bin);
    command.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    command
}

/// The actionable hint shown whenever a backend binary could not be spawned
/// at all, whether that is discovered at startup or partway through a
/// session (e.g. the binary was removed, or PATH changed under a
/// long-running process).
///
/// Distinguishes a binary that genuinely isn't on PATH (`NotFound`) from one
/// that exists but could not be executed for some other reason. Blaming PATH
/// for both sends someone chasing PATH when the real problem is something
/// else — so anything other than `NotFound` just reports what the OS
/// actually said, rather than guessing why, while still naming the one
/// thing the user can actually do about it either way: point at a
/// different binary.
pub fn spawn_error_hint(bin: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        format!(
            "`{bin}` was not found on PATH. audioedit needs ffmpeg and ffprobe \
             (set AUDIOEDIT_FFMPEG / AUDIOEDIT_FFPROBE to point at them directly)"
        )
    } else {
        format!(
            "`{bin}` could not be run: {err} \
             (set AUDIOEDIT_FFMPEG / AUDIOEDIT_FFPROBE to use a different binary)"
        )
    }
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
    let mut command = backend_command(bin);
    command
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::debug::log_command(&command);
    let status = match command.status() {
        Ok(status) => status,
        Err(err) => bail!("{}", spawn_error_hint(bin, &err)),
    };
    ensure!(
        status.success(),
        "`{bin} -version` ran but exited with an error; audioedit needs a \
         working ffmpeg and ffprobe (set AUDIOEDIT_FFMPEG / AUDIOEDIT_FFPROBE \
         to point at a different binary)"
    );
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

/// Fail with a `stderr` tail if a subprocess's exit status wasn't success —
/// the "spawned fine, then errored out" check shared by every ffmpeg/ffprobe
/// call site. `what` is folded in as `"{what}: {tail}"`; pass `""` when the
/// caller already prefixes its own context (e.g. which save attempt failed).
pub fn require_success(status: ExitStatus, stderr: &str, what: &str, tail_lines: usize) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    let tail = tail_of(stderr, tail_lines);
    if what.is_empty() {
        bail!("{tail}");
    }
    bail!("{what}: {tail}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binary_that_spawns_but_exits_nonzero_is_not_available() {
        let err = check_runnable("false").expect_err("`false` always exits non-zero");
        // `false` is perfectly reachable on PATH, so the message must not
        // send someone chasing PATH for what is actually a bad binary.
        assert!(!format!("{err:#}").contains("PATH"));
    }

    #[test]
    fn a_binary_that_exits_zero_is_available() {
        assert!(check_runnable("true").is_ok());
    }

    #[test]
    fn a_binary_that_cannot_even_spawn_is_not_available() {
        let err = check_runnable("definitely-not-a-real-audioedit-binary")
            .expect_err("no such binary exists");
        assert!(
            format!("{err:#}").contains("PATH"),
            "a genuinely missing binary should still point at PATH"
        );
    }

    #[test]
    fn backend_command_does_not_leak_arbitrary_environment_variables() {
        // SAFETY: this test doesn't spawn threads that also touch the
        // environment, and the variable is removed again right after.
        unsafe {
            std::env::set_var("AUDIOEDIT_TEST_SHOULD_NOT_LEAK", "leaked");
        }
        let output = backend_command("env").output().expect("running `env`");
        unsafe {
            std::env::remove_var("AUDIOEDIT_TEST_SHOULD_NOT_LEAK");
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("AUDIOEDIT_TEST_SHOULD_NOT_LEAK"),
            "the child must not see the parent's arbitrary environment"
        );
        assert!(
            stdout.contains("PATH="),
            "PATH must still be passed through"
        );
    }

    #[test]
    fn spawn_error_hint_blames_path_only_when_the_binary_is_truly_missing() {
        let not_found = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(spawn_error_hint("ffmpeg", &not_found).contains("PATH"));

        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let hint = spawn_error_hint("ffmpeg", &denied);
        assert!(
            !hint.contains("PATH"),
            "a binary that exists but can't be run isn't a PATH problem"
        );
        // Reports the OS's own message rather than guessing why.
        assert!(hint.contains(&denied.to_string()));
    }
}
