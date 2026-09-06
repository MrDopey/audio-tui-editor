//! The file currently open: playback, waveform, markers and metadata fields.

use std::sync::mpsc::channel;

use super::{Analysis, MarkerKind};
use crate::config::Config;
use crate::media::autotrim::{self, TrimSuggestion};
use crate::media::cover_art::{self, RawCoverArt};
use crate::media::probe::{MediaInfo, METADATA_FIELDS};
use crate::media::waveform::{self, Waveform};
use crate::player::{AudioOutput, AudioPlayer};
use crate::timespec::Marker;

/// Minimum gap kept between `begin` and `end` when clamping one against the
/// other, so they never land exactly on top of each other.
const MARKER_GAP_SECONDS: f64 = 0.01;

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
    /// The extracted cover-art image, if the file has one — `Ready(None)`
    /// (no background thread ever spawned) when it doesn't.
    pub cover_art: Analysis<Option<RawCoverArt>>,
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
            cover_art: Analysis::Idle,
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
        session.start_cover_art();
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

    /// Skips straight to `Ready(None)` (no thread spawned at all) when the
    /// file has no cover art, so `App::desired_cover_art` never waits on
    /// anything for the vastly more common case.
    fn start_cover_art(&mut self) {
        if !self.info.has_cover_art {
            self.cover_art = Analysis::Ready(None);
            return;
        }
        let (tx, rx) = channel();
        let path = self.info.path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Ok(cover_art::fetch(&path)));
        });
        self.cover_art = Analysis::Running(rx);
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
                let limit = (self.end.seconds() - MARKER_GAP_SECONDS).max(0.0);
                self.begin = if marker.seconds() > limit {
                    Marker::absolute(limit, duration)
                } else {
                    marker
                };
            }
            MarkerKind::End => {
                let limit = (self.begin.seconds() + MARKER_GAP_SECONDS).min(duration);
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
mod tests;
