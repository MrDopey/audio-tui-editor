//! EDIT rendering: the marker panel (design §8–§11).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::play::render_transport;
use super::waveform::render_waveform;
use super::{ACCENT, RETAINED};
use crate::app::{App, MarkerKind, Mode, Session};
use crate::timespec::format_timestamp;

pub(super) fn render_edit(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    let [markers, waveform, transport] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .areas(area);

    render_markers(frame, session, markers);
    render_waveform(frame, session, waveform, Some(()));
    render_transport(frame, session, app.audio_device_available(), transport);
}

fn render_markers(frame: &mut Frame, session: &Session, area: Rect) {
    let kept = (session.end.seconds() - session.begin.seconds()).max(0.0);
    let removed_begin = session.begin.seconds();
    let removed_end = (session.duration() - session.end.seconds()).max(0.0);

    let marker_line = |kind: MarkerKind, marker: &crate::timespec::Marker| {
        let active = session.active == kind;
        let arrow = if active { "▸ " } else { "  " };
        let style = if active {
            Style::default().fg(RETAINED).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        Line::from(vec![
            Span::styled(arrow, Style::default().fg(RETAINED)),
            Span::styled(
                format!("{:<10}", format!("{}:", kind.label())),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(marker.to_string(), style),
        ])
    };

    let status = if session.auto.is_running() {
        Span::styled(
            "detecting automatic markers…",
            Style::default().fg(Color::DarkGray),
        )
    } else if let Some(error) = session.auto.error() {
        Span::styled(
            format!("auto-detect failed: {error}"),
            Style::default().fg(Color::Red),
        )
    } else if session.markers_dirty {
        Span::styled("markers edited", Style::default().fg(Color::Yellow))
    } else {
        Span::styled("automatic markers", Style::default().fg(Color::DarkGray))
    };

    let text = vec![
        marker_line(MarkerKind::Begin, &session.begin),
        marker_line(MarkerKind::End, &session.end),
        Line::from(vec![
            Span::styled("  retained:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format_timestamp(kept)),
            Span::styled("   removed:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{removed_begin:.3}s + {removed_end:.3}s")),
        ]),
        Line::from(vec![Span::raw("  "), status]),
    ];

    frame.render_widget(
        Paragraph::new(text).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title(" Markers ")
                .title_style(Style::default().fg(ACCENT)),
        ),
        area,
    );
}
