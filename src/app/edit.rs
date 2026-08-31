//! EDIT mode: marker navigation (design §8–§10).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, MarkerKind, Overlay, Prompt, PromptKind};
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
                self.nudge_marker(active, if ctrl { -large } else { -fine })
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.nudge_marker(active, if ctrl { large } else { fine })
            }
            KeyCode::Up | KeyCode::Char('k') if ctrl => self.cycle_song(-1),
            KeyCode::Down | KeyCode::Char('j') if ctrl => self.cycle_song(1),
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(session) = &mut self.session {
                    session.active = session.active.toggled();
                }
            }
            KeyCode::Char('b') => self.set_marker_at_playhead(MarkerKind::Begin),
            KeyCode::Char('e') => self.set_marker_at_playhead(MarkerKind::End),
            KeyCode::Char('B') => self.prompt_for_marker(MarkerKind::Begin),
            KeyCode::Char('E') => self.prompt_for_marker(MarkerKind::End),
            KeyCode::Char('i') => self.prompt_for_marker(active),
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
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, press, press_ctrl};
    use crate::app::{Mode, Overlay};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn q_behaves_like_esc_in_play_edit_and_metadata_modes() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::Play, "q in EDIT should behave like Esc");

        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode, Mode::Metadata);
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::Play, "q in METADATA should behave like Esc");

        press(&mut app, KeyCode::Char('q'));
        assert_eq!(app.mode, Mode::Browse, "q in PLAY should close the file like Esc");
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
