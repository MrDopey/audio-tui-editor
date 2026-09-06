//! The text prompt at the bottom of the screen: search, `:` commands, marker
//! expressions and metadata field edits (design §19).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, MarkerKind};
use crate::media::probe::METADATA_FIELDS;

/// What a text prompt at the bottom of the screen is collecting.
#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    Command,
    Search,
    Marker(MarkerKind),
    Cursor,
    MetadataField(usize),
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    pub cursor: usize,
    /// Shadow text shown in place of an empty buffer (e.g. the cursor's
    /// current position). Navigating into it copies it into `buffer` so it
    /// can be edited in place; typing a fresh character discards it instead,
    /// so the user's own input always starts from a blank field.
    pub placeholder: Option<String>,
}

impl Prompt {
    pub(super) fn new(kind: PromptKind, initial: String) -> Self {
        Prompt {
            cursor: initial.chars().count(),
            kind,
            buffer: initial,
            placeholder: None,
        }
    }

    /// A prompt that starts empty but shows `placeholder` as shadow text.
    pub(super) fn with_placeholder(kind: PromptKind, placeholder: String) -> Self {
        Prompt {
            cursor: 0,
            kind,
            buffer: String::new(),
            placeholder: Some(placeholder),
        }
    }

    /// Copy the placeholder into the buffer, cursor at its end, so it can be
    /// edited in place. No-op once the buffer already has content.
    fn materialize(&mut self) {
        if !self.buffer.is_empty() {
            return;
        }
        if let Some(placeholder) = self.placeholder.take() {
            self.cursor = placeholder.chars().count();
            self.buffer = placeholder;
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
            PromptKind::Cursor => "Cursor: ".to_string(),
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

impl App {
    pub(super) fn on_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.cancel_search();
                return;
            }
            KeyCode::Enter => {
                if let Some(mut prompt) = self.prompt.take() {
                    // An untouched placeholder still represents a real,
                    // parseable value (e.g. the cursor's current position),
                    // so accept it on submit rather than treating it as an
                    // empty input.
                    prompt.materialize();
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
            KeyCode::Backspace => {
                prompt.materialize();
                prompt.backspace();
            }
            KeyCode::Left => {
                prompt.materialize();
                prompt.cursor = prompt.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                prompt.materialize();
                prompt.cursor = (prompt.cursor + 1).min(prompt.buffer.chars().count());
            }
            KeyCode::Home => {
                prompt.materialize();
                prompt.cursor = 0;
            }
            KeyCode::End => {
                prompt.materialize();
                prompt.cursor = prompt.buffer.chars().count();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                prompt.buffer.clear();
                prompt.cursor = 0;
                prompt.placeholder = None;
            }
            KeyCode::Char(c) => {
                // Typing discards the placeholder outright rather than
                // materializing it first: the user's own input starts fresh.
                prompt.placeholder = None;
                prompt.insert(c);
            }
            _ => {}
        }

        // Eager ("incsearch"-style) search: preview the match live as the
        // user types, before `Enter` commits to it.
        if let Some(prompt) = &self.prompt {
            if matches!(prompt.kind, PromptKind::Search) {
                let buffer = prompt.buffer.clone();
                self.live_search(&buffer);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
