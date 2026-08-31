//! BROWSE rendering: the file list (design §5).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, List, ListItem, Paragraph};
use ratatui::Frame;

use super::ACCENT;
use crate::app::App;

pub(super) fn render_browse(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Files ")
        .title_style(Style::default().fg(ACCENT));

    if app.files.is_empty() {
        let text = vec![
            Line::raw(""),
            Line::raw("  No supported audio files were found in this folder."),
            Line::raw(""),
            Line::raw("  Start audioedit elsewhere with --folder, or press r to rescan."),
        ];
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let inner = block.inner(area);
    app.page_rows = inner.height as usize;

    let name_width = (inner.width as usize).saturating_sub(20).max(10);
    let items: Vec<ListItem> = app
        .file_rows()
        .iter()
        .map(|(name, duration, format)| {
            ListItem::new(Line::from(vec![
                Span::raw(format!(
                    "{:<width$} ",
                    crate::text::truncate_with_ellipsis(name, name_width),
                    width = name_width
                )),
                Span::styled(format!("{duration:>8} "), Style::default().fg(Color::Gray)),
                Span::styled(format!("{format:<6}"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    app.list_state.select(Some(app.selected));
    let list = List::new(items)
        .block(block)
        .highlight_symbol("> ")
        .highlight_style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD));
    frame.render_stateful_widget(list, area, &mut app.list_state);
}
