//! Folder-wide runs: the confirmation prompt and live progress (design §17).

use std::sync::mpsc::{channel, Receiver};

use super::App;
use crate::batch::{self, BatchItem, BatchReport, Progress, RunMode};
use crate::player::AudioPlayer;

/// Live state of a folder-wide run.
pub struct BatchView {
    pub mode: RunMode,
    pub total: usize,
    pub items: Vec<BatchItem>,
    pub report: Option<BatchReport>,
    pub scroll: usize,
    rx: Option<Receiver<Progress>>,
}

impl BatchView {
    pub(super) fn is_running(&self) -> bool {
        self.rx.is_some()
    }
}

impl App {
    pub(super) fn start_batch(&mut self, mode: RunMode) {
        if self.files.is_empty() && self.skipped.is_empty() {
            self.warn("No audio files in this folder.");
            self.overlay = super::Overlay::None;
            return;
        }
        // A real run rewrites files on disk, including the one open for
        // editing; refuse rather than silently discarding unsaved markers or
        // metadata (design: "unsaved changes must not be silently discarded").
        if !mode.is_dry_run() && self.session.as_ref().is_some_and(super::Session::is_dirty) {
            self.overlay = super::Overlay::None;
            self.warn("Save or discard changes to the open file before applying to the whole folder.");
            return;
        }
        // Playback holds a decoder open on one of these files.
        self.with_player(AudioPlayer::pause);

        let (tx, rx) = channel();
        batch::spawn(
            self.files.clone(),
            self.skipped.clone(),
            self.config.clone(),
            mode,
            tx,
        );
        self.overlay = super::Overlay::Batch(BatchView {
            mode,
            total: self.files.len() + self.skipped.len(),
            items: Vec::new(),
            report: None,
            scroll: 0,
            rx: Some(rx),
        });
    }

    pub(super) fn poll_batch(&mut self) -> bool {
        let mut changed = false;
        let mut finished_run: Option<RunMode> = None;

        if let super::Overlay::Batch(view) = &mut self.overlay {
            if let Some(rx) = view.rx.take() {
                let mut still_running = true;
                loop {
                    let message = match rx.try_recv() {
                        Ok(message) => message,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            still_running = false;
                            changed = true;
                            break;
                        }
                    };
                    changed = true;
                    match message {
                        Progress::Started { total, mode } => {
                            view.total = total;
                            view.mode = mode;
                        }
                        Progress::Item(item) => {
                            view.items.push(item);
                            // Keep the newest result in view.
                            view.scroll = view.items.len().saturating_sub(1);
                        }
                        Progress::Finished(report) => {
                            view.report = Some(report);
                            still_running = false;
                            finished_run = Some(view.mode);
                            break;
                        }
                    }
                }
                if still_running {
                    view.rx = Some(rx);
                }
            }
        }

        // Files on disk changed, so durations and tags must be re-read.
        if finished_run.is_some_and(|mode| !mode.is_dry_run()) {
            self.rescan_folder();
            // Any open session describes the file as it was before the
            // folder-wide rewrite; keeping it around risks a later `:w`
            // reapplying stale marker offsets to the new file.
            if self.session.is_some() {
                self.close_file();
                self.warn(
                    "The open file may have been rewritten by the batch run. \
                     Reopen it to continue editing.",
                );
            }
        }
        changed
    }

    /// Rescan the folder on a worker thread; scanning shells out to ffprobe
    /// once per file, which would otherwise stall the render loop.
    pub fn rescan_folder(&mut self) {
        if self.rescan_rx.is_some() {
            return;
        }
        let folder = self.folder.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::media::probe::scan_folder_detailed(&folder));
        });
        self.rescan_rx = Some(rx);
    }

    pub(super) fn poll_rescan(&mut self) -> bool {
        let Some(rx) = &self.rescan_rx else {
            return false;
        };
        let received = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(anyhow::anyhow!(
                "the folder scan worker stopped unexpectedly"
            )),
        };
        self.rescan_rx = None;
        match received {
            Ok(scan) => {
                let current = self.current().map(|f| f.path.clone());
                self.files = scan.files;
                self.skipped = scan.skipped;
                self.files_generation += 1;
                if let Some(index) =
                    current.and_then(|path| self.files.iter().position(|f| f.path == path))
                {
                    self.selected = index;
                }
                self.selected = self.selected.min(self.files.len().saturating_sub(1));
            }
            Err(err) => self.warn(format!("Could not rescan folder: {err:#}")),
        }
        true
    }

    /// The confirmation text shown before a folder-wide run (design §17).
    pub fn apply_confirmation_lines(&self) -> Vec<String> {
        let mut lines =
            batch::confirmation_lines(self.files.len(), self.skipped.len(), &self.config.auto_trim);
        lines.push(String::new());
        lines.push("[Enter] apply    [d] dry run    [Esc] cancel".to_string());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, press};
    use crate::app::Overlay;
    use crate::batch::{BatchReport, RunMode};
    use ratatui::crossterm::event::KeyCode;

    #[test]
    fn apply_defaults_asks_for_confirmation_first() {
        let mut app = app(&[("a.opus", 60.0), ("b.opus", 70.0)]);
        app.overlay = Overlay::None;
        app.run_command("apply-defaults");
        assert!(matches!(app.overlay, Overlay::ConfirmApply));

        let lines = app.apply_confirmation_lines();
        assert_eq!(lines[0], "Apply automatic trim to 2 files?");
        assert!(lines.iter().any(|l| l.contains("begin -40 dB")));
        assert!(lines.iter().any(|l| l.contains("[Esc] cancel")));
        assert!(lines.iter().any(|l| l.contains("[d] dry run")));

        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn apply_defaults_dry_run_skips_the_confirmation() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.run_command("apply-defaults --dry-run");
        match &app.overlay {
            Overlay::Batch(view) => assert_eq!(view.mode, RunMode::DryRun),
            _ => panic!("expected a batch view"),
        }
    }

    #[test]
    fn apply_defaults_is_blocked_while_the_open_file_has_unsaved_edits() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty marker edit
        press(&mut app, KeyCode::Esc); // back to PLAY; session stays open and dirty

        app.run_command("apply-defaults");
        assert!(matches!(app.overlay, Overlay::ConfirmApply));
        press(&mut app, KeyCode::Enter); // try to confirm the write
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(
            app.session.is_some(),
            "the dirty session must not be silently overwritten by a folder-wide run"
        );
    }

    #[test]
    fn apply_defaults_dry_run_is_allowed_even_with_unsaved_edits() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));

        app.run_command("apply-defaults --dry-run");
        match &app.overlay {
            Overlay::Batch(view) => assert_eq!(view.mode, RunMode::DryRun),
            _ => panic!("expected a batch view"),
        }
        assert!(app.session.is_some());
    }

    #[test]
    fn a_finished_apply_run_closes_the_open_session_to_avoid_stale_markers() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(app.session.is_some());

        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(crate::batch::Progress::Finished(BatchReport::new(
            RunMode::Apply,
        )))
        .unwrap();
        app.overlay = Overlay::Batch(super::BatchView {
            mode: RunMode::Apply,
            total: 1,
            items: Vec::new(),
            report: None,
            scroll: 0,
            rx: Some(rx),
        });

        app.poll_batch();
        assert!(
            app.session.is_none(),
            "a session left open across a real batch run could later save stale markers"
        );
    }
}
