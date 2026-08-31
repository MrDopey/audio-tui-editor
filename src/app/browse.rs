//! BROWSE mode: the file list, search and navigation (design §5).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, PendingNav, Prompt, PromptKind};

impl App {
    pub(super) fn on_browse_key(&mut self, key: KeyEvent) {
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

    pub(super) fn repeat_search(&mut self, forward: bool) {
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
mod tests {
    use super::super::tests::{app, press, press_ctrl, type_text};
    use crate::app::{Overlay, PendingNav};
    use ratatui::crossterm::event::KeyCode;

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
    fn browse_q_quits_when_nothing_is_open() {
        let mut app = app(&[("a.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn choosing_save_in_the_discard_dialog_remembers_the_original_target() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('l')); // dirty

        app.selected = 1;
        app.open_selected();
        assert!(matches!(
            app.overlay,
            Overlay::ConfirmDiscard(PendingNav::Open(1))
        ));

        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.pending_nav_after_save, Some(PendingNav::Open(1)));
    }

    #[test]
    fn a_successful_save_continues_to_the_remembered_navigation_target() {
        let mut app = app(&[("a.opus", 600.0), ("b.opus", 60.0)]);
        app.overlay = Overlay::None;
        press(&mut app, KeyCode::Enter); // opens a.opus, index 0

        app.pending_nav_after_save = Some(PendingNav::Open(1));
        let (tx, rx) = std::sync::mpsc::channel();
        app.save_rx = Some(rx);
        tx.send(Ok(super::super::save::fake_save_outcome("/rec/a.opus")))
            .unwrap();
        app.poll_save();

        assert_eq!(
            app.session.as_ref().map(|s| s.index),
            Some(1),
            "must continue to the file the user selected, not just close to BROWSE"
        );
        assert!(app.pending_nav_after_save.is_none());
    }

    #[test]
    fn opening_a_file_with_no_files_present_warns_and_does_not_open() {
        let mut app = app(&[]);
        app.overlay = Overlay::None;
        app.open_selected();
        assert!(app.status.as_ref().unwrap().is_error);
        assert!(app.session.is_none());
    }
}
