//! The binary's startup behaviour: config files, CLI overrides and the
//! non-interactive consent gate (design §2, §3, §12, §17).

mod common;

use std::process::Command;

use common::{run_binary, Workspace};

#[test]
fn the_binary_reads_a_config_file_and_lets_the_cli_override_it() {
    let ws = Workspace::new("config");
    ws.make("a.flac", &["-c:a", "flac"]);
    ws.make("b.opus", &["-c:a", "libopus", "-b:a", "64k"]);

    // Minimum durations longer than the fixture's silences (4s / 7s): nothing qualifies.
    let config = ws.path().join("config.toml");
    std::fs::write(
        &config,
        "[auto_trim]\nbegin_min_duration = 8\nend_min_duration = 8\n",
    )
    .unwrap();

    let folder = ws.path().to_str().unwrap();
    let config_path = config.to_str().unwrap();

    let with_config = run_binary(&["--folder", folder, "--config", config_path, "--dry-run"]);
    assert!(
        with_config.contains("Would change: 0"),
        "config ignored:\n{with_config}"
    );
    assert!(with_config.contains("No-op:     2"));

    // The CLI wins over the file.
    let overridden = run_binary(&[
        "--folder",
        folder,
        "--config",
        config_path,
        "--dry-run",
        "--begin-min-duration",
        "1",
        "--end-min-duration",
        "1",
    ]);
    assert!(
        overridden.contains("Would change: 2"),
        "override ignored:\n{overridden}"
    );
    assert!(overridden.contains("no files were modified"));
}

#[test]
fn the_binary_rejects_an_invalid_configuration_instead_of_guessing() {
    let ws = Workspace::new("badconfig");
    let config = ws.path().join("config.toml");
    std::fs::write(&config, "[playback]\nsmall_seek_seconds = 0\n").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(["--folder", ws.path().to_str().unwrap()])
        .args(["--config", config.to_str().unwrap(), "--dry-run"])
        .output()
        .expect("running audioedit");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("small_seek_seconds"),
        "unhelpful error: {stderr}"
    );
}

#[test]
fn a_non_interactive_apply_refuses_without_explicit_consent() {
    let ws = Workspace::new("consent");
    let path = ws.make("a.flac", &["-c:a", "flac"]);
    let before = std::fs::read(&path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_audioedit"))
        .args(["--folder", ws.path().to_str().unwrap(), "--apply-defaults"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("running audioedit");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--yes"), "unhelpful error: {stderr}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file was rewritten anyway"
    );
}

#[test]
fn a_folder_with_no_audio_is_reported_rather_than_failing() {
    let ws = Workspace::new("empty");
    std::fs::write(ws.path().join("notes.txt"), b"nothing here").unwrap();
    let stdout = run_binary(&["--folder", ws.path().to_str().unwrap(), "--dry-run"]);
    assert!(stdout.contains("No supported audio files"), "{stdout}");
}
