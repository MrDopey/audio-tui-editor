//! The `:` command line and prompt submission (design §19).

use super::{App, MarkerKind, Overlay, PendingNav, Prompt, PromptKind};
use crate::batch::RunMode;
use crate::timespec::Marker;

impl App {
    pub(super) fn submit_prompt(&mut self, prompt: Prompt) {
        let input = prompt.buffer.trim().to_string();
        match prompt.kind {
            PromptKind::Command => self.run_command(&input),
            PromptKind::Search => {
                if input.is_empty() {
                    return;
                }
                self.last_search = input;
                self.repeat_search(true);
            }
            PromptKind::Marker(kind) => self.set_marker_from_expression(kind, &input),
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

    fn set_marker_from_expression(&mut self, kind: MarkerKind, input: &str) {
        let Some(duration) = self.session.as_ref().map(super::Session::duration) else {
            self.warn("No file is open.");
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
                    self.warn("No file is open.");
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
    use super::super::tests::{app, press};
    use crate::app::{Overlay, Prompt, PromptKind};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn prompt_editing_supports_backspace_and_cursor_movement() {
        let mut prompt = Prompt::new(PromptKind::Command, String::new());
        for c in "wq".chars() {
            prompt.insert(c);
        }
        assert_eq!(prompt.buffer, "wq");
        prompt.backspace();
        assert_eq!(prompt.buffer, "w");
        prompt.cursor = 0;
        prompt.insert('x');
        assert_eq!(prompt.buffer, "xw");
    }

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
        assert!(app.session.is_none(), "should close immediately, like vim's :x");
        assert!(app.save_rx.is_none(), "must not spawn the save pipeline for a no-op exit");
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
        assert!(app.session.is_some(), "the file stays open while the save runs");
        assert!(app.save_rx.is_some());
        assert_eq!(
            app.pending_nav_after_save,
            Some(crate::app::PendingNav::CloseFile)
        );
    }
}
