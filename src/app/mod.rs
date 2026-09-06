//! Application state, modes and key handling (design §4–§6, §8–§10, §18–§20).
//!
//! Split by concern: [`browse`]/[`play`]/[`edit`]/[`metadata`] hold each
//! mode's key handling, [`session`] owns the file currently open,
//! [`command`] parses `:` commands, [`save`]/[`batch_view`] own the two
//! background pipelines — a single save, and a folder-wide run —
//! [`overlay`] owns the modal popups, [`prompt`] owns the bottom-line text
//! prompt, and [`nav`] owns moving between files and markers.

mod analysis;
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
mod search;
mod session;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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

pub use analysis::Analysis;

#[derive(Debug, Clone, PartialEq)]
pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
}

/// `(name, duration, format)`, as shown in the browse list (design §5).
pub type FileRow = (String, String, String);

/// How long the cursor must sit still before a crossed Begin/End (see
/// `Session::drag_active_marker`) auto-corrects on its own, in `App::tick`.
const CURSOR_SETTLE_DELAY: Duration = Duration::from_secs(3);

/// Shown by any command that needs an open file when there isn't one.
pub(super) const NO_FILE_OPEN: &str = "No file is open. Open one with Enter.";

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
    /// The selection before the current `/` search started, so live search
    /// has somewhere to search forward from and `Esc` has somewhere to
    /// restore to. `None` when no Search prompt is open.
    search_origin: Option<usize>,
    /// Last time the cursor moved (Left/Right or `b`/`e`) in EDIT mode. A
    /// crossed Begin/End (see `Session::drag_active_marker`) is only auto-corrected
    /// once this has been idle for [`CURSOR_SETTLE_DELAY`] — i.e. once the
    /// user has actually stopped moving, not on every keystroke.
    last_cursor_move: Option<Instant>,
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
    /// Set by a Ctrl-C press; a second, consecutive Ctrl-C force-quits.
    /// Cleared by any other key, so it never lingers across unrelated input.
    ctrl_c_armed: bool,
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
            search_origin: None,
            last_cursor_move: None,
            should_quit: false,
            page_rows: 10,
            list_state: ListState::default(),
            overlay_scroll: 0,
            overlay_lines: 0,
            overlay_view_rows: 0,
            output,
            volume: 25.0,
            pending_g: false,
            ctrl_c_armed: false,
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
        let mut settled = false;

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
            // A cursor drag can leave Begin/End transiently crossed (see
            // `Session::drag_active_marker`); only auto-correct once the user has
            // actually stopped moving, not on every keystroke.
            let idle = self
                .last_cursor_move
                .is_none_or(|t| t.elapsed() >= CURSOR_SETTLE_DELAY);
            if idle && session.settle_crossed_markers() {
                changed = true;
                settled = true;
            }
        }

        if settled {
            self.info("Begin/End swapped.");
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

        // Ctrl-C is a universal escape hatch, ahead of overlays and prompts:
        // the first press arms it and warns, a second, consecutive press
        // force-quits (bypassing the unsaved-changes prompt, like `:q!`).
        // Any other key disarms it, so it never fires from unrelated presses
        // made much later.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.ctrl_c_armed {
                self.should_quit = true;
            } else {
                self.ctrl_c_armed = true;
                self.warn("Press Ctrl-C again to quit.");
            }
            return;
        }
        self.ctrl_c_armed = false;

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
mod tests;
