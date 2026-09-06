//! The `:` command line and prompt submission (design §19).

use super::{App, MarkerKind, Mode, Overlay, PendingNav, Prompt, PromptKind, NO_FILE_OPEN};
use crate::batch::RunMode;
use crate::timespec::{parse_cursor_pos, Marker};

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
    fn set_marker_from_expression(&mut self, kind: MarkerKind, input: &str) {
        let Some(duration) = self.session.as_ref().map(super::Session::duration) else {
            self.warn(NO_FILE_OPEN);
            return;
        };
        match Marker::parse(input, duration) {
            Ok(marker) => {
                let shown = if let Some(session) = &mut self.session {
                    session.set_marker(kind, marker);
                    session.marker(kind).to_string()
                } else {
                    return;
                };
                self.info(format!("{} marker set to {shown}", kind.label()));
            }
            Err(err) => self.warn(format!("{err}. Try 10:00, +10s, -1m or 50%.")),
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
mod tests {
    use super::super::tests::{app, press, press_ctrl, type_text};
    use crate::app::Overlay;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn relative_expressions_set_markers_and_keep_their_text() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        app.run_command("b +10s");
        app.run_command("e -10s");
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 10.0);
        assert_eq!(session.end.seconds(), 590.0);
        assert_eq!(session.begin.text(), "+10s");
        assert_eq!(session.end.to_string(), "-10s (09:50)");
    }

    #[test]
    fn cursor_prompt_single_prefixes_are_relative_to_the_markers_current_position() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press_ctrl(&mut app, KeyCode::Char('l')); // begin marker -> 10s

        press(&mut app, KeyCode::Char('c'));
        type_text(&mut app, "+5s");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 15.0);

        press(&mut app, KeyCode::Char('c'));
        type_text(&mut app, "-20s");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.session.as_ref().unwrap().begin.seconds(),
            0.0,
            "clamped to the file"
        );
    }

    #[test]
    fn cursor_prompt_double_prefixes_are_absolute_from_start_and_end() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press_ctrl(&mut app, KeyCode::Char('l')); // begin marker -> 10s

        press(&mut app, KeyCode::Char('c'));
        type_text(&mut app, "++5s");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 5.0);

        press(&mut app, KeyCode::Tab); // switch active marker to End
        press(&mut app, KeyCode::Char('c'));
        type_text(&mut app, "--5s");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().end.seconds(), 595.0);
    }

    #[test]
    fn an_untouched_cursor_prompt_submits_the_placeholder_value() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press_ctrl(&mut app, KeyCode::Char('l')); // begin marker -> 10s

        press(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Enter); // submit without typing anything
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 10.0);
    }

    #[test]
    fn a_bad_marker_expression_is_reported_not_applied() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.run_command("b banana");
        assert!(app.status.as_ref().unwrap().is_error);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 0.0);
    }

    #[test]
    fn a_nonfinite_marker_expression_is_rejected_like_any_other_bad_input() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.run_command("b nan");
        assert!(app.status.as_ref().unwrap().is_error);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 0.0);

        app.run_command("e inf");
        assert!(app.status.as_ref().unwrap().is_error);
        assert_eq!(app.session.as_ref().unwrap().end.seconds(), 600.0);
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_first() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));
        app.run_command("q");
        assert!(matches!(app.overlay, Overlay::ConfirmDiscard(_)));
        assert!(!app.should_quit);
    }

    #[test]
    fn quit_bang_discards_without_asking() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));
        app.run_command("q!");
        assert!(app.session.is_none());
    }

    #[test]
    fn unknown_commands_are_reported() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.run_command("nonsense");
        let status = app.status.as_ref().unwrap();
        assert!(status.is_error);
        assert!(status.text.contains("Unknown command"));
    }

    #[test]
    fn help_is_reachable_from_the_command_line() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.run_command("help");
        assert!(matches!(app.overlay, Overlay::Help));
    }

    #[test]
    fn wq_on_an_unmodified_file_closes_without_running_the_save_pipeline() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(!app.session.as_ref().unwrap().is_dirty());

        app.run_command("wq");
        assert!(
            app.session.is_none(),
            "should close immediately, like vim's :x"
        );
        assert!(
            app.save_rx.is_none(),
            "must not spawn the save pipeline for a no-op exit"
        );
    }

    #[test]
    fn wq_on_a_modified_file_still_saves() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty
        press(&mut app, KeyCode::Esc);

        app.run_command("wq");
        assert!(
            app.session.is_some(),
            "the file stays open while the save runs"
        );
        assert!(app.save_rx.is_some());
        assert_eq!(
            app.pending_nav_after_save,
            Some(crate::app::PendingNav::CloseFile)
        );
    }
}
