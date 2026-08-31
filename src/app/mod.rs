//! Application state, modes and key handling (design §4–§6, §8–§10, §18–§20).
//!
//! Split by concern: [`browse`]/[`play`]/[`edit`]/[`metadata`] hold each
//! mode's key handling, [`session`] owns the file currently open,
//! [`command`] parses `:` commands, [`save`]/[`batch_view`] own the two
//! background pipelines — a single save, and a folder-wide run —
//! [`overlay`] owns the modal popups, [`prompt`] owns the bottom-line text
//! prompt, and [`nav`] owns moving between files and markers.

mod batch_view;
mod browse;
mod command;
mod edit;
mod metadata;
mod nav;
mod overlay;
mod play;
mod prompt;
mod save;
mod session;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::widgets::ListState;

use crate::config::Config;
use crate::media::probe::{self, MediaInfo, SkippedFile};
use crate::player::AudioOutput;

pub use batch_view::BatchView;
pub use overlay::{Overlay, PendingNav};
pub use prompt::{Prompt, PromptKind};
pub use save::MediaBackend;
pub use session::{MetaField, Session};

use save::FfmpegBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Play,
    Edit,
    Metadata,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Browse => "BROWSE",
            Mode::Play => "PLAY",
            Mode::Edit => "EDIT",
            Mode::Metadata => "METADATA",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerKind {
    Begin,
    End,
}

impl MarkerKind {
    pub fn label(self) -> &'static str {
        match self {
            MarkerKind::Begin => "beginning",
            MarkerKind::End => "ending",
        }
    }

    fn toggled(self) -> Self {
        match self {
            MarkerKind::Begin => MarkerKind::End,
            MarkerKind::End => MarkerKind::Begin,
        }
    }
}

/// Poll a worker's result channel: `Empty` means not ready yet, and
/// `Disconnected` (the thread panicked or was dropped without answering) is
/// synthesized into an error rather than silently forgotten. Shared by every
/// background pipeline in `app` that hands its result back over a channel.
pub(super) fn try_recv_result<T>(
    rx: &Receiver<anyhow::Result<T>>,
    what: &str,
) -> Option<anyhow::Result<T>> {
    match rx.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => {
            Some(Err(anyhow::anyhow!("{what} stopped unexpectedly")))
        }
    }
}

/// A background analysis whose result arrives on a channel.
pub enum Analysis<T> {
    Idle,
    Running(Receiver<anyhow::Result<T>>),
    Ready(T),
    Failed(String),
}

impl<T> Analysis<T> {
    /// Move to `Ready`/`Failed` if the worker has answered. Returns true on a
    /// state change, so the caller knows a redraw is warranted.
    fn poll(&mut self) -> bool {
        let Analysis::Running(rx) = self else {
            return false;
        };
        let Some(result) = try_recv_result(rx, "analysis worker") else {
            return false;
        };
        match result {
            Ok(value) => *self = Analysis::Ready(value),
            Err(err) => {
                // The channel carries the full `anyhow::Error` so its chain
                // survives the thread boundary; it is only rendered to text
                // once it settles here, at the point of actually being shown.
                *self = Analysis::Failed(format!("{err:#}"));
            }
        }
        true
    }

    pub fn ready(&self) -> Option<&T> {
        match self {
            Analysis::Ready(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Analysis::Running(_))
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Analysis::Failed(err) => Some(err),
            _ => None,
        }
    }
}

pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
}

/// `(name, duration, format)`, as shown in the browse list (design §5).
pub type FileRow = (String, String, String);

pub struct App {
    pub config: Config,
    pub folder: PathBuf,
    pub files: Vec<MediaInfo>,
    /// Candidates found while scanning the folder that could not be probed
    /// successfully, so folder-wide runs can still account for them.
    pub skipped: Vec<SkippedFile>,
    pub selected: usize,
    pub mode: Mode,
    pub overlay: Overlay,
    pub prompt: Option<Prompt>,
    pub session: Option<Session>,
    pub status: Option<StatusMessage>,
    pub last_search: String,
    pub should_quit: bool,
    // ---- renderer-owned scratch state -------------------------------
    // These five fields are measurements `ui.rs` takes of the terminal on
    // every frame (how many rows a list or popup has to work with) and
    // writes back here so key handling — `move_selection`, `scroll_overlay`
    // — can clamp against them. They are not part of the application model;
    // grouped here, not in their own struct, only because splitting them out
    // would mean threading a second `&mut` through every render function for
    // no behavioral change.
    /// Rows the file list can show, updated by the renderer for paging.
    pub page_rows: usize,
    /// Scroll position of the file list, owned by the renderer.
    pub list_state: ListState,
    /// Scroll offset within the active overlay.
    pub overlay_scroll: u16,
    /// Total and visible rows of the active overlay, measured by the renderer.
    pub overlay_lines: usize,
    pub overlay_view_rows: usize,
    // ---- end renderer-owned scratch state ----------------------------
    output: AudioOutput,
    /// Volume carried across files so it feels like one application.
    volume: f64,
    pending_g: bool,
    /// Bumped every time `files` is replaced or one of its entries changes,
    /// so [`App::file_rows`] can skip re-formatting on frames where nothing
    /// changed (the browse list is redrawn up to 20 times a second).
    files_generation: u64,
    file_rows_cache: Option<(u64, Vec<FileRow>)>,
    /// How a save is actually performed; a fake implementation is swapped in
    /// under test so the save/error-overlay wiring can be exercised without
    /// a real ffmpeg process.
    backend: Arc<dyn MediaBackend>,
    save_rx: Option<Receiver<anyhow::Result<crate::media::ffmpeg::SaveOutcome>>>,
    /// Set when a save should be followed by continuing to this navigation
    /// target, so "save, then go where I was headed" actually gets there.
    pending_nav_after_save: Option<PendingNav>,
    refresh_rx: Option<Receiver<anyhow::Result<(usize, MediaInfo)>>>,
    rescan_rx: Option<Receiver<anyhow::Result<probe::ScanResult>>>,
}

impl App {
    pub fn new(
        folder: PathBuf,
        files: Vec<MediaInfo>,
        skipped: Vec<SkippedFile>,
        config: Config,
        output: AudioOutput,
    ) -> App {
        App {
            config,
            folder,
            files,
            skipped,
            selected: 0,
            mode: Mode::Browse,
            overlay: Overlay::Warning,
            prompt: None,
            session: None,
            status: None,
            last_search: String::new(),
            should_quit: false,
            page_rows: 10,
            list_state: ListState::default(),
            overlay_scroll: 0,
            overlay_lines: 0,
            overlay_view_rows: 0,
            output,
            volume: 100.0,
            pending_g: false,
            files_generation: 0,
            file_rows_cache: None,
            backend: Arc::new(FfmpegBackend),
            save_rx: None,
            pending_nav_after_save: None,
            refresh_rx: None,
            rescan_rx: None,
        }
    }

    /// Swap in a fake save backend, so tests can exercise the save/error
    /// overlay wiring without shelling out to a real ffmpeg process.
    #[cfg(test)]
    pub(crate) fn set_backend(&mut self, backend: impl MediaBackend + 'static) {
        self.backend = Arc::new(backend);
    }

    pub fn audio_device_available(&self) -> bool {
        self.output.is_available()
    }

    pub fn audio_device_error(&self) -> Option<&str> {
        self.output.error()
    }

    pub fn current(&self) -> Option<&MediaInfo> {
        self.files.get(self.selected)
    }

    fn info(&mut self, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            text: text.into(),
            is_error: false,
        });
    }

    fn warn(&mut self, text: impl Into<String>) {
        self.status = Some(StatusMessage {
            text: text.into(),
            is_error: true,
        });
    }

    fn fail(&mut self, message: impl Into<String>, detail: impl Into<String>) {
        self.overlay = Overlay::Error {
            message: message.into(),
            detail: detail.into(),
            showing_detail: false,
        };
    }

    // ---- periodic work --------------------------------------------------

    /// Poll background workers. Returns true when something changed.
    pub fn tick(&mut self) -> bool {
        let mut changed = false;

        if let Some(session) = &mut self.session {
            changed |= session.waveform.poll();
            if session.auto.poll() {
                changed = true;
                // Copy the suggestion out before mutating the session.
                let suggestion = match &session.auto {
                    Analysis::Ready(suggestion) => Some(*suggestion),
                    _ => None,
                };
                if let Some(suggestion) = suggestion {
                    session.adopt_suggestion(suggestion);
                }
            }
            // The playback cursor moves on its own while playing.
            if session.player.is_playing() {
                changed = true;
                if session.player.at_end() {
                    session.player.pause();
                }
            }
        }

        changed |= self.poll_save();
        changed |= self.poll_batch();
        changed |= self.poll_refresh();
        changed |= self.poll_rescan();
        changed
    }

    fn is_busy(&self) -> bool {
        self.save_rx.is_some()
    }

    /// Whether the open file (if any) has edits that would be discarded by
    /// navigating away without saving.
    pub(super) fn session_is_dirty(&self) -> bool {
        self.session.as_ref().is_some_and(Session::is_dirty)
    }

    // ---- key handling ---------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let overlay_before = self.overlay_id();
        self.dispatch_key(key);
        // A different overlay starts at the top.
        if self.overlay_id() != overlay_before {
            self.overlay_scroll = 0;
            self.overlay_lines = 0;
            self.overlay_view_rows = 0;
        }
    }

    fn dispatch_key(&mut self, key: KeyEvent) {
        self.status = None;

        if !matches!(self.overlay, Overlay::None) {
            self.on_overlay_key(key);
            return;
        }
        if self.prompt.is_some() {
            self.on_prompt_key(key);
            return;
        }
        if self.is_busy() {
            return;
        }

        // `:` and `?` behave identically in every mode, so they are handled
        // once here rather than duplicated in each mode's key handler.
        match key.code {
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()));
                return;
            }
            KeyCode::Char('?') => {
                self.overlay = Overlay::Help;
                return;
            }
            _ => {}
        }

        match self.mode {
            Mode::Browse => self.on_browse_key(key),
            Mode::Play => self.on_play_key(key),
            Mode::Edit => self.on_edit_key(key),
            Mode::Metadata => self.on_metadata_key(key),
        }
    }

    /// One row per file for the browse list (design §5).
    pub fn file_rows(&mut self) -> &[FileRow] {
        let stale = self
            .file_rows_cache
            .as_ref()
            .is_none_or(|(generation, _)| *generation != self.files_generation);
        if stale {
            let rows = self
                .files
                .iter()
                .map(|info| {
                    (
                        info.file_name(),
                        crate::timespec::format_timestamp(info.duration),
                        info.audio_codec.clone(),
                    )
                })
                .collect();
            self.file_rows_cache = Some((self.files_generation, rows));
        }
        &self.file_rows_cache.as_ref().expect("just populated").1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use ratatui::crossterm::event::KeyModifiers;
    use std::collections::BTreeMap;

    pub(super) fn info(name: &str, duration: f64) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("/rec/{name}")),
            duration,
            tags: BTreeMap::from([("title".to_string(), "Interview".to_string())]),
            ..probe::fixture()
        }
    }

    /// An app with no audio device and no open file, for state-machine tests.
    pub(super) fn app(names: &[(&str, f64)]) -> App {
        let files = names.iter().map(|(n, d)| info(n, *d)).collect();
        App::new(
            PathBuf::from("/rec"),
            files,
            Vec::new(),
            Config::default(),
            AudioOutput::silent(),
        )
    }

    pub(super) fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub(super) fn press_ctrl(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::CONTROL));
    }

    pub(super) fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            press(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn starts_with_the_in_place_warning_then_browses() {
        let mut app = app(&[("a.opus", 60.0)]);
        assert!(matches!(app.overlay, Overlay::Warning));
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.overlay, Overlay::None));
        assert_eq!(app.mode, Mode::Browse);
    }

    #[test]
    fn mode_transitions_match_the_specification() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.mode, Mode::Play);

        press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.mode, Mode::Edit);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Play);

        press(&mut app, KeyCode::Char('m'));
        assert_eq!(app.mode, Mode::Metadata);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Play);

        press(&mut app, KeyCode::Esc);
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.session.is_none());
    }

    #[test]
    fn file_rows_show_name_duration_and_format() {
        let mut app = app(&[("interview-001.opus", 6151.0)]);
        let rows = app.file_rows();
        assert_eq!(rows[0].0, "interview-001.opus");
        assert_eq!(rows[0].1, "01:42:31");
        assert_eq!(rows[0].2, "opus");
    }

    #[test]
    fn file_rows_are_recomputed_after_a_rescan_replaces_the_file_list() {
        let mut app = app(&[("a.opus", 60.0)]);
        assert_eq!(app.file_rows()[0].0, "a.opus");

        app.files = vec![info("b.opus", 30.0)];
        app.files_generation += 1;
        assert_eq!(
            app.file_rows()[0].0,
            "b.opus",
            "a stale cache must not survive a file-list change"
        );
    }

    #[test]
    fn a_save_error_says_the_original_is_untouched() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.fail(
            "Could not save the file.\n\nThe original file has NOT been modified.",
            "ffmpeg: boom",
        );
        match &app.overlay {
            Overlay::Error {
                message,
                showing_detail,
                ..
            } => {
                assert!(message.contains("NOT been modified"));
                assert!(!showing_detail);
            }
            _ => panic!("expected an error overlay"),
        }
        press(&mut app, KeyCode::Enter);
        match &app.overlay {
            Overlay::Error {
                showing_detail,
                detail,
                ..
            } => {
                assert!(showing_detail);
                assert!(detail.contains("boom"));
            }
            _ => panic!("expected an error overlay"),
        }
    }

    #[test]
    fn opening_a_file_with_no_files_present_warns() {
        let mut app = app(&[]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(app.session.is_none());
    }
}
