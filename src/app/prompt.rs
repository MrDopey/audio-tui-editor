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
    MetadataField(usize),
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub kind: PromptKind,
    pub buffer: String,
    pub cursor: usize,
}

impl Prompt {
    pub(super) fn new(kind: PromptKind, initial: String) -> Self {
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

impl App {
    pub(super) fn on_prompt_key(&mut self, key: KeyEvent) {
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
