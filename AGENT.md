# AGENT.md

Guidance for Claude Code on this repo, from a review of past sessions.

## Tools
- Use `Write`/`Edit`/`Grep`/`Glob`/`Read` — not heredocs, `sed -i`, `python3` replace scripts, `grep -rn`/`find`, or `cat`.
- Don't re-read a file right after `Edit`/`Write` to "verify" — a successful result already confirms it.

## Testing
- Extend `tests/pipeline.rs` fixtures instead of one-off scratch dirs with manual `ffmpeg` runs.

## Docs
- Keep README concise by default.
- Use Mermaid for state-machine/mode diagrams.

## Navigating the app
Modes: BROWSE → (Enter) → PLAY → (e) → EDIT, PLAY → (m) → METADATA. `Esc`/`q` steps back one level (EDIT/METADATA always return to PLAY, never straight to BROWSE); `q` in BROWSE quits. Full state diagram: README.md "Modes" section.

## Repo layout
```
Cargo.toml / Cargo.lock       crate manifest (bin+lib "audioedit")
config.example.toml           sample user config
README.md                     user-facing docs
src/
  main.rs, lib.rs, cli.rs     entry point, lib root, clap CLI
  app/                        TUI state machine (mod.rs + one file per mode: browse/play/edit/metadata/save/command/session/batch_view)
  media/                      audio backend (ffmpeg.rs, probe.rs, autotrim.rs, waveform.rs)
  player.rs, batch.rs         playback (rodio), batch/dry-run pipeline
  config.rs, timespec.rs, ui.rs
tests/pipeline.rs              integration tests (extend these, don't add scratch dirs)
```

## Dev tools available
- `cargo build` / `cargo run --` / `cargo test` / `cargo clippy` / `cargo fmt` — standard Rust toolchain, already installed.
- `ffmpeg` / `ffprobe` — installed in the devcontainer; used by `src/media` for decode/probe and by tests for fixture generation.

## CLI conventions
- `--help` shows defaults for every option.
- Dry-run/batch summaries: `-` as per-field placeholder for "unchanged/no value."

