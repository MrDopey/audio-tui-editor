//! Application state, modes and key handling (design §4–§6, §8–§10, §18–§20).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;

use crate::batch::{self, BatchItem, BatchReport, Progress, RunMode};
use crate::config::Config;
use crate::media::autotrim::{self, TrimSuggestion};
use crate::media::ffmpeg::{self, SaveOutcome, SaveRequest};
use crate::media::probe::{self, MediaInfo, METADATA_FIELDS};
use crate::media::waveform::{self, Waveform};
use crate::player::{AudioOutput, AudioPlayer};
use crate::timespec::{format_timestamp, Marker};

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
    Running(Receiver<Result<T, String>>),
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
                *self = Analysis::Failed(err);
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
}

impl Session {
    fn new(index: usize, info: MediaInfo, output: &AudioOutput, volume: f64) -> Session {
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
    pub fn metadata_edits(&self) -> BTreeMap<String, Option<String>> {
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
            let result = waveform::analyse(&path, duration).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.waveform = Analysis::Running(rx);
    }

    /// Kick off automatic marker detection, at most once per session.
    fn start_auto_markers(&mut self, config: &Config) {
        if self.auto_requested {
            return;
        }
        self.auto_requested = true;
        let (tx, rx) = channel();
        let path = self.info.path.clone();
        let duration = self.info.duration;
        let auto_config = config.auto_trim.clone();
        std::thread::spawn(move || {
            let result =
                autotrim::detect(&path, duration, &auto_config).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.auto = Analysis::Running(rx);
    }

    /// Adopt a detected suggestion, unless the user has already moved markers.
    fn adopt_suggestion(&mut self, suggestion: TrimSuggestion) {
        if self.markers_dirty {
            return;
        }
        let duration = self.duration();
        self.begin = Marker::absolute(suggestion.begin, duration);
        self.end = Marker::absolute(suggestion.end, duration);
    }

    fn marker(&self, kind: MarkerKind) -> &Marker {
        match kind {
            MarkerKind::Begin => &self.begin,
            MarkerKind::End => &self.end,
        }
    }

    /// Set a marker, keeping `begin < end` and clamping to the file.
    fn set_marker(&mut self, kind: MarkerKind, marker: Marker) {
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

    fn nudge(&mut self, kind: MarkerKind, delta: f64) {
        let duration = self.duration();
        let moved = self.marker(kind).nudged(delta, duration);
        self.set_marker(kind, moved);
    }
}

/// A navigation the user must confirm because edits would be discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingNav {
    Quit,
    CloseFile,
    Open(usize),
}

/// Live state of a folder-wide run.
pub struct BatchView {
    pub mode: RunMode,
    pub total: usize,
    pub items: Vec<BatchItem>,
    pub report: Option<BatchReport>,
    pub scroll: usize,
    rx: Option<Receiver<Progress>>,
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

pub struct App {
    pub config: Config,
    pub folder: PathBuf,
    pub files: Vec<MediaInfo>,
    pub selected: usize,
    pub mode: Mode,
    pub overlay: Overlay,
    pub prompt: Option<Prompt>,
    pub session: Option<Session>,
    pub status: Option<StatusMessage>,
    pub last_search: String,
    pub should_quit: bool,
    /// Rows the file list can show, updated by the renderer for paging.
    pub page_rows: usize,
    /// Scroll position of the file list, owned by the renderer.
    pub list_state: ListState,
    /// Scroll offset within the active overlay.
    pub overlay_scroll: u16,
    /// Total and visible rows of the active overlay, measured by the renderer.
    pub overlay_lines: usize,
    pub overlay_view_rows: usize,
    output: AudioOutput,
    /// Volume carried across files so it feels like one application.
    volume: f64,
    pending_g: bool,
    save_rx: Option<Receiver<Result<SaveOutcome, String>>>,
    /// Set when a save should be followed by leaving the file.
    save_then_close: bool,
}

impl App {
    pub fn new(folder: PathBuf, files: Vec<MediaInfo>, config: Config, output: AudioOutput) -> App {
        App {
            config,
            folder,
            files,
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
            save_rx: None,
            save_then_close: false,
        }
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
        changed
    }

    fn poll_save(&mut self) -> bool {
        let Some(rx) = &self.save_rx else {
            return false;
        };
        let received = match rx.try_recv() {
            Ok(result) => Some(result),
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("the save worker stopped unexpectedly".to_string()))
            }
        };
        self.save_rx = None;

        match received.expect("a result or a disconnect") {
            Ok(outcome) => {
                let lines = outcome.summary_lines();
                self.refresh_after_save();
                self.overlay = Overlay::Summary(lines);
                if self.save_then_close {
                    self.save_then_close = false;
                    self.close_file();
                }
            }
            Err(err) => {
                self.save_then_close = false;
                self.fail(
                    "Could not save the file.\n\nThe original file has NOT been modified.",
                    err,
                );
            }
        }
        true
    }

    fn poll_batch(&mut self) -> bool {
        let mut changed = false;
        let mut finished_run: Option<RunMode> = None;

        if let Overlay::Batch(view) = &mut self.overlay {
            if view.rx.is_some() {
                loop {
                    let message = match view.rx.as_ref().expect("rx is present").try_recv() {
                        Ok(message) => message,
                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            view.rx = None;
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
                            view.rx = None;
                            finished_run = Some(view.mode);
                            break;
                        }
                    }
                }
            }
        }

        // Files on disk changed, so durations and tags must be re-read.
        if finished_run.is_some_and(|mode| !mode.is_dry_run()) {
            self.rescan_folder();
        }
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
                running: view.rx.is_some(),
                count: view.items.len(),
            },
        };

        match kind {
            Kind::None | Kind::Working => {}
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
                KeyCode::Enter => self.start_batch(RunMode::Apply),
                KeyCode::Char('d') => self.start_batch(RunMode::DryRun),
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
                    // Saving first, then continuing where the user was headed.
                    self.save_then_close = nav != PendingNav::Quit;
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

    fn on_browse_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half_page = (self.page_rows / 2).max(1) as isize;
        let page = self.page_rows.max(1) as isize;
        let pending_g = std::mem::take(&mut self.pending_g);

        match key.code {
            KeyCode::Char('g') if pending_g => self.selected = 0,
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.selected = self.files.len().saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('d') if ctrl => self.move_selection(half_page),
            KeyCode::Char('u') if ctrl => self.move_selection(-half_page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Char('/') => {
                self.prompt = Some(Prompt::new(PromptKind::Search, String::new()))
            }
            KeyCode::Char('n') => self.repeat_search(true),
            KeyCode::Char('N') => self.repeat_search(false),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            KeyCode::Char('r') => self.rescan_folder(),
            KeyCode::Char('q') => self.request_nav(PendingNav::Quit),
            _ => {}
        }
    }

    fn on_play_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let small = self.config.playback.small_seek_seconds;
        let large = self.config.playback.large_seek_seconds;
        let step = self.config.playback.volume_step;

        match key.code {
            KeyCode::Esc => self.request_nav(PendingNav::CloseFile),
            KeyCode::Char(' ') => self.with_player(AudioPlayer::toggle),
            KeyCode::Left | KeyCode::Char('h') => {
                let delta = if ctrl { -large } else { -small };
                self.with_player(|p| p.seek_by(delta));
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let delta = if ctrl { large } else { small };
                self.with_player(|p| p.seek_by(delta));
            }
            KeyCode::Up | KeyCode::Char('k') => self.change_volume(step),
            KeyCode::Down | KeyCode::Char('j') => self.change_volume(-step),
            KeyCode::Char('g') => self.with_player(|p| p.seek_to(0.0)),
            KeyCode::Char('G') => self.with_player(|p| {
                let end = p.duration();
                p.seek_to((end - 5.0).max(0.0));
            }),
            KeyCode::Char('e') => self.enter_edit_mode(),
            KeyCode::Char('m') => self.mode = Mode::Metadata,
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }

    fn on_edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let fine = self.config.editing.fine_step_seconds;
        let large = self.config.editing.large_step_seconds;

        let Some(active) = self.session.as_ref().map(|s| s.active) else {
            self.mode = Mode::Browse;
            return;
        };

        match key.code {
            KeyCode::Esc => self.mode = Mode::Play,
            KeyCode::Left | KeyCode::Char('h') => {
                self.nudge_marker(active, if ctrl { -large } else { -fine })
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.nudge_marker(active, if ctrl { large } else { fine })
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(session) = &mut self.session {
                    session.active = session.active.toggled();
                }
            }
            KeyCode::Char('b') => self.set_marker_at_playhead(MarkerKind::Begin),
            KeyCode::Char('e') => self.set_marker_at_playhead(MarkerKind::End),
            KeyCode::Char('B') => self.prompt_for_marker(MarkerKind::Begin),
            KeyCode::Char('E') => self.prompt_for_marker(MarkerKind::End),
            KeyCode::Char('i') => self.prompt_for_marker(active),
            KeyCode::Char(' ') => self.with_player(AudioPlayer::toggle),
            KeyCode::Char('p') => {
                let target = self.session.as_ref().map(|s| s.marker(active).seconds());
                if let Some(target) = target {
                    self.with_player(|p| {
                        p.seek_to(target);
                        p.play();
                    });
                }
            }
            KeyCode::Char('a') => self.recalculate_auto_markers(),
            KeyCode::Char('r') => self.reset_markers(),
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
    }

    fn on_metadata_key(&mut self, key: KeyEvent) {
        if self.session.is_none() {
            self.mode = Mode::Browse;
            return;
        }
        let last = METADATA_FIELDS.len().saturating_sub(1);

        match key.code {
            KeyCode::Esc => self.mode = Mode::Play,
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
            KeyCode::Char(':') => {
                self.prompt = Some(Prompt::new(PromptKind::Command, String::new()))
            }
            KeyCode::Char('?') => self.overlay = Overlay::Help,
            _ => {}
        }
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
            // Let the fresh suggestion win over anything set so far.
            session.markers_dirty = false;
            session.auto_requested = false;
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

    // ---- prompt submission ----------------------------------------------

    fn submit_prompt(&mut self, prompt: Prompt) {
        let input = prompt.buffer.trim().to_string();
        match prompt.kind {
            PromptKind::Command => self.run_command(&input),
            PromptKind::Search => {
                if input.is_empty() {
                    return;
                }
                self.last_search = input;
                self.repeat_search(true);
            }
            PromptKind::Marker(kind) => self.set_marker_from_expression(kind, &input),
            PromptKind::MetadataField(index) => {
                if let Some(session) = &mut self.session {
                    if let Some(field) = session.fields.get_mut(index) {
                        let trimmed = input.trim();
                        field.value = if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_string())
                        };
                    }
                }
            }
        }
    }

    fn set_marker_from_expression(&mut self, kind: MarkerKind, input: &str) {
        let Some(duration) = self.session.as_ref().map(Session::duration) else {
            self.warn("No file is open.");
            return;
        };
        match Marker::parse(input, duration) {
            Ok(marker) => {
                let shown = if let Some(session) = &mut self.session {
                    session.set_marker(kind, marker);
                    session.marker(kind).to_string()
                } else {
                    return;
                };
                self.info(format!("{} marker set to {shown}", kind.label()));
            }
            Err(err) => self.warn(format!("{err}. Try 10:00, +10s, -1m or 50%.")),
        }
    }

    // ---- commands (design §19) ------------------------------------------

    fn run_command(&mut self, input: &str) {
        let mut parts = input.split_whitespace();
        let Some(command) = parts.next() else {
            return;
        };
        let rest: Vec<&str> = parts.collect();
        let argument = rest.join(" ");

        match command {
            "w" | "write" => self.save_current(),
            "q" | "quit" => {
                let target = self.close_target();
                self.request_nav(target);
            }
            "q!" | "quit!" => {
                let target = self.close_target();
                self.perform_nav(target);
            }
            "wq" | "x" => {
                self.save_then_close = self.session.is_some();
                self.save_current();
            }
            "help" | "h" => self.overlay = Overlay::Help,
            "apply-defaults" => {
                if rest.iter().any(|a| *a == "--dry-run" || *a == "-n") {
                    self.start_batch(RunMode::DryRun);
                } else {
                    self.overlay = Overlay::ConfirmApply;
                }
            }
            "dry-run" => self.start_batch(RunMode::DryRun),
            "b" | "begin" => self.set_marker_from_expression(MarkerKind::Begin, &argument),
            "e" | "end" => self.set_marker_from_expression(MarkerKind::End, &argument),
            "auto" => self.recalculate_auto_markers(),
            "reset" => {
                if self.session.is_some() {
                    self.reset_markers();
                } else {
                    self.warn("No file is open.");
                }
            }
            other => self.warn(format!("Unknown command: :{other}. Try :help")),
        }
    }

    fn close_target(&self) -> PendingNav {
        if self.session.is_some() {
            PendingNav::CloseFile
        } else {
            PendingNav::Quit
        }
    }

    // ---- navigation ------------------------------------------------------

    fn move_selection(&mut self, delta: isize) {
        if self.files.is_empty() {
            return;
        }
        let last = self.files.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    fn repeat_search(&mut self, forward: bool) {
        if self.last_search.is_empty() {
            self.warn("No search pattern. Press / to search.");
            return;
        }
        if self.files.is_empty() {
            return;
        }
        let needle = self.last_search.to_lowercase();
        let count = self.files.len();
        // Wrap around, starting from the entry after (or before) the cursor.
        for offset in 1..=count {
            let index = if forward {
                (self.selected + offset) % count
            } else {
                (self.selected + count * count - offset) % count
            };
            if self.files[index]
                .file_name()
                .to_lowercase()
                .contains(&needle)
            {
                self.selected = index;
                let pattern = self.last_search.clone();
                self.info(format!("/{pattern}"));
                return;
            }
        }
        let pattern = self.last_search.clone();
        self.warn(format!("Pattern not found: {pattern}"));
    }

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

    fn open_selected(&mut self) {
        if self.files.is_empty() {
            self.warn("No audio files in this folder.");
            return;
        }
        let index = self.selected;
        if self.session.as_ref().is_some_and(Session::is_dirty) {
            self.overlay = Overlay::ConfirmDiscard(PendingNav::Open(index));
            return;
        }
        self.open_index(index);
    }

    fn open_index(&mut self, index: usize) {
        let Some(info) = self.files.get(index).cloned() else {
            return;
        };
        self.session = Some(Session::new(index, info, &self.output, self.volume));
        self.mode = Mode::Play;
    }

    fn enter_edit_mode(&mut self) {
        let config = self.config.clone();
        if let Some(session) = &mut self.session {
            session.start_auto_markers(&config);
            self.mode = Mode::Edit;
        }
    }

    fn with_player(&mut self, f: impl FnOnce(&mut AudioPlayer)) {
        if let Some(session) = &mut self.session {
            f(&mut session.player);
        }
    }

    fn change_volume(&mut self, delta: f64) {
        let Some(session) = &mut self.session else {
            return;
        };
        session.player.adjust_volume(delta);
        self.volume = session.player.volume();
        let volume = self.volume;
        self.info(format!("Volume {volume:.0}%"));
    }

    // ---- saving ----------------------------------------------------------

    fn save_current(&mut self) {
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

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = ffmpeg::save(&info, &request).map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        self.save_rx = Some(rx);
        self.overlay = Overlay::Working(format!("Saving {name}…"));
    }

    /// Re-read the saved file so durations, tags and markers reflect disk.
    fn refresh_after_save(&mut self) {
        let Some((index, path)) = self
            .session
            .as_ref()
            .map(|s| (s.index, s.info.path.clone()))
        else {
            return;
        };
        let Ok(Some(info)) = probe::probe(&path) else {
            return;
        };
        if let Some(slot) = self.files.get_mut(index) {
            *slot = info.clone();
        }
        // Rebuild the session against the new file: the audio, waveform and
        // markers all describe the previous contents otherwise.
        self.session = Some(Session::new(index, info, &self.output, self.volume));
    }

    pub fn rescan_folder(&mut self) {
        let current = self.current().map(|f| f.path.clone());
        match probe::scan_folder(&self.folder) {
            Ok(files) => {
                self.files = files;
                if let Some(index) =
                    current.and_then(|path| self.files.iter().position(|f| f.path == path))
                {
                    self.selected = index;
                }
                self.selected = self.selected.min(self.files.len().saturating_sub(1));
            }
            Err(err) => self.warn(format!("Could not rescan folder: {err:#}")),
        }
    }

    // ---- folder-wide runs -------------------------------------------------

    fn start_batch(&mut self, mode: RunMode) {
        if self.files.is_empty() {
            self.warn("No audio files in this folder.");
            self.overlay = Overlay::None;
            return;
        }
        // Playback holds a decoder open on one of these files.
        self.with_player(AudioPlayer::pause);

        let (tx, rx) = channel();
        batch::spawn(self.files.clone(), self.config.clone(), mode, tx);
        self.overlay = Overlay::Batch(BatchView {
            mode,
            total: self.files.len(),
            items: Vec::new(),
            report: None,
            scroll: 0,
            rx: Some(rx),
        });
    }

    /// The confirmation text shown before a folder-wide run (design §17).
    pub fn apply_confirmation_lines(&self) -> Vec<String> {
        let auto = &self.config.auto_trim;
        vec![
            format!("Apply automatic trim to {} files?", self.files.len()),
            String::new(),
            "Threshold:".to_string(),
            format!("  begin {} dB", auto.begin_threshold_db),
            format!("  end   {} dB", auto.end_threshold_db),
            String::new(),
            "Minimum duration:".to_string(),
            format!("  begin {}s", auto.begin_min_duration),
            format!("  end   {}s", auto.end_min_duration),
            String::new(),
            "Applying rewrites every file in place.".to_string(),
            "A dry run reports what would change without writing anything.".to_string(),
            String::new(),
            "[Enter] apply    [d] dry run    [Esc] cancel".to_string(),
        ]
    }

    /// One row per file for the browse list (design §5).
    pub fn file_rows(&self) -> Vec<(String, String, String)> {
        self.files
            .iter()
            .map(|info| {
                (
                    info.file_name(),
                    format_timestamp(info.duration),
                    info.audio_codec.clone(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, duration: f64) -> MediaInfo {
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
    fn app(names: &[(&str, f64)]) -> App {
        let files = names.iter().map(|(n, d)| info(n, *d)).collect();
        App::new(
            PathBuf::from("/rec"),
            files,
            Config::default(),
            AudioOutput::silent(),
        )
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn press_ctrl(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::CONTROL));
    }

    fn type_text(app: &mut App, text: &str) {
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
    fn browse_navigation_follows_vim_motions() {
        let mut app = app(&[
            ("a.opus", 1.0),
            ("b.opus", 2.0),
            ("c.opus", 3.0),
            ("d.opus", 4.0),
        ]);
        app.overlay = Overlay::None;
        app.page_rows = 4;

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.selected, 3);
        press(&mut app, KeyCode::Char('g'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.selected, 0);
        press_ctrl(&mut app, KeyCode::Char('d'));
        assert_eq!(app.selected, 2);
        press_ctrl(&mut app, KeyCode::Char('u'));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn navigation_is_clamped_to_the_list() {
        let mut app = app(&[("a.opus", 1.0), ("b.opus", 2.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.selected, 0);
        press(&mut app, KeyCode::Char('G'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn search_finds_wraps_and_reports_misses() {
        let mut app = app(&[("alpha.opus", 1.0), ("beta.opus", 2.0), ("gamma.opus", 3.0)]);
        app.overlay = Overlay::None;

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "gam");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 2);

        // `n` wraps around to the only match again.
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.selected, 2);

        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "nothing");
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_ref().unwrap().is_error);
    }

    #[test]
    fn search_is_case_insensitive_and_n_capital_goes_backwards() {
        let mut app = app(&[("One.opus", 1.0), ("two.opus", 2.0), ("three.opus", 3.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "ONE");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.selected, 0);

        app.last_search = "o".to_string();
        app.selected = 2;
        press(&mut app, KeyCode::Char('N'));
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn prompt_editing_supports_backspace_and_cursor_movement() {
        let mut prompt = Prompt::new(PromptKind::Command, String::new());
        for c in "wq".chars() {
            prompt.insert(c);
        }
        assert_eq!(prompt.buffer, "wq");
        prompt.backspace();
        assert_eq!(prompt.buffer, "w");
        prompt.cursor = 0;
        prompt.insert('x');
        assert_eq!(prompt.buffer, "xw");
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
        press_ctrl(&mut app, KeyCode::Char('l'));
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 11.0);
        press_ctrl(&mut app, KeyCode::Char('h'));
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
    fn relative_expressions_set_markers_and_keep_their_text() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));

        app.run_command("b +10s");
        app.run_command("e -10s");
        let session = app.session.as_ref().unwrap();
        assert_eq!(session.begin.seconds(), 10.0);
        assert_eq!(session.end.seconds(), 590.0);
        assert_eq!(session.begin.text(), "+10s");
        assert_eq!(session.end.to_string(), "-10s (09:50)");
    }

    #[test]
    fn a_bad_marker_expression_is_reported_not_applied() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        app.run_command("b banana");
        assert!(app.status.as_ref().unwrap().is_error);
        assert_eq!(app.session.as_ref().unwrap().begin.seconds(), 0.0);
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
    fn unsaved_marker_changes_are_not_silently_discarded() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Esc); // back to PLAY
        press(&mut app, KeyCode::Esc); // would close the file

        assert!(matches!(
            app.overlay,
            Overlay::ConfirmDiscard(PendingNav::CloseFile)
        ));
        assert!(
            app.session.is_some(),
            "the file must stay open until confirmed"
        );

        press(&mut app, KeyCode::Esc); // cancel
        assert!(app.session.is_some());

        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Enter); // confirm discard
        assert!(app.session.is_none());
    }

    #[test]
    fn quitting_with_unsaved_changes_asks_first() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));
        app.run_command("q");
        assert!(matches!(app.overlay, Overlay::ConfirmDiscard(_)));
        assert!(!app.should_quit);
    }

    #[test]
    fn quit_bang_discards_without_asking() {
        let mut app = app(&[("a.opus", 600.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l'));
        app.run_command("q!");
        assert!(app.session.is_none());
    }

    #[test]
    fn browse_q_quits_when_nothing_is_open() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
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
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
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
    fn unknown_commands_are_reported() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.run_command("nonsense");
        let status = app.status.as_ref().unwrap();
        assert!(status.is_error);
        assert!(status.text.contains("Unknown command"));
    }

    #[test]
    fn help_is_reachable_from_the_command_line() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        app.run_command("help");
        assert!(matches!(app.overlay, Overlay::Help));
    }

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
    fn file_rows_show_name_duration_and_format() {
        let app = app(&[("interview-001.opus", 6151.0)]);
        let rows = app.file_rows();
        assert_eq!(rows[0].0, "interview-001.opus");
        assert_eq!(rows[0].1, "01:42:31");
        assert_eq!(rows[0].2, "opus");
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
    fn opening_a_file_with_no_files_present_warns() {
        let mut app = app(&[]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(app.session.is_none());
    }
}
