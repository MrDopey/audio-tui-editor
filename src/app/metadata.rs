//! METADATA mode: editable tag fields (design §18).

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Mode, Overlay, Prompt, PromptKind};
use crate::media::probe::METADATA_FIELDS;

impl App {
    pub(super) fn on_metadata_key(&mut self, key: KeyEvent) {
        if self.session.is_none() {
            self.mode = Mode::Browse;
            return;
        }
        let last = METADATA_FIELDS.len().saturating_sub(1);

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Play,
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(session) = &mut self.session {
                    session.field_index = (session.field_index + 1).min(last);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(session) = &mut self.session {
                    session.field_index = session.field_index.saturating_sub(1);
                }
            }
            KeyCode::Char('g') => {
                if let Some(session) = &mut self.session {
                    session.field_index = 0;
                }
            }
            KeyCode::Char('G') => {
                if let Some(session) = &mut self.session {
                    session.field_index = last;
                }
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                let current = self
                    .session
                    .as_ref()
                    .map(|s| (s.field_index, s.fields[s.field_index].display().to_string()));
                if let Some((index, value)) = current {
                    self.prompt = Some(Prompt::new(PromptKind::MetadataField(index), value));
                }
            }
            KeyCode::Char('u') => {
                if let Some(session) = &mut self.session {
                    let field = &mut session.fields[session.field_index];
                    field.value = field.original.clone();
                }
                self.info("Field reverted.");
            }
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }
}
