//! METADATA rendering: editable tag fields (design §18).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::cover_art_widget::{split_for_cover_art, COVER_ART_ROWS};
use super::ACCENT;
use crate::app::{App, Mode};

pub(super) fn render_metadata(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    // Unlike PLAY/EDIT, METADATA has no existing top band to carve a corner
    // out of — one is introduced here, but only when there is actually an
    // image to reserve space for, so the common case (no cover art, or the
    // terminal/toggle doesn't support it) keeps today's full-area list
    // exactly as it was.
    let show_cover_art =
        app.show_cover_art && app.cover_art_supported() && session.info.has_cover_art;
    let area = if show_cover_art {
        let [top, rest] =
            Layout::vertical([Constraint::Length(COVER_ART_ROWS), Constraint::Min(0)]).areas(area);
        let (_, cover_art_area) = split_for_cover_art(top, true);
        app.cover_art_area = cover_art_area;
        rest
    } else {
        area
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
