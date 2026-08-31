//! PLAY rendering: file details and the transport bar (design §6, §7).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use ratatui::Frame;

use super::waveform::render_waveform;
use super::ACCENT;
use crate::app::{App, Mode, Session};
use crate::media::probe::MediaInfo;
use crate::timespec::format_timestamp;

pub(super) fn render_play(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(session) = &app.session else {
        app.mode = Mode::Browse;
        return;
    };

    // Three detail lines plus the box borders.
    let [details, waveform, transport] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .areas(area);

    render_details(frame, &session.info, details);
    render_waveform(frame, session, waveform, None);
    render_transport(frame, session, app.audio_device_available(), transport);
}

fn render_details(frame: &mut Frame, info: &MediaInfo, area: Rect) {
    // The container is worth showing next to the codec: `opus` in `ogg`.
    let container = info.format_name.split(',').next().unwrap_or("").to_string();
    let mut facts = vec![info.audio_codec.clone()];
    if !container.is_empty() && container != info.audio_codec {
        facts.push(container);
    }
    if let Some(rate) = info.sample_rate {
        facts.push(format!("{rate} Hz"));
    }
    if let Some(channels) = info.channels {
        facts.push(format!("{channels} ch"));
    }
    if let Some(bitrate) = info.bit_rate {
        facts.push(format!("{} kb/s", bitrate / 1000));
    }
    if info.has_cover_art {
        facts.push("cover art".to_string());
    }
    if info.chapter_count > 0 {
        facts.push(format!("{} chapters", info.chapter_count));
    }

    let title = info.tag("title").unwrap_or("—").to_string();
    let artist = info.tag("artist").unwrap_or("—").to_string();

    let text = vec![
        Line::from(vec![
            Span::styled("Format  ", Style::default().fg(Color::DarkGray)),
            Span::raw(facts.join(" · ")),
        ]),
        Line::from(vec![
            Span::styled("Title   ", Style::default().fg(Color::DarkGray)),
            Span::raw(title),
        ]),
        Line::from(vec![
            Span::styled("Artist  ", Style::default().fg(Color::DarkGray)),
            Span::raw(artist),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().border_type(BorderType::Rounded)),
        area,
    );
}

pub(super) fn render_transport(frame: &mut Frame, session: &Session, device: bool, area: Rect) {
    let position = session.player.position();
    let duration = session.duration();
    let symbol = if session.player.is_playing() {
        "▶"
    } else {
        "‖"
    };

    let mut spans = vec![
        Span::styled(format!(" {symbol} "), Style::default().fg(ACCENT)),
        Span::styled(
            format!(
                "{} / {}",
                format_timestamp(position),
                format_timestamp(duration)
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled(
            format!("vol {:.0}%", session.player.volume()),
            Style::default().fg(Color::Gray),
        ),
    ];
    if !device {
        spans.push(Span::styled(
            "    (no audio device — silent transport)",
            Style::default().fg(Color::Yellow),
        ));
    }
    if let Some(error) = session.player.error() {
        spans.push(Span::styled(
            format!("    playback failed: {error}"),
            Style::default().fg(Color::Red),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::bordered().border_type(BorderType::Rounded)),
        area,
    );
}
