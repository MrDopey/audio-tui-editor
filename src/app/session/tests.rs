    use super::*;
    use crate::app::tests::{app, press, type_text};
    use crate::app::{Mode, Overlay};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn markers_default_to_the_whole_file_and_move_by_configured_steps() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 0.0);
        assert_eq!(session.end.seconds(), 600.0);
        assert_eq!(session.active, MarkerKind::Begin);

        // Fine step is one second by default.
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 1.0);
        // Large step is ten.
        crate::app::tests::press_ctrl(&mut app, KeyCode::Char('l'));
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 11.0);
        crate::app::tests::press_ctrl(&mut app, KeyCode::Char('h'));
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 1.0);
        assert!(app.session.as_ref().unwrap().markers_dirty);
    }

    #[test]
    fn tab_switches_the_active_marker() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.session.as_ref().unwrap().active, MarkerKind::End);

        // Tab already picked End up from its own position, so Left/Right
        // drags it immediately — no separate "engage" step needed.
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.session.as_ref().unwrap().end.seconds(), 599.0);
    }

    #[test]
    fn b_and_e_jump_the_cursor_and_switch_which_marker_follows_it() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e')); // EDIT

        // `b` opens a typed jump; the cursor moves there and Begin hugs it.
        press(&mut app, KeyCode::Char('b'));
        type_text(&mut app, "100");
        press(&mut app, KeyCode::Enter);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 100.0);
        assert_eq!(session.active, MarkerKind::Begin);

        // `e` does the same for End. End starts at the file's end (600), and
        // a bare number is relative to that (same as `-300`), not absolute.
        press(&mut app, KeyCode::Char('e'));
        type_text(&mut app, "-300");
        press(&mut app, KeyCode::Enter);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.end.seconds(), 300.0);
        assert_eq!(session.active, MarkerKind::End);
    }

    #[test]
    fn b_and_e_relative_jumps_are_relative_to_their_own_marker_not_the_cursor() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e')); // EDIT, Begin at 0, End at 600

        // Leave the cursor somewhere unrelated to either marker.
        app.session.as_mut().unwrap().player.seek_to(300.0);

        // `+10` for `b` means 10s after Begin's own position (0), not
        // 10s after the cursor (300).
        press(&mut app, KeyCode::Char('b'));
        type_text(&mut app, "+10");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 10.0);

        // Move the cursor away again before jumping End.
        app.session.as_mut().unwrap().player.seek_to(50.0);

        // `-10` for `e` means 10s before End's own position (600), not
        // 10s before the cursor (50).
        press(&mut app, KeyCode::Char('e'));
        type_text(&mut app, "-10");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().end.seconds(), 590.0);
    }

    #[test]
    fn typed_jump_beyond_duration_clamps_without_crossing() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e')); // EDIT, Begin active, End at 600

        // Drag Begin's cursor past End: allowed to stand, not clamped.
        press(&mut app, KeyCode::Char('b'));
        type_text(&mut app, "650"); // clamped to the file's duration on seek
        press(&mut app, KeyCode::Enter);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 600.0);
        assert_eq!(session.end.seconds(), 600.0);
        assert!(!session.is_crossed(), "600 == 600 isn't a crossing");
    }

    #[test]
    fn crossing_the_other_marker_is_transient_until_settled() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e')); // EDIT, Begin active, End at 600

        // Force a real crossing directly, then settle it: Begin/End swap
        // values (preserving the 200s range width) and `active` flips to
        // whichever marker now holds the cursor's time (400, still End).
        {
            let session = app.session.as_mut().unwrap();
            session.begin = crate::timespec::Marker::absolute(400.0, 600.0);
            session.end = crate::timespec::Marker::absolute(200.0, 600.0);
            session.player.seek_to(400.0);
            session.active = MarkerKind::Begin;
        }
        assert!(app.session.as_ref().unwrap().is_crossed());
        assert!(app.session.as_mut().unwrap().settle_crossed_markers());
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 200.0);
        assert_eq!(session.end.seconds(), 400.0);
        assert_eq!(session.active, MarkerKind::End);
        assert!(!session.is_crossed());
    }

    #[test]
    fn tab_settles_a_pending_crossing_immediately() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        {
            let session = app.session.as_mut().unwrap();
            session.begin = crate::timespec::Marker::absolute(400.0, 600.0);
            session.end = crate::timespec::Marker::absolute(200.0, 600.0);
            session.player.seek_to(400.0);
            session.active = MarkerKind::Begin;
        }
        press(&mut app, KeyCode::Tab);
        let session = app.session.as_ref().unwrap();
        assert!(!session.is_crossed(), "Tab should settle before toggling");
        assert_eq!(session.begin.seconds(), 200.0);
        assert_eq!(session.end.seconds(), 400.0);
        // Tab toggles from Begin (active when it was pressed) to End,
        // regardless of whatever settling itself did to `active`.
        assert_eq!(session.active, MarkerKind::End);
        assert!(app.status.as_ref().is_some_and(|s| !s.is_error));
    }

    #[test]
    fn an_idle_tick_settles_a_pending_crossing() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        {
            let session = app.session.as_mut().unwrap();
            session.begin = crate::timespec::Marker::absolute(400.0, 600.0);
            session.end = crate::timespec::Marker::absolute(200.0, 600.0);
            session.player.seek_to(400.0);
            session.active = MarkerKind::Begin;
        }
        app.last_cursor_move = Some(std::time::Instant::now());

        // Fresh from the last move: too soon to auto-correct.
        app.tick();
        assert!(app.session.as_ref().unwrap().is_crossed());

        // Backdate the last move past the settle delay.
        app.last_cursor_move = Some(
            std::time::Instant::now()
                - crate::app::CURSOR_SETTLE_DELAY
                - std::time::Duration::from_millis(1),
        );
        app.tick();
        let session = app.session.as_ref().unwrap();
        assert!(!session.is_crossed(), "idle long enough, should now settle");
        assert_eq!(session.begin.seconds(), 200.0);
        assert_eq!(session.end.seconds(), 400.0);
        assert_eq!(session.active, MarkerKind::End);
    }

    #[test]
    fn markers_cannot_cross_each_other() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.run_command("e -500"); // end -> 100 (500s before its own 600)
        app.run_command("b +200"); // begin -> 200 (200s after its own 0)
        let session = app.session.as_ref().unwrap();
        assert!(session.begin.seconds() < session.end.seconds());
        assert!((session.begin.seconds() - 99.99).abs() < 0.001);
    }

    #[test]
    fn automatic_suggestions_do_not_override_manual_markers() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // manual nudge

        let session = app.session.as_mut().unwrap();
        session.adopt_suggestion(TrimSuggestion {
            begin: 12.0,
            end: 500.0,
            begin_detected: true,
            end_detected: true,
        });
        assert_eq!(session.begin.seconds(), 1.0, "manual edits win");
    }

    #[test]
    fn recalculating_markers_keeps_dirty_true_until_the_suggestion_lands() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // manual nudge, now dirty
        assert!(app.session.as_ref().unwrap().is_dirty());

        press(&mut app, KeyCode::Char('a')); // recalculate
        assert!(
            app.session.as_ref().unwrap().is_dirty(),
            "dirty must stay true until the new suggestion actually replaces the markers"
        );

        // Once the suggestion lands, it overrides the manual edit and the
        // session is clean again.
        let session = app.session.as_mut().unwrap();
        session.adopt_suggestion(TrimSuggestion {
            begin: 12.0,
            end: 500.0,
            begin_detected: true,
            end_detected: true,
        });
        assert_eq!(session.begin.seconds(), 12.0);
        assert!(!session.is_dirty());
    }

    #[test]
    fn automatic_suggestions_apply_when_markers_are_untouched() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        let session = app.session.as_mut().unwrap();
        session.adopt_suggestion(TrimSuggestion {
            begin: 12.0,
            end: 500.0,
            begin_detected: true,
            end_detected: true,
        });
        assert_eq!(session.begin.seconds(), 12.0);
        assert_eq!(session.end.seconds(), 500.0);
    }

    #[test]
    fn adopting_a_suggestion_resets_the_cursor_so_dragging_continues_from_it() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        // Leave the cursor somewhere unrelated to the coming suggestion.
        app.session.as_mut().unwrap().player.seek_to(300.0);

        let session = app.session.as_mut().unwrap();
        session.adopt_suggestion(TrimSuggestion {
            begin: 12.0,
            end: 500.0,
            begin_detected: true,
            end_detected: true,
        });
        assert!(
            (session.player.position() - 12.0).abs() < 0.01,
            "cursor should follow the active (Begin) marker to its new position"
        );

        // Left/Right now drags Begin from 12, not from the stale 300.
        press(&mut app, KeyCode::Char('l'));
        assert!((app.session.as_ref().unwrap().begin.seconds() - 13.0).abs() < 0.01);
    }

    #[test]
    fn resetting_markers_resets_the_cursor_too() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.session.as_mut().unwrap().player.seek_to(300.0);

        press(&mut app, KeyCode::Char('r'));
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 0.0);
        assert!((session.player.position() - 0.0).abs() < 0.01);

        press(&mut app, KeyCode::Char('l'));
        assert!((app.session.as_ref().unwrap().begin.seconds() - 1.0).abs() < 0.01);
    }

    #[test]
    fn metadata_fields_are_editable_and_track_changes() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode, Mode::Metadata);

        let session = app.session.as_ref().unwrap();
        assert_eq!(session.fields[0].label, "Title");
        assert_eq!(session.fields[0].display(), "Interview");
        assert!(!session.metadata_dirty());

        press(&mut app, KeyCode::Enter); // edit the field
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Enter);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.fields[0].display(), "Intervie");
        assert!(session.metadata_dirty());
        assert_eq!(
            session.metadata_edits().get("title"),
            Some(&Some("Intervie".to_string()))
        );
    }

    #[test]
    fn clearing_a_metadata_field_requests_its_removal() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Enter);
        app.on_key(ratatui::crossterm::event::KeyEvent::new(
            KeyCode::Char('u'),
            ratatui::crossterm::event::KeyModifiers::CONTROL,
        ));
        press(&mut app, KeyCode::Enter);
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.metadata_edits().get("title"), Some(&None));
    }

    #[test]
    fn metadata_navigation_moves_between_fields() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.session.as_ref().unwrap().field_index, 1);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(
            app.session.as_ref().unwrap().field_index,
            METADATA_FIELDS.len() - 1
        );
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.session.as_ref().unwrap().field_index, 0);
    }

    #[test]
    fn volume_changes_persist_across_files() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j')); // volume down
        let reduced = app.session.as_ref().unwrap().player.volume();
        assert_eq!(reduced, 95.0);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.session.as_ref().unwrap().player.volume(), 95.0);
    }
