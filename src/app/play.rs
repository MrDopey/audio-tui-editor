//! PLAY mode: playback, seeking and volume (design §6).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, PendingNav};
use crate::player::AudioPlayer;

impl App {
    pub(super) fn on_play_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let small = self.config.playback.small_seek_seconds;
        let large = self.config.playback.large_seek_seconds;
        let step = self.config.playback.volume_step;

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.request_nav(PendingNav::CloseFile),
            KeyCode::Char(' ') => self.with_player(AudioPlayer::toggle),
            KeyCode::Left | KeyCode::Char('h') => {
                let delta = if ctrl { -large } else { -small };
                self.with_player(|p| p.seek_by(delta));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let delta = if ctrl { large } else { small };
                self.with_player(|p| p.seek_by(delta));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if ctrl {
                    self.cycle_song(-1);
                } else {
                    self.change_volume(step);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if ctrl {
                    self.cycle_song(1);
                } else {
                    self.change_volume(-step);
                }
            }
            KeyCode::Char('g') => self.with_player(|p| p.seek_to(0.0)),
            KeyCode::Char('G') => self.with_player(|p| p.seek_to(p.duration())),
            KeyCode::Char('c') => self.prompt_for_cursor(),
            KeyCode::Char('e') => self.enter_edit_mode(),
            KeyCode::Char('m') => self.mode = crate::app::Mode::Metadata,
            KeyCode::Char('I') => self.toggle_cover_art(),
            _ => {}
        }
    }

    /// This lives in PLAY because it is only ever reachable via its `e` key.
    /// Deliberately does not kick off automatic marker detection — that only
    /// runs when the user asks for it with `a` (design: EDIT opens on
    /// whatever markers already exist, not a fresh auto-trim guess).
    fn enter_edit_mode(&mut self) {
        if self.session.is_some() {
            self.mode = crate::app::Mode::Edit;
        }
    }

    fn change_volume(&mut self, delta: f64) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.player.adjust_volume(delta);
        self.volume = session.player.volume();
        let volume = self.volume;
        self.info(format!("Volume {volume:.0}%"));
    }
}

impl App {
    /// Shared with EDIT mode (space to toggle, `p` to seek-and-play).
    pub(super) fn with_player(&mut self, f: impl FnOnce(&mut AudioPlayer)) {
        if let Some(session) = &mut self.session {
            f(&mut session.player);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, app_with_cover_art_support, press, press_ctrl};
    use crate::app::Overlay;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn left_right_seeking_uses_the_configured_small_and_large_steps() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.session.as_ref().unwrap().player.position(), 10.0);
        press_ctrl(&mut app, KeyCode::Char('l'));
        assert_eq!(app.session.as_ref().unwrap().player.position(), 70.0);
        press_ctrl(&mut app, KeyCode::Char('h'));
        assert_eq!(app.session.as_ref().unwrap().player.position(), 10.0);
    }

    #[test]
    fn g_and_capital_g_seek_to_the_start_and_end_in_play_mode() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.session.as_ref().unwrap().player.position(), 0.0);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.session.as_ref().unwrap().player.position(), 600.0);
    }

    #[test]
    fn ctrl_jk_cycles_songs_and_wraps_at_both_ends() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0), ("c.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(app.session.as_ref().unwrap().index, 1);

        press_ctrl(&mut app, KeyCode::Down);
        assert_eq!(app.session.as_ref().unwrap().index, 2);

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(app.session.as_ref().unwrap().index, 0, "wraps around");

        press_ctrl(&mut app, KeyCode::Char('k'));
        assert_eq!(
            app.session.as_ref().unwrap().index,
            2,
            "wraps the other way"
        );
    }

    #[test]
    fn c_opens_a_cursor_prompt_in_play_mode() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('l')); // cursor -> 10s

        press(&mut app, KeyCode::Char('c'));
        let prompt = app.prompt.as_ref().unwrap();
        assert!(prompt.buffer.is_empty(), "buffer starts empty, not prefilled");
        assert_eq!(prompt.placeholder.as_deref(), Some("00:10"));
    }

    #[test]
    fn capital_i_toggles_cover_art_in_play_mode() {
        let mut app = app_with_cover_art_support(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(app.show_cover_art, "starts shown");
        press(&mut app, KeyCode::Char('I'));
        assert!(!app.show_cover_art);
        press(&mut app, KeyCode::Char('I'));
        assert!(app.show_cover_art);
    }

    #[test]
    fn toggling_cover_art_without_terminal_support_warns_instead_of_flipping_the_flag() {
        let mut app = app(&[("a.opus", 60.0)]); // cover_art_supported: false
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        let before = app.show_cover_art;
        press(&mut app, KeyCode::Char('I'));
        assert_eq!(
            app.show_cover_art, before,
            "the flag must not flip without terminal support"
        );
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn plain_j_k_change_volume_but_ctrl_j_k_does_not() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        let starting_volume = app.session.as_ref().unwrap().player.volume();

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(
            app.session.as_ref().unwrap().player.volume(),
            starting_volume,
            "ctrl-j cycles songs, it must not touch volume"
        );

        press(&mut app, KeyCode::Char('k'));
        assert_eq!(
            app.session.as_ref().unwrap().player.volume(),
            starting_volume + app.config.playback.volume_step,
            "plain k still changes volume"
        );
    }
}
