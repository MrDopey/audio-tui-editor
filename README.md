# audioedit

A keyboard-driven terminal editor for browsing, auditioning, trimming and
retagging audio files. It behaves like Vim: distinct modes, `hjkl` navigation,
`/` search, `gg`/`G`, `Esc` to leave a mode, and a `:` command line.

```
browse files → select file → play / inspect → edit trim markers → save in place
```

## Requirements

`ffmpeg` and `ffprobe` on `PATH`. They do all decoding, trimming and muxing;
audioedit never touches codec internals itself. Set `AUDIOEDIT_FFMPEG` and
`AUDIOEDIT_FFPROBE` to point at a different build.

Audio output is optional. Without a device, everything except sound still
works and the transport says so.

## Install

```bash
cargo build --release
./target/release/audioedit --help
```

## Usage

```bash
audioedit                          # the current directory
audioedit --folder ~/recordings
```

Configuration values can be overridden per run:

```bash
audioedit \
  --folder ~/recordings \
  --begin-threshold-db -40 \
  --end-threshold-db -40 \
  --begin-min-duration 1 \
  --end-min-duration 1
```

Apply the automatic trim policy to every supported file in a folder:

```bash
audioedit --folder ~/recordings --apply-defaults          # asks first
audioedit --folder ~/recordings --apply-defaults --yes    # unattended
audioedit --folder ~/recordings --dry-run                 # report only, writes nothing
```

A dry run performs exactly the same silence detection as a real run and prints
what each file *would* become, without modifying anything.

## Modes

```
BROWSE  Enter → PLAY
PLAY    e → EDIT    m → METADATA    Esc → BROWSE
EDIT    Esc → PLAY
METADATA Esc → PLAY
```

The mode is always shown in the top-left. A file with unsaved changes is
marked `[+]`, and leaving it asks before discarding.

### BROWSE

| Key | Action |
| --- | --- |
| `j` `k` `↓` `↑` | next / previous file |
| `gg` `G` | first / last file |
| `Ctrl-d` `Ctrl-u` | page down / up |
| `/` `n` `N` | search, next match, previous match |
| `Enter` | open in PLAY mode |
| `r` | rescan the folder |
| `q` | quit |

Files are listed by probing them, not by trusting extensions, so anything
ffprobe recognises as audio appears and nothing else does.

### PLAY

| Key | Action |
| --- | --- |
| `space` | play / pause |
| `←` `→` `h` `l` | seek by the small step (default 10 s) |
| `Ctrl-←` `Ctrl-→` `Ctrl-h` `Ctrl-l` | seek by the large step (default 60 s) |
| `↑` `↓` `k` `j` | volume up / down |
| `e` / `m` | EDIT / METADATA mode |
| `Esc` | back to BROWSE |

The waveform covers the whole file, scales to the terminal width and shows the
playback cursor in real time. Analysis runs once per file in the background and
is cached on disk, so playback updates never recompute it.

### EDIT

Two markers define what is *kept*: everything between `beginning` and `ending`.

| Key | Action |
| --- | --- |
| `←` `→` `h` `l` | move the active marker by the fine step (default 1 s) |
| `Ctrl-←` `Ctrl-→` `Ctrl-h` `Ctrl-l` | move it by the large step (default 10 s) |
| `Tab` | switch between the beginning and ending marker |
| `b` / `e` | set the beginning / ending marker at the playhead |
| `B` / `E` / `i` | type a position |
| `a` | recalculate the automatic markers |
| `r` | reset to the whole file |
| `p` | play from the active marker |

Positions can be written relative to either end of the file, so you never have
to work out an absolute timestamp:

```
+10s    10 seconds after the start
-10s    10 seconds before the end
+1m     one minute in
50%     halfway
10:00   an absolute timestamp
```

For a ten-minute file, `:b +10s` is `00:10` and `:e -10s` is `09:50`. The
expression you wrote is kept on screen next to the timestamp it resolves to.

On entering EDIT mode the markers are set automatically: the beginning moves
past any leading silence and the ending stops before any trailing silence,
using the configured thresholds and minimum durations.

### METADATA

`j`/`k` move between fields, `Enter` or `i` edits one, `u` reverts it, and
`:w` saves. Edited fields are highlighted until saved.

### Commands

```
:w                        save in place
:q                        leave the file (or quit)
:q!                       leave, discarding changes
:wq                       save and leave
:b <pos>  :e <pos>        set a marker, e.g. :b +10s
:auto                     recalculate automatic markers
:reset                    reset markers to the whole file
:apply-defaults           trim every file in the folder
:apply-defaults --dry-run report what would change, writing nothing
:help                     key reference
```

## Saving

Saving is an in-place operation, and the original is only ever replaced by an
atomic rename over a file that has already been produced and checked:

```
source file → temporary output → media validation → metadata validation → atomic replacement
```

If any step fails the original is left byte-for-byte untouched, the temporary
file is removed, and the error dialog says so with the ffmpeg diagnostics
behind `[Enter] details`.

Lossless stream copy is preferred. When copying cannot produce a valid file the
audio is re-encoded with an explicit policy that keeps the source codec and
bitrate, and the save summary always states which was used. (FLAC is the common
case: ffmpeg copies the source `STREAMINFO`, so a stream-copied trim would
declare the wrong length — audioedit rejects that and re-encodes, which is
still lossless.)

Metadata is compared field by field between the source and the finished file,
and the summary reports exactly what survived. Nothing is claimed as preserved
unless it is actually present in the output.

## Configuration

`$XDG_CONFIG_HOME/audioedit/config.toml` (or `--config <path>`):

```toml
[playback]
small_seek_seconds = 10
large_seek_seconds = 60
volume_step = 5

[editing]
fine_step_seconds = 1
large_step_seconds = 10

[auto_trim]
begin_threshold_db = -40
end_threshold_db = -40
begin_min_duration = 1
end_min_duration = 1
```

Every value has a matching command-line flag, and the flag wins.

## Development

```bash
cargo test          # unit tests plus integration tests against real audio
cargo clippy --all-targets
cargo fmt
```

The integration tests build audio fixtures with ffmpeg and assert the
guarantees that matter: originals survive failed saves, no-ops are reported and
do not rewrite files, dry runs write nothing, and metadata claims match what is
on disk.
