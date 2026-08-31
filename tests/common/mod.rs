//! Shared fixtures for the integration tests: a scratch workspace of real
//! audio files produced by ffmpeg, plus the helpers every suite needs to
//! probe them and check what audioedit left behind.
//!
//! Each `tests/*.rs` file is compiled as its own binary with its own copy of
//! this module, and no single suite uses every helper here — hence the
//! blanket `dead_code` allowance rather than one per unused item.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use audioedit::media::probe::{self, MediaInfo};

/// Standard tags applied to every fixture.
pub const TAGS: &[(&str, &str)] = &[
    ("title", "Interview with Jane"),
    ("artist", "Example Podcast"),
    ("album", "Episode 42"),
    ("album_artist", "Example Podcast"),
    ("date", "2026-08-21"),
    ("genre", "Podcast"),
    ("comment", "Recorded remotely"),
];

/// A temporary directory removed when the test finishes.
pub struct Workspace(PathBuf);

impl Workspace {
    pub fn new(name: &str) -> Workspace {
        let path = std::env::temp_dir().join(format!(
            "audioedit-it-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("creating the workspace");
        Workspace(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Write a fixture: 4 s of silence, 6 s of tone, 7 s of silence. The
    /// silences comfortably clear the default min durations (3s / 5s).
    pub fn make(&self, name: &str, codec: &[&str]) -> PathBuf {
        let out = self.0.join(name);
        let mut command = Command::new("ffmpeg");
        command.args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc='0.5*sin(2*PI*440*t)*between(t,4,10)':s=48000:d=17:c=stereo",
        ]);
        command.args(codec);
        for (key, value) in TAGS {
            command.arg("-metadata").arg(format!("{key}={value}"));
        }
        command.arg(&out);
        let status = command.status().expect("running ffmpeg");
        assert!(status.success(), "could not build fixture {name}");
        out
    }

    /// A fixture with no silence anywhere, so nothing should be trimmed.
    pub fn make_continuous(&self, name: &str) -> PathBuf {
        let out = self.0.join(name);
        let status = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "aevalsrc='0.5*sin(2*PI*440*t)':s=48000:d=5:c=stereo",
                "-c:a",
                "libopus",
            ])
            .arg(&out)
            .status()
            .expect("running ffmpeg");
        assert!(status.success());
        out
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn probe_ok(path: &Path) -> MediaInfo {
    probe::probe(path)
        .unwrap_or_else(|e| panic!("probing {}: {e}", path.display()))
        .unwrap_or_else(|| panic!("{} has no audio stream", path.display()))
}

/// Files audioedit leaves behind while working, which must never survive.
pub fn temp_files(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("listing the workspace")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("audioedit-"))
        .collect()
}

/// Run the built binary and return its stdout.
pub fn run_binary(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(args)
        .output()
        .expect("running audioedit");
    assert!(
        output.status.success(),
        "audioedit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
