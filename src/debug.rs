//! Opt-in debug logging (`--debug` / `AUDIOEDIT_DEBUG`).
//!
//! Kept intentionally small: a process-wide toggle plus a formatter for
//! logging the exact ffmpeg/ffprobe commands audioedit runs, since that
//! single piece of information — the full argv and its byte size — is what
//! past investigations into spawn/save failures actually needed.

use std::process::Command;
use std::sync::OnceLock;

static ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether `--debug` or `AUDIOEDIT_DEBUG` requests debug mode.
pub fn requested(cli_flag: bool) -> bool {
    cli_flag || std::env::var("AUDIOEDIT_DEBUG").is_ok()
}

/// Turns debug mode on or off for the rest of the process. Call once, early
/// in `main`, before anything that might spawn a command or construct an
/// error worth a backtrace. Later calls are ignored (the flag can only be
/// set once) — safe to call defensively, but only the first call wins.
pub fn enable_if(enabled: bool) {
    let _ = ENABLED.set(enabled);
}

/// Whether debug mode is on. `false` until [`enable_if`] has been called.
pub fn is_enabled() -> bool {
    ENABLED.get().copied().unwrap_or(false)
}

/// Logs `cmd` to stderr as `[debug] running: ...` when debug mode is on;
/// a no-op otherwise. Call once per ffmpeg/ffprobe invocation, after all
/// args are attached but before spawning, so the full argv is captured.
pub fn log_command(cmd: &Command) {
    if is_enabled() {
        eprintln!("{}", describe_command(cmd));
    }
}

/// Pure formatter for [`log_command`], split out so the format can be unit
/// tested without capturing stderr.
fn describe_command(cmd: &Command) -> String {
    let mut argv: Vec<String> = vec![cmd.get_program().to_string_lossy().into_owned()];
    argv.extend(cmd.get_args().map(|a| a.to_string_lossy().into_owned()));
    let argv_bytes: usize = argv.iter().map(|a| a.len() + 1).sum::<usize>().saturating_sub(1);

    let env_vars: Vec<(String, String)> = cmd
        .get_envs()
        .filter_map(|(k, v)| {
            let v = v?;
            Some((k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        })
        .collect();
    let env_bytes: usize = env_vars
        .iter()
        .map(|(k, v)| k.len() + 1 + v.len())
        .sum();

    format!(
        "[debug] running: {} (argv {}B, {} env var{}, {}B)",
        argv.join(" "),
        argv_bytes,
        env_vars.len(),
        if env_vars.len() == 1 { "" } else { "s" },
        env_bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_program_args_and_byte_sizes() {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-y", "-i", "in.wav"]);
        cmd.env_clear();
        cmd.env("PATH", "/usr/bin");

        let line = describe_command(&cmd);
        assert!(line.starts_with("[debug] running: ffmpeg -y -i in.wav ("), "{line}");
        assert!(line.contains("argv "), "{line}");
        assert!(line.contains("1 env var,"), "{line}");
    }

    #[test]
    fn pluralizes_env_var_count() {
        let mut cmd = Command::new("true");
        cmd.env_clear();
        let line = describe_command(&cmd);
        assert!(line.contains("0 env vars,"), "{line}");
    }

    #[test]
    fn the_flag_or_the_env_var_either_one_requests_debug_mode() {
        // SAFETY: no other thread in this test touches this variable.
        unsafe {
            std::env::remove_var("AUDIOEDIT_DEBUG");
        }
        assert!(!requested(false));
        assert!(requested(true));

        unsafe {
            std::env::set_var("AUDIOEDIT_DEBUG", "1");
        }
        assert!(requested(false));
        unsafe {
            std::env::remove_var("AUDIOEDIT_DEBUG");
        }
    }
}
