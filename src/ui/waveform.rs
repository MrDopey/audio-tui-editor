//! Waveform rendering, shared by PLAY and EDIT (design §7).

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::{ACCENT, BLOCKS, REMOVED, RETAINED};
use crate::app::Session;
use crate::timespec::format_timestamp;

/// Draw the waveform, the playback cursor and (in EDIT) the marker range.
pub(super) fn render_waveform(
    frame: &mut Frame,
    session: &Session,
    area: Rect,
    markers: Option<()>,
) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Waveform ")
        .title_style(Style::default().fg(ACCENT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 4 || inner.height < 3 {
        return;
    }

    let width = inner.width as usize;
    let duration = session.duration().max(0.001);

    // Row 0: the time scale. Last row: the playback cursor. Rest: amplitude.
    let scale = Line::from(vec![
        Span::styled("00:00", Style::default().fg(Color::DarkGray)),
        Span::raw(" ".repeat(width.saturating_sub(10 + 3))),
        Span::styled(
            format!("{:>8}", format_timestamp(duration)),
            Style::default().fg(Color::DarkGray),
        ),
    ]);

    let bar_rows = (inner.height as usize).saturating_sub(2).max(1);
    let columns = match session.waveform.ready() {
        Some(waveform) => waveform.downsample(width),
        None => vec![(0.0, 0.0); width],
    };

    let position_column = column_for(session.player.position(), duration, width);
    let (begin_column, end_column) = (
        column_for(session.begin.seconds(), duration, width),
        column_for(session.end.seconds(), duration, width),
    );
    let show_markers = markers.is_some();

    let mut lines = vec![scale];
    for row in (0..bar_rows).rev() {
        let spans = columns
            .iter()
            .enumerate()
            .map(|(col, (peak, rms))| {
                // Peak sets the outline, RMS fills the body.
                let level = (*peak).max(*rms * 1.2).clamp(0.0, 1.0);
                let eighths = (level as f64 * (bar_rows * 8) as f64).round() as isize;
                let in_row = (eighths - (row * 8) as isize).clamp(0, 8) as usize;
                let mut style = Style::default().fg(ACCENT);
                if show_markers {
                    style = if col >= begin_column && col <= end_column {
                        Style::default().fg(RETAINED)
                    } else {
                        Style::default().fg(REMOVED)
                    };
                }
                if col == position_column {
                    style = style.bg(Color::DarkGray);
                }
                Span::styled(BLOCKS[in_row].to_string(), style)
            })
            .collect::<Vec<_>>();
        lines.push(Line::from(spans));
    }

    // The cursor line, plus marker letters when editing.
    let mut cursor: Vec<char> = vec![' '; width];
    if position_column < width {
        cursor[position_column] = '│';
    }
    let mut cursor_spans: Vec<Span> = Vec::with_capacity(width);
    for (col, ch) in cursor.iter().enumerate() {
        let (glyph, style) = if show_markers && col == begin_column {
            (
                'b',
                Style::default().fg(RETAINED).add_modifier(Modifier::BOLD),
            )
        } else if show_markers && col == end_column {
            (
                'e',
                Style::default().fg(RETAINED).add_modifier(Modifier::BOLD),
            )
        } else {
            (*ch, Style::default().fg(Color::White))
        };
        cursor_spans.push(Span::styled(glyph.to_string(), style));
    }
    lines.push(Line::from(cursor_spans));

    if let Some(error) = session.waveform.error() {
        lines.push(Line::styled(
            format!("waveform unavailable: {error}"),
            Style::default().fg(Color::Red),
        ));
    } else if session.waveform.is_running() {
        lines.push(Line::styled(
            "analysing waveform…",
            Style::default().fg(Color::DarkGray),
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn column_for(seconds: f64, duration: f64, width: usize) -> usize {
    if width == 0 || duration <= 0.0 {
        return 0;
    }
    let fraction = (seconds / duration).clamp(0.0, 1.0);
    ((fraction * width as f64) as usize).min(width - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::waveform::Waveform;

    #[test]
    fn columns_map_positions_across_the_full_width() {
        assert_eq!(column_for(0.0, 100.0, 80), 0);
        assert_eq!(column_for(100.0, 100.0, 80), 79);
        assert_eq!(column_for(50.0, 100.0, 80), 40);
    }

    #[test]
    fn column_mapping_survives_degenerate_input() {
        assert_eq!(column_for(5.0, 0.0, 80), 0);
        assert_eq!(column_for(5.0, 10.0, 0), 0);
        assert_eq!(column_for(-5.0, 10.0, 8), 0);
        assert_eq!(column_for(1000.0, 10.0, 8), 7);
    }

    #[test]
    fn waveform_downsamples_to_the_drawn_width() {
        let waveform = Waveform {
            duration: 10.0,
            peaks: (0..500).map(|i| (i % 100) as f32 / 100.0).collect(),
            rms: (0..500).map(|i| (i % 100) as f32 / 200.0).collect(),
        };
        assert_eq!(waveform.downsample(64).len(), 64);
    }
}
