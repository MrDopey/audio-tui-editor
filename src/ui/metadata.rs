//! METADATA rendering: editable tag fields (design §18).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::ACCENT;
use crate::app::{App, Mode};

pub(super) fn render_metadata(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    let lines: Vec<Line> = session
        .fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = index == session.field_index;
            let marker = if selected { "▸ " } else { "  " };
            let value_style = if field.is_changed() {
                Style::default().fg(Color::Yellow)
            } else if field.value.is_none() {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
            };
            let value = if field.display().is_empty() {
                "—"
            } else {
                field.display()
            };
            Line::from(vec![
                Span::styled(marker, Style::default().fg(ACCENT)),
                Span::styled(
                    format!("{:<14}", format!("{}:", field.label)),
                    if selected {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    },
                ),
                Span::styled(value.to_string(), value_style),
                if field.is_changed() {
                    Span::styled("  (edited)", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Metadata ")
                .title_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}
