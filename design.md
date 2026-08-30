# Implementation Requirement: Vim-like TUI Audio Editor

## 1. Objective

Develop a keyboard-driven terminal user interface for browsing, playing, trimming, and editing metadata for audio files.

The application should behave conceptually like Vim:

* distinct interaction modes
* mode-specific keybindings
* `hjkl` navigation
* `/` search
* `g` / `G` navigation
* `Esc` to leave the current mode
* command-mode operations where useful

The primary workflow is:

```text
browse files
    ↓
select file
    ↓
play / inspect
    ↓
edit trim markers
    ↓
save in place
```

---

# 2. CLI

The application starts in the current working directory by default.

```bash
audioedit
```

A folder may be supplied:

```bash
audioedit --folder ~/recordings
```

Configuration values may be overridden from the CLI.

Example:

```bash
audioedit \
  --folder ~/recordings \
  --begin-threshold-db -40 \
  --end-threshold-db -40 \
  --begin-min-duration 1 \
  --end-min-duration 1
```

The CLI should also support applying the default trim policy to all supported files in the selected folder.

Example:

```bash
audioedit --apply-defaults
```

---

# 3. Startup

On startup:

1. Determine the working folder.
2. Scan the folder for supported audio files.
3. Load application configuration.
4. Apply CLI overrides.
5. Display an in-place editing warning.
6. Enter BROWSE mode.

The warning must clearly state that saving overwrites the original file.

Example:

```text
WARNING

Saving edits modifies the original audio file in place.

A temporary output is created and verified before the
original file is replaced.

Metadata is preserved where supported by the source format.
```

---

# 4. Modes

The application has four primary modes:

```text
BROWSE
PLAY
EDIT
METADATA
```

The current mode must always be visible.

Expected transitions:

```text
BROWSE
  Enter → PLAY

PLAY
  e → EDIT
  m → METADATA
  Esc → BROWSE

EDIT
  Esc → PLAY

METADATA
  Esc → PLAY
```

Unsaved changes must not be silently discarded.

---

# 5. BROWSE Mode

## File List

Display supported audio files in the current folder.

Each entry should show at least:

```text
filename
duration
format
```

Example:

```text
> interview-001.opus     01:42:31
  interview-002.opus     00:48:12
  interview-003.mp3      02:15:04
```

The implementation should use media probing rather than trusting filename extensions alone.

## Navigation

Support:

```text
j / Down       next file
k / Up         previous file
g g            first file
G              last file
Ctrl-d         page down
Ctrl-u         page up
/              search
n              next match
N              previous match
Enter          open selected file
q              quit
```

---

# 6. PLAY Mode

A selected file is opened in PLAY mode.

The application must use an existing audio playback library rather than implementing a complete audio decoder/player from scratch.

The player must expose:

* play
* pause
* current position
* duration
* seek
* volume
* end-of-file state

## Seeking

Default controls:

```text
Left           seek backward 10 seconds
Right          seek forward 10 seconds

Ctrl-Left      seek backward 60 seconds
Ctrl-Right     seek forward 60 seconds

h              seek backward 10 seconds
l              seek forward 10 seconds

Ctrl-h         seek backward 60 seconds
Ctrl-l         seek forward 60 seconds
```

Seek amounts must be configurable.

## Volume

```text
Up             volume up
Down           volume down
k              volume up
j              volume down
```

Volume increments must be configurable.

---

# 7. Waveform

PLAY mode must display a discretized representation of the audio waveform across the file duration.

The waveform must:

* represent the complete file
* show current playback position
* scale to terminal width
* update the playback cursor in real time
* avoid recomputing the waveform on every playback update

Example:

```text
00:00                                      03:42

▁▁▂▃▂▁▁▃▆█▇▆▂▁▁▂▃▅▇█▆▃▂▁▁▁▂▅▆▇▅▃▂▁
                         │
                         playback
```

The implementation should use a suitable amplitude representation such as peak or RMS aggregation.

Waveform analysis should be cached where practical.

---

# 8. EDIT Mode

EDIT mode allows the user to define the section of audio that will be retained.

There are two markers:

```text
b = beginning marker
e = ending marker
```

The retained audio is:

```text
beginning marker → ending marker
```

Audio outside this range is removed.

---

# 9. Marker Navigation

Markers can be moved using Vim motions and arrow keys.

Default:

```text
Left / h       move marker backward
Right / l      move marker forward
```

The fine movement increment defaults to one second.

A larger movement increment should also be supported.

Example:

```text
Ctrl-Left / Ctrl-h
Ctrl-Right / Ctrl-l
```

The movement sizes must be configurable.

---

# 10. Relative Marker Positions

Relative positioning is a first-class user-facing feature.

Users must not have to calculate an absolute end timestamp.

The UI must support expressions such as:

```text
+10s
-10s
+1m
-1m
50%
```

Examples:

For a 10-minute file:

```text
b = +10s
```

means:

```text
00:10
```

and:

```text
e = -10s
```

means:

```text
09:50
```

The application internally resolves relative positions into absolute timestamps.

The user-facing representation should retain the semantic expression where practical.

The application should support both marker positions and relative movement without requiring the user to interact with FFmpeg timestamp syntax.

---

# 11. Automatic Marker Defaults

When a file is opened in EDIT mode, the application should automatically calculate sensible beginning and ending markers.

## Beginning

The beginning marker should default to the first suitable point where audio exceeds a configured amplitude threshold.

## Ending

The ending marker should default to the final suitable point where audio falls below the configured threshold toward the end of the file.

Both detections must have configurable minimum durations.

Defaults:

```text
begin threshold: -40 dB
end threshold:   -40 dB

begin minimum duration: 1 second
end minimum duration:   1 second
```

Beginning and ending settings must be independently configurable.

---

# 12. Editing Configuration

Example configuration:

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

---

# 13. Save

Saving is an in-place operation.

The user should have a keyboard shortcut and command for saving.

For example:

```text
:w
```

The original file must not be overwritten until the new file has successfully been generated and validated.

Required process:

```text
source file
    ↓
temporary output
    ↓
media validation
    ↓
metadata validation
    ↓
atomic replacement
```

If processing fails, the original file must remain unchanged.

---

# 14. Processing

The application should use a suitable command-line media-processing backend capable of:

* decoding
* trimming
* stream copying where possible
* re-encoding when necessary
* metadata preservation
* metadata modification

The application should prefer lossless stream-copy processing when technically valid.

When stream copying cannot safely perform the requested operation, the application may re-encode according to an explicit encoding policy.

The UI must indicate whether the operation used:

```text
stream copy
```

or:

```text
re-encoding
```

---

# 15. Metadata Preservation

Saving an edited file must preserve metadata wherever the source and destination format permit it.

At minimum, attempt to preserve:

```text
title
artist
album
album artist
date
genre
track
disc
comment
copyright
composer
lyrics
cover artwork
chapter metadata
stream metadata
```

Metadata preservation must be verified where practical.

The application must not falsely report metadata as preserved when the destination format cannot retain it.

---

# 16. Save Summary

After every save, display a summary.

Example:

```text
Saved: interview.opus

Duration:
  01:42:31.200 → 01:42:08.200

Removed:
  beginning: 12.000s
  ending:     11.000s

Processing:
  stream copy

Metadata:
  preserved

Status:
  SUCCESS
```

For no changes:

```text
Saved: interview.opus

No changes were required.

Duration:
  01:42:31.200 → 01:42:31.200

Status:
  NO-OP
```

No-op operations must be explicitly reported.

---

# 17. Apply Defaults to All Files

The user must be able to apply the automatically detected beginning/end trim policy to every supported audio file in the current folder.

Before starting, ask for confirmation.

Example:

```text
Apply automatic trim to 43 files?

Threshold:
  begin -40 dB
  end   -40 dB

Minimum duration:
  begin 1s
  end   1s

[Enter] continue
[Esc] cancel
```

Each file must be processed independently.

The final report must include:

```text
Processed: 43
Changed:   37
No-op:      5
Failed:     1
Skipped:    0
```

Each result must remain inspectable.

Example:

```text
01 interview-001.opus   02:31 → 02:28
02 interview-002.opus   NO-OP
03 interview-003.opus   14:22 → 14:10
```

---

# 18. METADATA Mode

METADATA mode displays editable metadata fields.

Example:

```text
Title:         Interview with Jane
Artist:        Example Podcast
Album:         Episode 42
Album Artist:  Example Podcast
Date:          2026-08-21
Genre:         Podcast
Comment:       Recorded remotely
```

Navigation should use Vim-style movement.

Suggested controls:

```text
j/k             next/previous field
Enter or i      edit field
Esc             finish editing
:w              save
:q              leave
:wq             save and leave
```

Saving metadata follows the same safe temporary-file and replacement workflow as trimming.

---

# 19. Command Mode

Provide a Vim-like command line.

Minimum commands:

```text
:w
:q
:wq
:help
:apply-defaults
```

Additional configuration/editing commands may be implemented later.

---

# 20. Error Handling

Errors must never silently modify the original file.

On failure:

```text
ERROR

Could not save the file.

The original file has NOT been modified.

[Enter] details
```

Failures must include actionable diagnostic information.

---

# 21. Supported Formats

Initial support should include common audio formats such as:

```text
opus
mp3
wav
flac
m4a
ogg
aac
```

The application should determine actual support through media probing rather than relying exclusively on file extensions.

---

# 22. Non-Goals

The first version is not intended to be:

* a full DAW
* a multitrack editor
* a plugin host
* a sound-design environment
* a general waveform production suite

The focus is:

```text
browse
audition
locate
trim
edit metadata
save safely
```

---

# 23. Acceptance Criteria

The implementation is complete when a user can:

1. Launch the application in a folder.
2. Browse audio files using Vim-like navigation.
3. Search for a file.
4. Open a file in PLAY mode.
5. Play and pause it.
6. Seek by configurable relative increments.
7. Adjust volume.
8. See a waveform and playback position.
9. Enter EDIT mode.
10. Use automatic beginning/end markers.
11. Move markers precisely.
12. Specify positions relative to the beginning or end of the file.
13. Save the edit in place.
14. Receive an accurate before/after summary.
15. Preserve metadata where supported.
16. Edit metadata manually.
17. Save metadata changes safely.
18. Apply automatic trimming to an entire folder.
19. Receive per-file results including no-ops and failures.
20. Never lose the original file due to an unsuccessful processing operation.

