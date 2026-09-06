//! Navigating between files and moving markers (design §5, §8–§11).
//!
//! Any navigation that would discard unsaved edits goes through
//! [`App::request_nav`], which defers to a discard-confirmation overlay
//! instead of acting immediately; [`App::perform_nav`] is what actually runs
//! once that's settled (or was never in question).

use std::time::Instant;

use super::{App, MarkerKind, Mode, Overlay, PendingNav, Prompt, PromptKind, Session};
use crate::player::AudioPlayer;
use crate::timespec::Marker;

impl App {
    pub(super) fn request_nav(&mut self, nav: PendingNav) {
        if self.session_is_dirty() {
            self.overlay = Overlay::ConfirmDiscard(nav);
        } else {
            self.perform_nav(nav);
        }
    }

    pub(super) fn perform_nav(&mut self, nav: PendingNav) {
        match nav {
            PendingNav::Quit => self.should_quit = true,
            PendingNav::CloseFile => self.close_file(),
            PendingNav::Open(index) => {
                self.selected = index;
                self.open_index(index);
            }
        }
    }

    pub(super) fn close_file(&mut self) {
        if let Some(session) = &mut self.session {
            session.player.pause();
            self.volume = session.player.volume();
        }
        self.session = None;
        self.mode = Mode::Browse;
    }

    pub(super) fn open_index(&mut self, index: usize) {
        let Some(info) = self.files.get(index).cloned() else {
            return;
        };
        self.session = Some(Session::new(index, info, &self.output, self.volume));
        self.mode = Mode::Play;
    }

    /// Cycle to the next (`delta = 1`) or previous (`delta = -1`) song in the
    /// current folder, wrapping around, and stay in the same mode (Edit or
    /// Metadata) so the user can keep tagging/marking their way through the
    /// folder. No-op with fewer than two files. If the outgoing session is
    /// dirty, `request_nav` defers to a discard-confirmation overlay instead
    /// of opening immediately; in that case the mode isn't preserved and the
    /// confirmed nav lands in Play mode as usual.
    pub(super) fn cycle_song(&mut self, delta: isize) {
        if self.files.len() < 2 {
            return;
        }
        let Some(current) = self.session.as_ref().map(|s| s.index) else {
            return;
        };
        let len = self.files.len() as isize;
        let next = (current as isize + delta).rem_euclid(len) as usize;
        let resume_mode = self.mode;
        let was_playing = self.session.as_ref().is_some_and(|s| s.player.is_playing());
        self.request_nav(PendingNav::Open(next));

        if self.session.as_ref().is_some_and(|s| s.index == next) {
            match resume_mode {
                // Same as entering EDIT directly (`e` in PLAY): stay on
                // whatever markers this file already has and wait for the
                // user to press `a` rather than guessing a trim eagerly.
                Mode::Edit => self.mode = Mode::Edit,
                Mode::Metadata => self.mode = Mode::Metadata,
                Mode::Browse | Mode::Play => {}
            }
            // Keep the music going across a song switch: a fresh session
            // otherwise always opens paused (see `Session::new`).
            if was_playing {
                self.with_player(AudioPlayer::play);
            }
        }
    }

    // ---- marker helpers --------------------------------------------------

    /// Move the cursor (Left/Right in EDIT) — the playback position. The
    /// active marker keeps following; a crossing with the other marker is
    /// allowed to stand until [`App::tick`] settles it once movement stops
    /// — see [`Session::drag_active_marker`].
    pub(super) fn move_cursor(&mut self, delta: f64) {
        self.last_cursor_move = Some(Instant::now());
        if let Some(session) = &mut self.session {
            session.move_cursor(delta);
        }
    }

    /// `b`/`e`/`i`: open the typed-jump prompt for a marker (`i` targets
    /// whichever is already active). Submitting moves the cursor there and
    /// makes that marker the active, hugging one — see
    /// `App::jump_marker_from_prompt` in `command.rs`.
    pub(super) fn prompt_for_marker(&mut self, kind: MarkerKind) {
        let current = self
            .session
            .as_ref()
            .map(|s| s.marker(kind).text().to_string())
            .unwrap_or_default();
        self.prompt = Some(Prompt::with_placeholder(PromptKind::Marker(kind), current));
    }

    pub(super) fn prompt_for_cursor(&mut self) {
        let current = self
            .session
            .as_ref()
            .map(|s| crate::timespec::format_timestamp(s.player.position()))
            .unwrap_or_default();
        self.prompt = Some(Prompt::with_placeholder(PromptKind::Cursor, current));
    }

    /// Seek playback to the active marker's position without playing
    /// (unlike `p`, which seeks there and plays) — useful to preview where
    /// a marker sits without engaging the cursor/drag machinery.
    pub(super) fn seek_to_active_marker(&mut self) {
        let target = self.session.as_ref().map(|s| s.marker(s.active).seconds());
        if let Some(target) = target {
            self.with_player(|p| p.seek_to(target));
        }
    }

    pub(super) fn recalculate_auto_markers(&mut self) {
        if self.session.is_none() {
            self.warn("No file is open.");
            return;
        }
        let config = self.config.clone();
        if let Some(session) = &mut self.session {
            // The fresh suggestion should win once it lands, even over
            // manually-edited markers — but until then, the markers on
            // screen are still whatever they were, so `dirty` must stay
            // truthful (clearing it early would let a navigation away
            // silently skip the discard-confirmation while a real, still
            // unsaved edit is on screen).
            session.request_recalculation();
            session.start_auto_markers(&config);
        }
        self.info("Recalculating automatic markers…");
    }

    pub(super) fn reset_markers(&mut self) {
        if let Some(session) = &mut self.session {
            let duration = session.duration();
            session.begin = Marker::absolute(0.0, duration);
            session.end = Marker::absolute(duration, duration);
            session.active = MarkerKind::Begin;
            session.markers_dirty = true;
        }
        self.info("Markers reset to the whole file.");
    }
}
