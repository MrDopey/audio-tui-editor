    use super::*;
    use crate::config::Config;
    use ratatui::crossterm::event::KeyModifiers;
    use std::collections::BTreeMap;

    pub(super) fn info(name: &str, duration: f64) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("/rec/{name}")),
            duration,
            tags: BTreeMap::from([("title".to_string(), "Interview".to_string())]),
            ..probe::fixture()
        }
    }

    /// An app with no audio device and no open file, for state-machine tests.
    pub(super) fn app(names: &[(&str, f64)]) -> App {
        let files = names.iter().map(|(n, d)| info(n, *d)).collect();
        App::new(
            PathBuf::from("/rec"),
            files,
            Vec::new(),
            Config::default(),
            AudioOutput::silent(),
        )
    }

    pub(super) fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub(super) fn press_ctrl(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::CONTROL));
    }

    pub(super) fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn starts_with_the_in_place_warning_then_browses() {
        let mut app = app(&[("a.opus", 60.0)]);
        assert!(matches!(app.overlay, Overlay::Warning));
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn entering_play_from_browse() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Play);
    }

    #[test]
    fn edit_esc_returns_to_play() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Play);
    }

    #[test]
    fn metadata_esc_returns_to_play() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode, Mode::Metadata);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Play);
    }

    #[test]
    fn play_esc_returns_to_browse_and_closes_session() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.session.is_none());
    }

    #[test]
    fn entering_edit_mode_does_not_auto_trim() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e')); // EDIT
        assert_eq!(app.mode, Mode::Edit);
        assert!(
            !app.session.as_ref().unwrap().auto.is_running(),
            "auto-trim must wait for `a`, not run just from opening EDIT"
        );
    }

    #[test]
    fn a_single_ctrl_c_only_warns() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press_ctrl(&mut app, KeyCode::Char('c'));
        assert!(!app.should_quit);
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn two_consecutive_ctrl_c_presses_force_quit() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press_ctrl(&mut app, KeyCode::Char('c'));
        press_ctrl(&mut app, KeyCode::Char('c'));
        assert!(app.should_quit);
    }

    #[test]
    fn an_intervening_key_disarms_ctrl_c() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press_ctrl(&mut app, KeyCode::Char('c'));
        press(&mut app, KeyCode::Char('j')); // unrelated key in BROWSE
        press_ctrl(&mut app, KeyCode::Char('c'));
        assert!(!app.should_quit, "a non-Ctrl-C key should reset the arm");
    }

    #[test]
    fn file_rows_show_name_duration_and_format() {
        let mut app = app(&[("interview-001.opus", 6151.0)]);
        let rows = app.file_rows();
        assert_eq!(rows[0].0, "interview-001.opus");
        assert_eq!(rows[0].1, "01:42:31");
        assert_eq!(rows[0].2, "opus");
    }

    #[test]
    fn file_rows_are_recomputed_after_a_rescan_replaces_the_file_list() {
        let mut app = app(&[("a.opus", 60.0)]);
        assert_eq!(app.file_rows()[0].0, "a.opus");

        app.files = vec![info("b.opus", 30.0)];
        app.files_generation += 1;
        assert_eq!(
            app.file_rows()[0].0,
            "b.opus",
            "a stale cache must not survive a file-list change"
        );
    }

    #[test]
    fn pressing_enter_on_an_error_overlay_reveals_the_detail() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.fail("Could not save the file.", "ffmpeg: boom");
        match &app.overlay {
            Overlay::Error { showing_detail, .. } => assert!(!showing_detail),
            _ => panic!("expected an error overlay"),
        }
        press(&mut app, KeyCode::Enter);
        match &app.overlay {
            Overlay::Error { showing_detail, .. } => assert!(showing_detail),
            _ => panic!("expected an error overlay"),
        }
    }

    #[test]
    fn opening_a_file_with_no_files_present_warns() {
        let mut app = app(&[]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(app.session.is_none());
    }
