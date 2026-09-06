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
    /// The marker currently hugging the cursor — moving the cursor
    /// (Left/Right, `c`, or a typed `b`/`e` jump) always drags this one
    /// along. There's no "neither" state; Tab switches which one it is.
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

        let fields = build_fields(&info);

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
        // The cursor (playback position) doesn't move on its own here, but
        // the active marker still hugs it — left stale, the next Left/Right
        // would yank the freshly detected marker straight back to wherever
        // playback happened to be. Reset it to match instead.
        let target = self.marker(self.active).seconds();
        self.player.seek_to(target);
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

    /// Move the cursor (Left/Right in EDIT) — the playback position itself,
    /// there is no separate cursor value — and keep the active marker
    /// snapped to it. See [`Session::drag_active_marker`].
    pub(super) fn move_cursor(&mut self, delta: f64) {
        self.player.seek_by(delta);
        self.drag_active_marker();
    }

    /// Keep `active` snapped to the cursor (possibly past the *other*
    /// marker — see [`Session::settle_crossed_markers`]). Called after any
    /// cursor move: Left/Right, a typed `c` jump, or a typed `b`/`e` jump
    /// (which also sets `active` first).
    pub(super) fn drag_active_marker(&mut self) {
        let active = self.active;
        self.snap_marker_to_cursor(active);
    }

    /// Snap `kind`'s marker to the cursor (the playback position), even if
    /// that pushes it past the *other* marker. Crossing is allowed to
    /// stand transiently — while the user is still moving, Begin can
    /// briefly read past End — and only gets resolved by
    /// [`Session::settle_crossed_markers`], not here. That keeps every
    /// keystroke cheap and avoids a role-swap mid-drag that would make the
    /// marker you're moving suddenly not be the one under your cursor.
    fn snap_marker_to_cursor(&mut self, kind: MarkerKind) {
        let duration = self.duration();
        let cursor = self.player.position();
        match kind {
            MarkerKind::Begin => self.begin = Marker::absolute(cursor, duration),
            MarkerKind::End => self.end = Marker::absolute(cursor, duration),
        }
        self.markers_dirty = true;
    }

    /// True while Begin/End are inverted from an in-progress cursor drag —
    /// a transient state [`Session::settle_crossed_markers`] resolves once
    /// the user stops moving, switches marker focus, or saves.
    pub fn is_crossed(&self) -> bool {
        self.begin.seconds() > self.end.seconds()
    }

    /// Resolve a crossed Begin/End by swapping which marker holds which
    /// value — preserving the trimmed range's width instead of collapsing
    /// it to a point — and flipping `active` to match, so it keeps naming
    /// whichever marker now holds the cursor's own time (the marker being
    /// dragged never appears to jump: it's still labeled differently, but
    /// still sits at the time you were just at). Returns whether a
    /// correction actually happened, so callers can tell the user.
    pub(super) fn settle_crossed_markers(&mut self) -> bool {
        if !self.is_crossed() {
            return false;
        }
        std::mem::swap(&mut self.begin, &mut self.end);
        self.active = self.active.toggled();
        true
    }
}

/// Every preconfigured field (in `METADATA_FIELDS` order), followed by
/// whatever other tags the file carries that aren't already covered —
/// alphabetical, since `MediaInfo::all_tags` hands them back in key order.
fn build_fields(info: &MediaInfo) -> Vec<MetaField> {
    let mut fields: Vec<MetaField> = METADATA_FIELDS
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

    for (key, value) in info.all_tags() {
        if METADATA_FIELDS.iter().any(|(k, _)| *k == key) {
            continue;
        }
        fields.push(MetaField {
            label: humanize(&key),
            key,
            original: Some(value.clone()),
            value: Some(value),
        });
    }
    fields
}

/// `"album_artist"` -> `"Album Artist"`, for the label of a tag that isn't
/// in the preconfigured list and so has no curated label of its own.
fn humanize(key: &str) -> String {
    key.split(['_', '-'])
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
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

        // `e` does the same for End.
        press(&mut app, KeyCode::Char('e'));
        type_text(&mut app, "300");
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
    fn crossing_the_other_marker_is_transient_until_settled() {
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
}
