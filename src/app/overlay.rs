//! Modal overlays: the startup warning, help, save summary, errors,
//! confirmations and folder-run progress (design §3, §16, §17), plus the
//! scrolling shared by the read-only ones.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, BatchView};

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

impl App {
    /// Identifies the active overlay so a change of overlay can be detected.
    pub(super) fn overlay_id(&self) -> u8 {
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

    pub(super) fn on_overlay_key(&mut self, key: KeyEvent) {
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

    /// Scroll the active overlay, clamped to the content the renderer measured.
    fn scroll_overlay(&mut self, delta: isize) {
        let max = self.overlay_lines.saturating_sub(self.overlay_view_rows) as isize;
        let next = (self.overlay_scroll as isize + delta).clamp(0, max.max(0));
        self.overlay_scroll = next as u16;
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
}
