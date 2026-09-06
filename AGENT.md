# AGENT.md

Guidance for Claude Code on this repo, from a review of past sessions.

## Tools
- Use `Write`/`Edit`/`Grep`/`Glob`/`Read` — not heredocs, `sed -i`, `python3` replace scripts, `grep -rn`/`find`, or `cat`.
- Don't re-read a file right after `Edit`/`Write` to "verify" — a successful result already confirms it.

## Testing
- Extend the fixtures in `tests/*.rs` instead of one-off scratch dirs with manual `ffmpeg` runs.

## Docs
- Keep README concise by default.
- Use Mermaid for state-machine/mode diagrams.

## Navigating the app
Modes: BROWSE → (Enter) → PLAY → (e) → EDIT, PLAY → (m) → METADATA. `Esc`/`q` steps back one level (EDIT/METADATA always return to PLAY, never straight to BROWSE); `q` in BROWSE quits. Full state diagram: README.md "Modes" section.

## Repo layout
See readme.md's "Project Structure" section for the up-to-date module tree
(don't duplicate it here — it drifts). In short: `src/app/` is the TUI state
machine (one file per mode), `src/ui/` is pure rendering mirroring `app`'s
mode split, `src/media/` is the ffmpeg/ffprobe backend, `src/player/` is
playback, `src/batch/` is the folder-wide trim pipeline. `tests/*.rs` are the
integration tests (extend these, don't add scratch dirs).

## Dev tools available
- `cargo build` / `cargo run --` / `cargo test` / `cargo clippy` / `cargo fmt` — standard Rust toolchain, already installed.
- `ffmpeg` / `ffprobe` — installed in the devcontainer; used by `src/media` for decode/probe and by tests for fixture generation.

## CLI conventions
- `--help` shows defaults for every option.
- Dry-run/batch summaries: `-` as per-field placeholder for "unchanged/no value."

