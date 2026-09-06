    use super::super::tests::{app, press, press_ctrl, type_text};
    use crate::app::{Overlay, PendingNav};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn browse_navigation_follows_vim_motions() {
        let mut app = app(&[
            ("a.opus", 1.0),
            ("b.opus", 2.0),
            ("c.opus", 3.0),
            ("d.opus", 4.0),
        ]);
        app.overlay = Overlay::None;
        app.page_rows = 4;

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.selected, 3);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.selected, 0);
        press_ctrl(&mut app, KeyCode::Char('d'));
        assert_eq!(app.selected, 2);
        press_ctrl(&mut app, KeyCode::Char('u'));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn navigation_is_clamped_to_the_list() {
        let mut app = app(&[("a.opus", 1.0), ("b.opus", 2.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn search_jumps_to_the_first_match() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "gam");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn repeat_search_wraps_around_to_a_single_match() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "gam");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 2);

        // `n` wraps around to the only match again.
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn search_with_no_match_reports_an_error() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "nothing");
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn search_previews_the_match_live_before_enter_is_pressed() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.selected, 0, "no preview yet with an empty buffer");
        type_text(&mut app, "gam");
        assert_eq!(app.selected, 2, "eagerly jumps to the match while typing");
        // Committing shouldn't search again past the already-previewed hit.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn escaping_a_search_restores_the_pre_search_selection() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;
        app.selected = 1;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "gam");
        assert_eq!(app.selected, 2, "previewed the match");

        press(&mut app, KeyCode::Esc);
        assert_eq!(
            app.selected, 1,
            "cancelling restores where the search started"
        );
        assert!(app.prompt.is_none());
    }

    #[test]
    fn a_live_search_with_no_match_holds_at_the_starting_selection() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;
        app.selected = 1;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zzz");
        assert_eq!(
            app.selected, 1,
            "no match yet, stays put rather than jumping"
        );
    }

    #[test]
    fn search_matches_are_case_insensitive() {
        let mut app = app(&[("One.opus", 1.0), ("two.opus", 2.0), ("three.opus", 3.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "ONE");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn capital_n_repeats_the_last_search_backwards() {
        let mut app = app(&[("One.opus", 1.0), ("two.opus", 2.0), ("three.opus", 3.0)]);
        app.overlay = Overlay::None;
        app.last_search = "o".to_string();
        app.selected = 2;
        press(&mut app, KeyCode::Char('N'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn browse_q_quits_when_nothing_is_open() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn choosing_save_in_the_discard_dialog_remembers_the_original_target() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty

        app.selected = 1;
        app.open_selected();
        assert!(matches!(
            app.overlay,
            Overlay::ConfirmDiscard(PendingNav::Open(1))
        ));

        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.pending_nav_after_save, Some(PendingNav::Open(1)));
        assert!(app.save_rx.is_some());
    }

    #[test]
    fn choosing_no_in_the_discard_dialog_discards_and_continues() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty

        app.selected = 1;
        app.open_selected();
        assert!(matches!(
            app.overlay,
            Overlay::ConfirmDiscard(PendingNav::Open(1))
        ));

        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.session.as_ref().unwrap().index, 1);
        assert_eq!(app.pending_nav_after_save, None);
    }

    #[test]
    fn escaping_the_discard_dialog_cancels_and_keeps_the_file_open() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty

        app.selected = 1;
        app.open_selected();
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(
            app.session.as_ref().unwrap().index,
            0,
            "stays on the dirty file"
        );
    }

    #[test]
    fn a_successful_save_continues_to_the_remembered_navigation_target() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // opens a.opus, index 0

        app.pending_nav_after_save = Some(PendingNav::Open(1));
        let (tx, rx) = std::sync::mpsc::channel();
        app.save_rx = Some(rx);
        tx.send(Ok(super::super::save::fake_save_outcome("/rec/a.opus")))
            .unwrap();
        app.poll_save();

        assert_eq!(
            app.session.as_ref().map(|s| s.index),
            Some(1),
            "must continue to the file the user selected, not just close to BROWSE"
        );
        assert!(app.pending_nav_after_save.is_none());
    }

    #[test]
    fn opening_a_file_with_no_files_present_warns_and_does_not_open() {
        let mut app = app(&[]);
        app.overlay = Overlay::None;
        app.open_selected();
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(app.session.is_none());
    }
