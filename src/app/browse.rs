//! BROWSE mode: the file list, search and navigation (design §5).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::search::{find_backward, find_forward, find_from};
use super::{App, Mode, PendingNav, Prompt, PromptKind};

impl App {
    pub(super) fn on_browse_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half_page = (self.page_rows / 2).max(1) as isize;
        let page = self.page_rows.max(1) as isize;
        let pending_g = std::mem::take(&mut self.pending_g);
        let pending_z = std::mem::take(&mut self.pending_z);

        match key.code {
            KeyCode::Char('g') if pending_g => self.selected = 0,
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.selected = self.files.len().saturating_sub(1),
            KeyCode::Char('z') if pending_z => self.center_viewport_on_selection(),
            KeyCode::Char('z') => self.pending_z = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('d') if ctrl => self.move_selection(half_page),
            KeyCode::Char('u') if ctrl => self.move_selection(-half_page),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Char('/') => {
                self.search_origin = Some(self.selected);
                self.prompt = Some(Prompt::new(PromptKind::Search, String::new()));
            }
            KeyCode::Char('n') => self.repeat_search(true),
            KeyCode::Char('N') => self.repeat_search(false),
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('r') => self.rescan_folder(),
            KeyCode::Char('q') => self.request_nav(PendingNav::Quit),
            _ => {}
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.files.is_empty() {
            return;
        }
        let last = self.files.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
    }

    /// `zz`: re-center the viewport on the current selection, vim-style.
    fn center_viewport_on_selection(&mut self) {
        let visible = self.page_rows.max(1);
        let target = self.selected.saturating_sub(visible / 2);
        let max_offset = self.files.len().saturating_sub(visible);
        *self.list_state.offset_mut() = target.min(max_offset);
    }

    /// `n`/`N`: repeat the last `/` search — over file names in BROWSE, or
    /// (see [`App::repeat_search_fields`]) over metadata fields in METADATA.
    pub(super) fn repeat_search(&mut self, forward: bool) {
        if self.mode == Mode::Metadata {
            self.repeat_search_fields(forward);
            return;
        }
        if self.last_search.is_empty() {
            self.warn("No search pattern. Press / to search.");
            return;
        }
        if self.files.is_empty() {
            return;
        }
        let needle = self.last_search.to_lowercase();
        let count = self.files.len();
        let pred = |i: usize| self.files[i].file_name().to_lowercase().contains(&needle);
        let found = if forward {
            find_forward(self.selected, count, pred)
        } else {
            find_backward(self.selected, count, pred)
        };
        let pattern = self.last_search.clone();
        match found {
            Some(index) => {
                self.selected = index;
                self.info(format!("/{pattern}"));
            }
            None => self.warn(format!("Pattern not found: {pattern}")),
        }
    }

    /// Live "as you type" preview for the `/` prompt: jump the selection to
    /// the first match at or after `search_origin`, wrapping, so the file
    /// currently under the cursor previews before `Enter` commits to it.
    /// With no match, fall back to `search_origin` rather than leaving the
    /// selection on a stale hit from an earlier, longer buffer.
    pub(super) fn live_search(&mut self, needle: &str) {
        if self.mode == Mode::Metadata {
            self.live_search_fields(needle);
            return;
        }
        let Some(origin) = self.search_origin else {
            return;
        };
        if self.files.is_empty() {
            return;
        }
        if needle.is_empty() {
            self.selected = origin;
            return;
        }
        let needle = needle.to_lowercase();
        let count = self.files.len();
        let found = find_from(origin, count, |i| {
            self.files[i].file_name().to_lowercase().contains(&needle)
        });
        self.selected = found.unwrap_or(origin);
    }

    /// Cancel an in-progress `/` search, restoring the pre-search selection
    /// (vim-style incsearch: `Esc` undoes the live preview).
    pub(super) fn cancel_search(&mut self) {
        let Some(origin) = self.search_origin.take() else {
            return;
        };
        if self.mode == Mode::Metadata {
            if let Some(session) = &mut self.session {
                let last = session.fields.len().saturating_sub(1);
                session.field_index = origin.min(last);
            }
            return;
        }
        self.selected = origin;
    }

    pub(super) fn current_file_matches(&self, needle: &str) -> bool {
        let needle = needle.to_lowercase();
        self.files
            .get(self.selected)
            .is_some_and(|f| f.file_name().to_lowercase().contains(&needle))
    }

    pub(super) fn open_selected(&mut self) {
        if self.files.is_empty() {
            self.warn("No audio files in this folder.");
            return;
        }
        let index = self.selected;
        if self.session_is_dirty() {
            self.overlay = super::Overlay::ConfirmDiscard(PendingNav::Open(index));
            return;
        }
        self.open_index(index);
    }
}

#[cfg(test)]
mod tests;
