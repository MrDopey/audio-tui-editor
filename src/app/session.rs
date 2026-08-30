//! The file currently open: playback, waveform, markers and metadata fields.

use std::sync::mpsc::channel;

use super::{Analysis, MarkerKind};
use crate::config::Config;
use crate::media::autotrim::{self, TrimSuggestion};
use crate::media::probe::{MediaInfo, METADATA_FIELDS};
use crate::media::waveform::{self, Waveform};
use crate::player::{AudioOutput, AudioPlayer};
use crate::timespec::Marker;

/// One editable metadata field (design §18).
#[derive(Debug, Clone)]
pub struct MetaField {
    pub key: String,
    pub label: String,
    pub original: Option<String>,
    pub value: Option<String>,
}

impl MetaField {
    pub fn is_changed(&self) -> bool {
        self.value != self.original
    }

    pub fn display(&self) -> &str {
        self.value.as_deref().unwrap_or("")
    }
}

/// Everything about the file currently open.
pub struct Session {
    pub index: usize,
    pub info: MediaInfo,
    pub player: AudioPlayer,
    pub waveform: Analysis<Waveform>,
    pub auto: Analysis<TrimSuggestion>,
    pub begin: Marker,
    pub end: Marker,
    pub active: MarkerKind,
    pub markers_dirty: bool,
    pub fields: Vec<MetaField>,
    pub field_index: usize,
    /// Whether automatic markers have been requested for this session.
    auto_requested: bool,
    /// Set by an explicit recalculation request: the next suggestion should
    /// replace the current markers even if they were manually edited.
    override_next_suggestion: bool,
}

impl Session {
    pub(super) fn new(index: usize, info: MediaInfo, output: &AudioOutput, volume: f64) -> Session {
        let duration = info.duration;
        let player = AudioPlayer::new(output, &info.path, duration, volume);

        let fields = METADATA_FIELDS
            .iter()
            .map(|(key, label)| {
                let value = info.tag(key).map(str::to_string);
                MetaField {
                    key: (*key).to_string(),
                    label: (*label).to_string(),
                    original: value.clone(),
                    value,
                }
            })
            .collect();

        let mut session = Session {
            index,
            info,
            player,
            waveform: Analysis::Idle,
            auto: Analysis::Idle,
            begin: Marker::absolute(0.0, duration),
            end: Marker::absolute(duration, duration),
            active: MarkerKind::Begin,
            markers_dirty: false,
            fields,
            field_index: 0,
            auto_requested: false,
            override_next_suggestion: false,
        };
        session.start_waveform();
        session
    }

    pub fn duration(&self) -> f64 {
        self.info.duration
    }

    pub fn metadata_dirty(&self) -> bool {
        self.fields.iter().any(MetaField::is_changed)
    }

    pub fn is_dirty(&self) -> bool {
        self.markers_dirty || self.metadata_dirty()
    }

    /// Metadata edits in the shape the save pipeline expects.
    pub fn metadata_edits(&self) -> std::collections::BTreeMap<String, Option<String>> {
        self.fields
            .iter()
            .filter(|f| f.is_changed())
            .map(|f| {
                let value = f
                    .value
                    .as_ref()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty());
                (f.key.clone(), value)
            })
            .collect()
    }

    fn start_waveform(&mut self) {
        let (tx, rx) = channel();
        let path = self.info.path.clone();
        let duration = self.info.duration;
        std::thread::spawn(move || {
            let _ = tx.send(waveform::analyse(&path, duration));
        });
        self.waveform = Analysis::Running(rx);
    }

    /// Kick off automatic marker detection, at most once per session.
    pub(super) fn start_auto_markers(&mut self, config: &Config) {
        if self.auto_requested {
            return;
        }
        self.auto_requested = true;
        let (tx, rx) = channel();
        let path = self.info.path.clone();
        let duration = self.info.duration;
        let auto_config = config.auto_trim.clone();
        std::thread::spawn(move || {
            let _ = tx.send(autotrim::detect(&path, duration, &auto_config));
        });
        self.auto = Analysis::Running(rx);
    }

    /// Ask the next detected suggestion to replace the current markers even
    /// if they have since been edited manually, and allow detection to run
    /// again. Until that suggestion actually lands, `markers_dirty` must stay
    /// true: the markers on screen are still whatever they were, so a
    /// navigation away must not silently skip the discard-confirmation while
    /// a real, still-unsaved edit is on screen.
    pub(super) fn request_recalculation(&mut self) {
        self.override_next_suggestion = true;
        self.auto_requested = false;
    }

    /// Adopt a detected suggestion, unless the user has already moved markers
    /// (an explicit recalculation request overrides that once).
    pub(super) fn adopt_suggestion(&mut self, suggestion: TrimSuggestion) {
        let forced = std::mem::take(&mut self.override_next_suggestion);
        if self.markers_dirty && !forced {
            return;
        }
        let duration = self.duration();
        self.begin = Marker::absolute(suggestion.begin, duration);
        self.end = Marker::absolute(suggestion.end, duration);
        self.markers_dirty = false;
    }

    pub(super) fn marker(&self, kind: MarkerKind) -> &Marker {
        match kind {
            MarkerKind::Begin => &self.begin,
            MarkerKind::End => &self.end,
        }
    }

    /// Set a marker, keeping `begin < end` and clamping to the file.
    pub(super) fn set_marker(&mut self, kind: MarkerKind, marker: Marker) {
        let duration = self.duration();
        match kind {
            MarkerKind::Begin => {
                let limit = (self.end.seconds() - 0.01).max(0.0);
                self.begin = if marker.seconds() > limit {
                    Marker::absolute(limit, duration)
                } else {
                    marker
                };
            }
            MarkerKind::End => {
                let limit = (self.begin.seconds() + 0.01).min(duration);
                self.end = if marker.seconds() < limit {
                    Marker::absolute(limit, duration)
                } else {
                    marker
                };
            }
        }
        self.markers_dirty = true;
    }

    pub(super) fn nudge(&mut self, kind: MarkerKind, delta: f64) {
        let duration = self.duration();
        let moved = self.marker(kind).nudged(delta, duration);
        self.set_marker(kind, moved);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, press};
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
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.session.as_ref().unwrap().end.seconds(), 599.0);
    }

    #[test]
    fn markers_cannot_cross_each_other() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.run_command("e 100");
        app.run_command("b 200");
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
}
