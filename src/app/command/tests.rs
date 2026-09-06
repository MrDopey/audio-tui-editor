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
