//! The `:` command line and prompt submission (design §19).

use super::{App, MarkerKind, Mode, Overlay, PendingNav, Prompt, PromptKind, NO_FILE_OPEN};
use crate::batch::RunMode;
use crate::timespec::{parse_cursor_pos, parse_marker_pos, Marker};

impl App {
    pub(super) fn submit_prompt(&mut self, prompt: Prompt) {
        let input = prompt.buffer.trim().to_string();
        match prompt.kind {
            PromptKind::Command => self.run_command(&input),
            PromptKind::Search => {
                // The live preview already moved `selected`; just stop
                // treating a search as "in progress" without moving it back.
                self.search_origin = None;
                if input.is_empty() {
                    return;
                }
                self.last_search = input;
                // Live search has already previewed the match (or left the
                // selection at the pre-search position if nothing matched);
                // just confirm it rather than searching again, which would
                // skip past a match already on screen.
                let pattern = self.last_search.clone();
                let matched = if self.mode == Mode::Metadata {
                    self.current_field_matches(&pattern)
                } else {
                    self.current_file_matches(&pattern)
                };
                if matched {
                    self.info(format!("/{pattern}"));
                } else {
                    self.warn(format!("Pattern not found: {pattern}"));
                }
            }
            PromptKind::Marker(kind) => self.jump_marker_from_prompt(kind, &input),
            PromptKind::Cursor => self.set_cursor_from_expression(&input),
            PromptKind::MetadataField(index) => {
                if let Some(session) = &mut self.session {
                    if let Some(field) = session.fields.get_mut(index) {
                        let trimmed = input.trim();
                        field.value = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        };
                    }
                }
            }
        }
    }

    /// `:b <pos>` / `:e <pos>`: set a marker to an absolute or
    /// start/end-relative position, clamped so Begin/End can never cross
    /// (see `Session::set_marker`). Distinct from the interactive `b`/`e`
    /// prompt (`jump_marker_from_prompt`, below), which moves the cursor
    /// and is relative to it, not the file's start/end, and allows a
    /// transient crossing like any other drag.
    /// `:b`/`:e`: set a marker from a typed expression, same grammar and the
    /// same "relative to this marker's own position" meaning for `+`/`-` as
    /// the `b`/`e` prompt (`jump_marker_from_prompt`) — unified so the two
    /// entry paths don't silently disagree on what `+10s` means. Unlike the
    /// prompt, this doesn't move playback: it only sets the marker in place.
    fn set_marker_from_expression(&mut self, kind: MarkerKind, input: &str) {
        let Some((current, duration)) = self
            .session
            .as_ref()
            .map(|s| (s.marker(kind).seconds(), s.duration()))
        else {
            self.warn(NO_FILE_OPEN);
            return;
        };
        match parse_marker_pos(input, current) {
            Ok(spec) => {
                let marker = Marker::from_spec(spec, input.trim().to_string(), duration);
                let shown = if let Some(session) = &mut self.session {
                    session.set_marker(kind, marker);
                    session.marker(kind).to_string()
                } else {
                    return;
                };
                self.info(format!("{} marker set to {shown}", kind.label()));
            }
            Err(err) => self.warn(format!("{err}. Try 10:00, +10s, ++10s, --10s or 50%.")),
        }
    }

    /// `b`/`e`/`i`: type a jump for a marker, same grammar as the `c`
    /// cursor-jump prompt (`++`/`--` from the start/end) except `+`/`-` are
    /// relative to *this marker's own* current position, not the cursor's —
    /// typing `+10` for `b` means 10s after wherever Begin already is, even
    /// if the cursor (playback) is somewhere else entirely. Moves the
    /// cursor to the result and makes `kind` the active, hugging marker —
    /// possibly crossing the other marker transiently, same as dragging
    /// with Left/Right.
    fn jump_marker_from_prompt(&mut self, kind: MarkerKind, input: &str) {
        let Some((current, duration)) = self
            .session
            .as_ref()
            .map(|s| (s.marker(kind).seconds(), s.duration()))
        else {
            self.warn(NO_FILE_OPEN);
            return;
        };
        match parse_cursor_pos(input, current, duration) {
            Ok(seconds) => {
                self.with_player(|p| p.seek_to(seconds));
                let shown = if let Some(session) = &mut self.session {
                    session.active = kind;
                    session.drag_active_marker();
                    session.marker(kind).to_string()
                } else {
                    return;
                };
                self.info(format!("{} marker set to {shown}", kind.label()));
            }
            Err(err) => self.warn(format!("{err}. Try 10:00, +10s, ++10s, --10s or 50%.")),
        }
    }

    /// `c`: type a jump for the cursor (the playback position — there is no
    /// separate cursor value), same as a large Left/Right move: the active
    /// marker keeps hugging it, possibly crossing the other marker
    /// transiently (see `Session::drag_active_marker`). `+`/`-` are
    /// relative to the cursor's current position; `++`/`--` are from the
    /// start/end of the file.
    fn set_cursor_from_expression(&mut self, input: &str) {
        let Some((current, duration)) = self
            .session
            .as_ref()
            .map(|s| (s.player.position(), s.duration()))
        else {
            self.warn(NO_FILE_OPEN);
            return;
        };
        match parse_cursor_pos(input, current, duration) {
            Ok(seconds) => {
                self.with_player(|p| p.seek_to(seconds));
                if let Some(session) = &mut self.session {
                    session.drag_active_marker();
                }
                self.info(format!(
                    "Cursor moved to {}",
                    crate::timespec::format_timestamp(seconds)
                ));
            }
            Err(err) => self.warn(format!("{err}. Try 10:00, +10s, ++10s, --10s or 50%.")),
        }
    }

    pub(super) fn run_command(&mut self, input: &str) {
        let mut parts = input.split_whitespace();
        let Some(command) = parts.next() else {
            return;
        };
        let rest: Vec<&str> = parts.collect();
        let argument = rest.join(" ");

        match command {
            "w" | "write" => self.save_current(),
            "q" | "quit" => {
                let target = self.close_target();
                self.request_nav(target);
            }
            "q!" | "quit!" => {
                let target = self.close_target();
                self.perform_nav(target);
            }
            "wq" | "x" => {
                if self.session.as_ref().is_some_and(|s| !s.is_dirty()) {
                    // Nothing to write; matches vim's `:x`, which only
                    // writes a modified buffer. A bare `:w` on an unchanged
                    // file still runs the full pipeline to report NO-OP
                    // explicitly (design §16).
                    self.close_file();
                } else {
                    self.pending_nav_after_save = Some(PendingNav::CloseFile);
                    self.save_current();
                }
            }
            "help" | "h" => self.overlay = Overlay::Help,
            "apply-defaults" => {
                if rest.iter().any(|a| *a == "--dry-run" || *a == "-n") {
                    self.start_batch(RunMode::DryRun);
                } else {
                    self.overlay = Overlay::ConfirmApply;
                }
            }
            "dry-run" => self.start_batch(RunMode::DryRun),
            "b" | "begin" => self.set_marker_from_expression(MarkerKind::Begin, &argument),
            "e" | "end" => self.set_marker_from_expression(MarkerKind::End, &argument),
            "auto" => self.recalculate_auto_markers(),
            "reset" => {
                if self.session.is_some() {
                    self.reset_markers();
                } else {
                    self.warn(NO_FILE_OPEN);
                }
            }
            other => self.warn(format!("Unknown command: :{other}. Try :help")),
        }
    }

    fn close_target(&self) -> PendingNav {
        if self.session.is_some() {
            PendingNav::CloseFile
        } else {
            PendingNav::Quit
        }
    }
}

#[cfg(test)]
mod tests;
