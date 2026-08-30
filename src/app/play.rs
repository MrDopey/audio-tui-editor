//! PLAY mode: playback, seeking and volume (design §6).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Overlay, PendingNav, Prompt, PromptKind};
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
            KeyCode::Up | KeyCode::Char('k') => self.change_volume(step),
            KeyCode::Down | KeyCode::Char('j') => self.change_volume(-step),
            KeyCode::Char('g') => self.with_player(|p| p.seek_to(0.0)),
            KeyCode::Char('G') => self.with_player(|p| p.seek_to(p.duration())),
            KeyCode::Char('e') => self.enter_edit_mode(),
            KeyCode::Char('m') => self.mode = crate::app::Mode::Metadata,
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }

    /// This lives in PLAY because it is only ever reachable via its `e` key.
    fn enter_edit_mode(&mut self) {
        let config = self.config.clone();
        if let Some(session) = &mut self.session {
            session.start_auto_markers(&config);
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
    use super::super::tests::{app, press, press_ctrl};
    use crate::app::Overlay;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn play_seeking_and_volume_use_configured_steps() {
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
}
