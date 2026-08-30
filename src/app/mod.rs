//! Application state, modes and key handling (design §4–§6, §8–§10, §18–§20).
//!
//! Split by concern: [`browse`]/[`play`]/[`edit`]/[`metadata`] hold each
//! mode's key handling, [`session`] owns the file currently open,
//! [`command`] parses `:` commands, and [`save`]/[`batch_view`] own the two
//! background pipelines — a single save, and a folder-wide run.

mod batch_view;
mod browse;
mod command;
mod edit;
mod metadata;
mod play;
mod save;
mod session;

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::config::Config;
use crate::media::probe::{self, MediaInfo, SkippedFile, METADATA_FIELDS};
use crate::player::AudioOutput;
use crate::timespec::Marker;

pub use batch_view::BatchView;
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

/// What a text prompt at the bottom of the screen is collecting.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    Command,
    Search,
    Marker(MarkerKind),
    MetadataField(usize),
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    pub cursor: usize,
}

impl Prompt {
    fn new(kind: PromptKind, initial: String) -> Self {
        Prompt {
            cursor: initial.chars().count(),
            kind,
            buffer: initial,
        }
    }

    /// The `:` / `/` sigil shown before the buffer.
    pub fn sigil(&self) -> &'static str {
        match self.kind {
            PromptKind::Command => ":",
            PromptKind::Search => "/",
            _ => "",
        }
    }

    pub fn label(&self) -> String {
        match &self.kind {
            PromptKind::Command | PromptKind::Search => String::new(),
            PromptKind::Marker(kind) => format!("{} marker: ", kind.label()),
            PromptKind::MetadataField(index) => {
                let label = METADATA_FIELDS
                    .get(*index)
                    .map(|(_, l)| *l)
                    .unwrap_or("Field");
                format!("{label}: ")
            }
        }
    }

    fn insert(&mut self, c: char) {
        let byte = self.byte_offset(self.cursor);
        self.buffer.insert(byte, c);
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_offset(self.cursor - 1);
        let end = self.byte_offset(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor -= 1;
    }

    fn byte_offset(&self, chars: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len())
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
        match rx.try_recv() {
            Ok(Ok(value)) => {
                *self = Analysis::Ready(value);
                true
            }
            Ok(Err(err)) => {
                // The channel carries the full `anyhow::Error` so its chain
                // survives the thread boundary; it is only rendered to text
                // once it settles here, at the point of actually being shown.
                *self = Analysis::Failed(format!("{err:#}"));
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                *self = Analysis::Failed("analysis worker stopped unexpectedly".to_string());
                true
            }
        }
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

/// A navigation the user must confirm because edits would be discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingNav {
    Quit,
    CloseFile,
    Open(usize),
}

pub enum Overlay {
    None,
    /// The in-place editing warning shown at startup (design §3).
    Warning,
    Help,
    /// A completed save summary (design §16).
    Summary(Vec<String>),
    Error {
        message: String,
        detail: String,
        showing_detail: bool,
    },
    /// Confirmation before a folder-wide run (design §17).
    ConfirmApply,
    ConfirmDiscard(PendingNav),
    Working(String),
    Batch(BatchView),
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

    /// Identifies the active overlay so a change of overlay can be detected.
    fn overlay_id(&self) -> u8 {
        match &self.overlay {
            Overlay::None => 0,
            Overlay::Warning => 1,
            Overlay::Help => 2,
            Overlay::Summary(_) => 3,
            Overlay::Error { .. } => 4,
            Overlay::ConfirmApply => 5,
            Overlay::ConfirmDiscard(_) => 6,
            Overlay::Working(_) => 7,
            Overlay::Batch(_) => 8,
        }
    }

    /// Scroll the active overlay, clamped to the content the renderer measured.
    fn scroll_overlay(&mut self, delta: isize) {
        let max = self.overlay_lines.saturating_sub(self.overlay_view_rows) as isize;
        let next = (self.overlay_scroll as isize + delta).clamp(0, max.max(0));
        self.overlay_scroll = next as u16;
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

        match self.mode {
            Mode::Browse => self.on_browse_key(key),
            Mode::Play => self.on_play_key(key),
            Mode::Edit => self.on_edit_key(key),
            Mode::Metadata => self.on_metadata_key(key),
        }
    }

    fn on_overlay_key(&mut self, key: KeyEvent) {
        /// What the active overlay needs to know to interpret a key, read out
        /// before anything mutates the overlay itself.
        enum Kind {
            None,
            Dismissable,
            Working,
            Error,
            ConfirmApply,
            ConfirmDiscard(PendingNav),
            Batch { running: bool, count: usize },
        }

        let kind = match &self.overlay {
            Overlay::None => Kind::None,
            Overlay::Warning | Overlay::Help | Overlay::Summary(_) => Kind::Dismissable,
            Overlay::Working(_) => Kind::Working,
            Overlay::Error { .. } => Kind::Error,
            Overlay::ConfirmApply => Kind::ConfirmApply,
            Overlay::ConfirmDiscard(nav) => Kind::ConfirmDiscard(*nav),
            Overlay::Batch(view) => Kind::Batch {
                running: view.is_running(),
                count: view.items.len(),
            },
        };

        match kind {
            Kind::None => {}
            Kind::Working => {
                if matches!(key.code, KeyCode::Esc) {
                    // The worker thread is left to finish (or fail) on its
                    // own; the temporary-file/atomic-replace pipeline still
                    // guarantees the original is untouched until it succeeds.
                    // Its result, if any still arrives, is simply ignored.
                    self.save_rx = None;
                    self.pending_nav_after_save = None;
                    self.overlay = Overlay::None;
                    self.warn(
                        "Stopped waiting. The save may still finish in the background; \
                         the original file is safe either way.",
                    );
                }
            }
            Kind::Dismissable => match key.code {
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
                _ => self.scroll_overlay_with(key),
            },
            Kind::Error => match key.code {
                KeyCode::Enter => {
                    if let Overlay::Error { showing_detail, .. } = &mut self.overlay {
                        *showing_detail = true;
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
                _ => self.scroll_overlay_with(key),
            },
            Kind::ConfirmApply => match key.code {
                KeyCode::Enter => self.start_batch(crate::batch::RunMode::Apply),
                KeyCode::Char('d') => self.start_batch(crate::batch::RunMode::DryRun),
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    self.info("Cancelled.");
                }
                _ => {}
            },
            Kind::ConfirmDiscard(nav) => match key.code {
                KeyCode::Enter => {
                    self.overlay = Overlay::None;
                    self.perform_nav(nav);
                }
                KeyCode::Char('w') => {
                    self.overlay = Overlay::None;
                    // Saving first, then continuing to the intended target —
                    // except Quit, which would otherwise end the program
                    // before the save summary ever gets drawn.
                    self.pending_nav_after_save = (nav != PendingNav::Quit).then_some(nav);
                    self.save_current();
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.overlay = Overlay::None;
                    self.info("Cancelled.");
                }
                _ => {}
            },
            Kind::Batch { running, count } => {
                let last = count.saturating_sub(1);
                if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter)
                    && !running
                {
                    self.overlay = Overlay::None;
                    return;
                }
                if let Overlay::Batch(view) = &mut self.overlay {
                    match key.code {
                        KeyCode::Char('j') | KeyCode::Down => {
                            view.scroll = (view.scroll + 1).min(last)
                        }
                        KeyCode::Char('k') | KeyCode::Up => {
                            view.scroll = view.scroll.saturating_sub(1)
                        }
                        KeyCode::Char('g') => view.scroll = 0,
                        KeyCode::Char('G') => view.scroll = last,
                        _ => {}
                    }
                }
            }
        }
    }

    /// Vim-ish scrolling shared by the read-only overlays.
    fn scroll_overlay_with(&mut self, key: KeyEvent) {
        let page = self.overlay_view_rows.max(1) as isize;
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.scroll_overlay(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_overlay(-1),
            KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_overlay(page),
            KeyCode::PageUp => self.scroll_overlay(-page),
            KeyCode::Char('g') => self.overlay_scroll = 0,
            KeyCode::Char('G') => self.scroll_overlay(self.overlay_lines as isize),
            _ => {}
        }
    }

    fn on_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                return;
            }
            KeyCode::Enter => {
                if let Some(prompt) = self.prompt.take() {
                    self.submit_prompt(prompt);
                }
                return;
            }
            _ => {}
        }

        let Some(prompt) = &mut self.prompt else {
            return;
        };
        match key.code {
            KeyCode::Backspace => prompt.backspace(),
            KeyCode::Left => prompt.cursor = prompt.cursor.saturating_sub(1),
            KeyCode::Right => {
                prompt.cursor = (prompt.cursor + 1).min(prompt.buffer.chars().count());
            }
            KeyCode::Home => prompt.cursor = 0,
            KeyCode::End => prompt.cursor = prompt.buffer.chars().count(),
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buffer.clear();
                prompt.cursor = 0;
            }
            KeyCode::Char(c) => prompt.insert(c),
            _ => {}
        }
    }

    // ---- navigation ------------------------------------------------------

    fn request_nav(&mut self, nav: PendingNav) {
        if self.session.as_ref().is_some_and(Session::is_dirty) {
            self.overlay = Overlay::ConfirmDiscard(nav);
        } else {
            self.perform_nav(nav);
        }
    }

    fn perform_nav(&mut self, nav: PendingNav) {
        match nav {
            PendingNav::Quit => self.should_quit = true,
            PendingNav::CloseFile => self.close_file(),
            PendingNav::Open(index) => {
                self.selected = index;
                self.open_index(index);
            }
        }
    }

    fn close_file(&mut self) {
        if let Some(session) = &mut self.session {
            session.player.pause();
            self.volume = session.player.volume();
        }
        self.session = None;
        self.mode = Mode::Browse;
    }

    fn open_index(&mut self, index: usize) {
        let Some(info) = self.files.get(index).cloned() else {
            return;
        };
        self.session = Some(Session::new(index, info, &self.output, self.volume));
        self.mode = Mode::Play;
    }

    // ---- marker helpers --------------------------------------------------

    fn nudge_marker(&mut self, kind: MarkerKind, delta: f64) {
        if let Some(session) = &mut self.session {
            session.nudge(kind, delta);
        }
    }

    fn set_marker_at_playhead(&mut self, kind: MarkerKind) {
        if let Some(session) = &mut self.session {
            let position = session.player.position();
            let duration = session.duration();
            session.active = kind;
            session.set_marker(kind, Marker::absolute(position, duration));
        }
    }

    fn prompt_for_marker(&mut self, kind: MarkerKind) {
        let current = self
            .session
            .as_ref()
            .map(|s| s.marker(kind).text().to_string())
            .unwrap_or_default();
        self.prompt = Some(Prompt::new(PromptKind::Marker(kind), current));
    }

    fn recalculate_auto_markers(&mut self) {
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

    fn reset_markers(&mut self) {
        if let Some(session) = &mut self.session {
            let duration = session.duration();
            session.begin = Marker::absolute(0.0, duration);
            session.end = Marker::absolute(duration, duration);
            session.markers_dirty = true;
        }
        self.info("Markers reset to the whole file.");
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
    use std::collections::BTreeMap;

    pub(super) fn info(name: &str, duration: f64) -> MediaInfo {
        MediaInfo {
            path: PathBuf::from(format!("/rec/{name}")),
            duration,
            format_name: "ogg".into(),
            audio_codec: "opus".into(),
            bit_rate: Some(64_000),
            sample_rate: Some(48_000),
            channels: Some(2),
            has_cover_art: false,
            chapter_count: 0,
            tags: BTreeMap::from([("title".to_string(), "Interview".to_string())]),
            stream_tags: BTreeMap::new(),
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
