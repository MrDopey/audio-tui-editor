//! The single-file save pipeline (design §13–§16).

use std::sync::mpsc::channel;

use anyhow::Context;

use super::{try_recv_result, App, Overlay, Session};
use crate::media::ffmpeg::{self, SaveOutcome, SaveRequest};
use crate::media::probe::{self, MediaInfo};

/// Abstracts how a save is actually performed, so the error-overlay wiring in
/// [`App::poll_save`] can be exercised end-to-end with a fake backend under
/// test, without shelling out to a real ffmpeg process.
pub trait MediaBackend: Send + Sync {
    fn save(&self, info: &MediaInfo, request: &SaveRequest) -> anyhow::Result<SaveOutcome>;
}

/// The real backend, calling into the ffmpeg-backed save pipeline.
pub(super) struct FfmpegBackend;

impl MediaBackend for FfmpegBackend {
    fn save(&self, info: &MediaInfo, request: &SaveRequest) -> anyhow::Result<SaveOutcome> {
        // Re-check the source immediately before saving, rather than trusting
        // whatever `MediaInfo` snapshot the session was opened with: if the
        // file changed on disk in the meantime, metadata preservation must be
        // judged against what is actually there now, not a stale cache.
        let fresh = probe::probe(&info.path)
            .with_context(|| format!("re-checking {} before saving", info.path.display()))?
            .ok_or_else(|| {
                anyhow::anyhow!("{} no longer has an audio stream", info.path.display())
            })?;
        ffmpeg::save(&fresh, request)
    }
}

impl App {
    // ---- saving -----------------------------------------------------------

    pub(super) fn save_current(&mut self) {
        if self.is_busy() {
            return;
        }
        let Some(session) = &self.session else {
            self.warn("No file is open. Open one with Enter.");
            return;
        };

        let request = SaveRequest {
            begin: session.begin.seconds(),
            end: session.end.seconds(),
            metadata: session.metadata_edits(),
        };
        let info = session.info.clone();
        let name = info.file_name();
        let backend = std::sync::Arc::clone(&self.backend);

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = backend.save(&info, &request);
            let _ = tx.send(result);
        });
        self.save_rx = Some(rx);
        self.overlay = Overlay::Working(format!("Saving {name}…"));
    }

    pub(super) fn poll_save(&mut self) -> bool {
        let Some(rx) = &self.save_rx else {
            return false;
        };
        let Some(received) = try_recv_result(rx, "the save worker") else {
            return false;
        };
        self.save_rx = None;

        match received {
            Ok(outcome) => {
                let lines = outcome.summary_lines();
                self.refresh_after_save();
                self.overlay = Overlay::Summary(lines);
                if let Some(nav) = self.pending_nav_after_save.take() {
                    self.perform_nav(nav);
                }
            }
            Err(err) => {
                self.pending_nav_after_save = None;
                self.fail(
                    "Could not save the file.\n\nThe original file has NOT been modified.",
                    format!("{err:#}"),
                );
            }
        }
        true
    }

    /// Re-read the saved file so durations, tags and markers reflect disk.
    ///
    /// Probing shells out to ffprobe, so this runs on a worker thread rather
    /// than blocking the render loop; see [`App::poll_refresh`].
    fn refresh_after_save(&mut self) {
        let Some((index, path)) = self
            .session
            .as_ref()
            .map(|s| (s.index, s.info.path.clone()))
        else {
            return;
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = probe::probe(&path).and_then(|info| {
                info.ok_or_else(|| anyhow::anyhow!("the file no longer has an audio stream"))
            });
            let _ = tx.send(result.map(|info| (index, info)));
        });
        self.refresh_rx = Some(rx);
    }

    pub(super) fn poll_refresh(&mut self) -> bool {
        let Some(rx) = &self.refresh_rx else {
            return false;
        };
        let Some(received) = try_recv_result(rx, "the file refresh worker") else {
            return false;
        };
        self.refresh_rx = None;
        match received {
            Ok((index, info)) => {
                if let Some(slot) = self.files.get_mut(index) {
                    *slot = info.clone();
                    self.files_generation += 1;
                }
                // Rebuild the session against the new file: the audio,
                // waveform and markers all describe the previous contents
                // otherwise. Skipped if the user has since closed or moved
                // on to a different file.
                if self.session.as_ref().is_some_and(|s| s.index == index) {
                    self.session = Some(Session::new(index, info, &self.output, self.volume));
                }
            }
            Err(err) => self.warn(format!("Could not refresh the saved file: {err:#}")),
        }
        true
    }
}

/// Test-only fixture, shared with other test modules that need a plausible
/// [`SaveOutcome`] without running a real save.
#[cfg(test)]
pub(super) fn fake_save_outcome(path: &str) -> SaveOutcome {
    SaveOutcome {
        path: std::path::PathBuf::from(path),
        noop: false,
        source_duration: 600.0,
        output_duration: 590.0,
        removed_beginning: 5.0,
        removed_ending: 5.0,
        processing: ffmpeg::Processing::StreamCopy,
        metadata: ffmpeg::MetadataReport::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app, press};
    use crate::app::PendingNav;
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn a_successful_save_updates_the_summary_and_refreshes_the_file() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);

        let (tx, rx) = std::sync::mpsc::channel();
        app.save_rx = Some(rx);
        tx.send(Ok(fake_save_outcome("/rec/a.opus"))).unwrap();
        app.poll_save();

        assert!(matches!(app.overlay, Overlay::Summary(_)));
    }

    #[test]
    fn esc_during_working_overlay_stops_waiting_without_losing_the_original() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        let (_tx, rx) = std::sync::mpsc::channel::<anyhow::Result<SaveOutcome>>();
        app.save_rx = Some(rx);
        app.overlay = Overlay::Working("Saving a.opus…".to_string());

        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.save_rx.is_none());
    }

    /// A fake backend that always fails, so the real error-overlay wiring
    /// (`save_current` → the worker thread → `poll_save`) can be exercised
    /// end to end instead of jumping straight to `App::fail` as a shortcut.
    struct AlwaysFails;

    impl MediaBackend for AlwaysFails {
        fn save(&self, _info: &MediaInfo, _request: &SaveRequest) -> anyhow::Result<SaveOutcome> {
            anyhow::bail!("ffmpeg: boom")
        }
    }

    #[test]
    fn a_failing_backend_produces_an_error_overlay_through_the_real_save_pipeline() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.set_backend(AlwaysFails);
        press(&mut app, KeyCode::Enter);

        app.save_current();
        assert!(
            app.save_rx.is_some(),
            "the worker thread must have been spawned"
        );

        // The worker thread runs concurrently; give it a moment to answer.
        let outcome = (0..200).find_map(|_| {
            if app.poll_save() {
                Some(())
            } else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                None
            }
        });
        assert!(outcome.is_some(), "poll_save never observed a result");

        match &app.overlay {
            Overlay::Error {
                message, detail, ..
            } => {
                assert!(message.contains("NOT been modified"));
                assert!(detail.contains("boom"));
            }
            _ => panic!("expected an error overlay after a failing save"),
        }
    }

    #[test]
    fn choosing_save_in_the_discard_dialog_sets_the_pending_target() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty

        app.selected = 1;
        app.open_selected();
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.pending_nav_after_save, Some(PendingNav::Open(1)));
        assert!(app.save_rx.is_some());
    }
}
