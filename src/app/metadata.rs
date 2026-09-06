//! METADATA mode: editable tag fields (design §18).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Mode, Prompt, PromptKind};

impl App {
    pub(super) fn on_metadata_key(&mut self, key: KeyEvent) {
        let Some(session) = &self.session else {
            self.mode = Mode::Browse;
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = session.fields.len().saturating_sub(1);

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.mode = Mode::Play,
            KeyCode::Char('j') | KeyCode::Down if ctrl => self.cycle_song(1),
            KeyCode::Char('k') | KeyCode::Up if ctrl => self.cycle_song(-1),
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
            KeyCode::Char('/') => {
                if let Some(session) = &self.session {
                    self.search_origin = Some(session.field_index);
                }
                self.prompt = Some(Prompt::new(PromptKind::Search, String::new()));
            }
            KeyCode::Char('n') => self.repeat_search(true),
            KeyCode::Char('N') => self.repeat_search(false),
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
            _ => {}
        }
    }

    /// Live "as you type" preview for `/` search over metadata fields — the
    /// METADATA-mode counterpart of [`App::live_search`] in BROWSE: jump
    /// `field_index` to the first field (by label or key) at or after
    /// `search_origin`, wrapping, so the field under the cursor previews
    /// before `Enter` commits to it.
    pub(super) fn live_search_fields(&mut self, needle: &str) {
        let Some(origin) = self.search_origin else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let count = session.fields.len();
        if count == 0 {
            return;
        }
        if needle.is_empty() {
            session.field_index = origin.min(count - 1);
            return;
        }
        let needle = needle.to_lowercase();
        let found = (0..count)
            .map(|offset| (origin + offset) % count)
            .find(|&index| field_matches(&session.fields[index], &needle));
        session.field_index = found.unwrap_or(origin.min(count - 1));
    }

    /// `n`/`N` over metadata fields: repeat the last `/` search, wrapping,
    /// starting after (or before) the currently selected field.
    pub(super) fn repeat_search_fields(&mut self, forward: bool) {
        if self.last_search.is_empty() {
            self.warn("No search pattern. Press / to search.");
            return;
        }
        let needle = self.last_search.to_lowercase();
        let found = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let count = session.fields.len();
            if count == 0 {
                return;
            }
            let start = session.field_index;
            let found = (1..=count)
                .map(|offset| {
                    if forward {
                        (start + offset) % count
                    } else {
                        (start + count * count - offset) % count
                    }
                })
                .find(|&index| field_matches(&session.fields[index], &needle));
            if let Some(index) = found {
                session.field_index = index;
            }
            found
        };
        let pattern = self.last_search.clone();
        if found.is_some() {
            self.info(format!("/{pattern}"));
        } else {
            self.warn(format!("Pattern not found: {pattern}"));
        }
    }

    pub(super) fn current_field_matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.session.as_ref().is_some_and(|s| {
            s.fields
                .get(s.field_index)
                .is_some_and(|f| field_matches(f, &needle))
        })
    }
}

/// Whether a metadata field's label or key contains `needle` (already
/// lowercased); matched against both so `/album` finds "Album Artist" and
/// `/album_artist` finds it by its raw tag key too.
fn field_matches(field: &super::MetaField, needle: &str) -> bool {
    field.label.to_lowercase().contains(needle) || field.key.contains(needle)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, press, press_ctrl, type_text};
    use crate::app::{Mode, Overlay};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn ctrl_j_k_cycle_songs_and_stay_in_metadata_mode() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // PLAY
        press(&mut app, KeyCode::Char('m')); // METADATA
        assert_eq!(app.mode, Mode::Metadata);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            app.session.as_ref().unwrap().field_index,
            1,
            "plain j still moves between fields"
        );

        press_ctrl(&mut app, KeyCode::Char('j'));
        assert_eq!(
            app.mode,
            Mode::Metadata,
            "should stay in METADATA after cycling"
        );
        assert_eq!(app.session.as_ref().unwrap().index, 1);

        press_ctrl(&mut app, KeyCode::Up);
        assert_eq!(app.mode, Mode::Metadata);
        assert_eq!(app.session.as_ref().unwrap().index, 0);
    }

    #[test]
    fn all_tags_are_shown_after_the_preconfigured_set_sorted_alphabetically() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.files[0]
            .tags
            .insert("encoder".to_string(), "libopus".to_string());
        app.files[0].tags.insert("bpm".to_string(), "120".to_string());
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));

        let session = app.session.as_ref().unwrap();
        assert_eq!(
            session.fields.len(),
            crate::media::probe::METADATA_FIELDS.len() + 2
        );
        let base = crate::media::probe::METADATA_FIELDS.len();
        assert_eq!(session.fields[base].key, "bpm");
        assert_eq!(session.fields[base + 1].key, "encoder");
        assert_eq!(session.fields[base + 1].label, "Encoder");
    }

    #[test]
    fn slash_search_jumps_to_a_matching_field_by_label() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "comment");
        press(&mut app, KeyCode::Enter);

        let session = app.session.as_ref().unwrap();
        assert_eq!(session.fields[session.field_index].label, "Comment");
        assert!(!app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn n_repeats_a_field_search_and_wraps() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "date");
        press(&mut app, KeyCode::Enter);
        let first_hit = app.session.as_ref().unwrap().field_index;

        press(&mut app, KeyCode::Char('n'));
        assert_eq!(
            app.session.as_ref().unwrap().field_index,
            first_hit,
            "the only match wraps back to itself"
        );
    }

    #[test]
    fn escaping_a_field_search_restores_the_pre_search_field() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Char('j')); // field_index = 1

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "comment");
        assert_ne!(app.session.as_ref().unwrap().field_index, 1, "previewed the match");

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.session.as_ref().unwrap().field_index, 1);
        assert!(app.prompt.is_none());
    }
}
