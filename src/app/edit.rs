//! EDIT mode: marker navigation (design §8–§10).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, MarkerKind};
use crate::player::AudioPlayer;

impl App {
    pub(super) fn on_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let fine = self.config.editing.fine_step_seconds;
        let large = self.config.editing.large_step_seconds;

        let Some(active) = self.session.as_ref().map(|s| s.active) else {
            self.mode = crate::app::Mode::Browse;
            return;
        };

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = crate::app::Mode::Play,
            KeyCode::Left | KeyCode::Char('h') => {
                self.move_cursor(if ctrl { -large } else { -fine })
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.move_cursor(if ctrl { large } else { fine })
            }
            KeyCode::Up | KeyCode::Char('k') if ctrl => self.cycle_song(-1),
            KeyCode::Down | KeyCode::Char('j') if ctrl => self.cycle_song(1),
            KeyCode::Tab | KeyCode::BackTab => {
                // Switching which marker you're editing is a natural point
                // to confirm a pending cursor-crossing correction (see
                // `Session::settle_crossed_markers`), same as an idle pause
                // or a save. Settling can itself flip `active` (to keep
                // naming whichever marker holds the cursor's time), so Tab
                // computes its own toggle from `active` as it was when the
                // key was pressed — not from whatever settling just left it
                // at — otherwise the two toggles could cancel out.
                let corrected = self
                    .session
                    .as_mut()
                    .is_some_and(super::Session::settle_crossed_markers);
                if let Some(session) = &mut self.session {
                    session.active = active.toggled();
                    // Pick up the other marker from where it actually sits,
                    // so the next Left/Right resumes hugging it smoothly
                    // instead of yanking it to wherever playback happened
                    // to be.
                    let target = session.marker(session.active).seconds();
                    session.player.seek_to(target);
                }
                if corrected {
                    self.info("Begin/End swapped.");
                }
            }
            KeyCode::Char('b') => self.prompt_for_marker(MarkerKind::Begin),
            KeyCode::Char('e') => self.prompt_for_marker(MarkerKind::End),
            KeyCode::Char('c') => self.prompt_for_cursor(),
            KeyCode::Char('g') => self.seek_to_active_marker(),
            KeyCode::Char(' ') => self.with_player(AudioPlayer::toggle),
            KeyCode::Char('p') => {
                let target = self.session.as_ref().map(|s| s.marker(active).seconds());
                if let Some(target) = target {
                    self.with_player(|p| {
                        p.seek_to(target);
                        p.play();
                    });
                }
            }
            KeyCode::Char('a') => self.recalculate_auto_markers(),
            KeyCode::Char('r') => self.reset_markers(),
            KeyCode::Char('i') => self.toggle_cover_art(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, app_with_cover_art_support, press, press_ctrl};
    use crate::app::{Mode, Overlay};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn q_in_edit_mode_behaves_like_esc() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::Play, "q in EDIT should behave like Esc");
    }

    #[test]
    fn q_in_metadata_mode_behaves_like_esc() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode, Mode::Metadata);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::Play, "q in METADATA should behave like Esc");
    }

    #[test]
    fn q_in_play_mode_closes_the_file_like_esc() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(
            app.mode,
            Mode::Browse,
            "q in PLAY should close the file like Esc"
        );
        assert!(app.session.is_none());
    }

    #[test]
    fn ctrl_j_k_cycle_songs_and_stay_in_edit_mode() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e')); // EDIT
        assert_eq!(app.mode, Mode::Edit);

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(app.mode, Mode::Edit, "should stay in EDIT after cycling");
        assert_eq!(app.session.as_ref().unwrap().index, 1);

        press_ctrl(&mut app, KeyCode::Up);
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.session.as_ref().unwrap().index, 0);
    }

    #[test]
    fn cycling_songs_keeps_playing_if_the_outgoing_song_was_playing() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e')); // EDIT
        app.with_player(crate::player::AudioPlayer::play);
        assert!(app.session.as_ref().unwrap().player.is_playing());

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(app.session.as_ref().unwrap().index, 1);
        assert!(
            app.session.as_ref().unwrap().player.is_playing(),
            "switching songs should not silently pause playback"
        );
    }

    #[test]
    fn cycling_songs_stays_paused_if_the_outgoing_song_was_paused() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e')); // EDIT
        assert!(!app.session.as_ref().unwrap().player.is_playing());

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(app.session.as_ref().unwrap().index, 1);
        assert!(!app.session.as_ref().unwrap().player.is_playing());
    }

    #[test]
    fn g_seeks_to_the_active_marker_without_playing() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press_ctrl(&mut app, KeyCode::Char('l')); // move begin marker to 10s
        press(&mut app, KeyCode::Char('g'));
        let session = app.session.as_ref().unwrap();
        assert!(!session.player.is_playing(), "g must not start playback");
        assert!((session.player.position() - 10.0).abs() < 0.01);
    }

    #[test]
    fn c_opens_a_cursor_prompt_shadowed_by_the_cursors_position() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('c'));
        let prompt = app.prompt.as_ref().unwrap();
        assert!(
            prompt.buffer.is_empty(),
            "buffer starts empty, not prefilled"
        );
        assert_eq!(prompt.placeholder.as_deref(), Some("00:00"));
    }

    #[test]
    fn i_toggles_cover_art_in_edit_mode() {
        let mut app = app_with_cover_art_support(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e')); // EDIT
        assert!(app.show_cover_art, "starts shown");
        press(&mut app, KeyCode::Char('i'));
        assert!(!app.show_cover_art);
    }

    #[test]
    fn play_from_marker_seeks_and_plays() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press_ctrl(&mut app, KeyCode::Char('l')); // move begin marker to 10s
        press(&mut app, KeyCode::Char('p'));
        let session = app.session.as_ref().unwrap();
        assert!(session.player.is_playing());
        // A silent player tracks position from wall-clock elapsed time, so a
        // few microseconds pass between the seek and this check.
        assert!((session.player.position() - 10.0).abs() < 0.01);
    }
}
