//! Rendering. The current mode is always visible (design §4) and the waveform
//! is redrawn from cached analysis rather than recomputed (design §7).
//!
//! Split by concern, mirroring `app`: [`browse`]/[`play`]/[`edit`]/[`metadata`]
//! each render their mode, [`waveform`] draws the amplitude view shared by
//! PLAY and EDIT, and [`overlay`] draws the modal popups.

mod browse;
mod edit;
mod metadata;
mod overlay;
mod play;
mod waveform;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Mode};

use browse::render_browse;
use edit::render_edit;
use metadata::render_metadata;
use overlay::render_overlay;
use play::render_play;

/// Eighth-block glyphs used to draw amplitude, plus a blank for silence.
const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

const ACCENT: Color = Color::Cyan;
const RETAINED: Color = Color::Green;
const REMOVED: Color = Color::DarkGray;

pub fn render(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(frame.area());

    render_header(frame, app, header);
    match app.mode {
        Mode::Browse => render_browse(frame, app, body),
        Mode::Play => render_play(frame, app, body),
        Mode::Edit => render_edit(frame, app, body),
        Mode::Metadata => render_metadata(frame, app, body),
    }
    render_footer(frame, app, footer);
    render_overlay(frame, app);
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(
            format!(" {} ", app.mode.label()),
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
    ];

    match &app.session {
        Some(session) => {
            spans.push(Span::styled(
                session.info.file_name(),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            if session.is_dirty() {
                spans.push(Span::styled(" [+]", Style::default().fg(Color::Yellow)));
            }
        }
        None => spans.push(Span::styled(
            app.folder.display().to_string(),
            Style::default().fg(Color::Gray),
        )),
    }

    let right = format!("{} files ", app.files.len());
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let gap = (area.width as usize).saturating_sub(used + right.chars().count());
    spans.push(Span::raw(" ".repeat(gap)));
    spans.push(Span::styled(right, Style::default().fg(Color::DarkGray)));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// ---- footer ---------------------------------------------------------------

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let [hints, message] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    frame.render_widget(
        Paragraph::new(Line::styled(
            hint_text(app),
            Style::default().fg(Color::DarkGray),
        )),
        hints,
    );

    let line = if let Some(prompt) = &app.prompt {
        Line::from(vec![
            Span::styled(
                format!("{}{}", prompt.sigil(), prompt.label()),
                Style::default().fg(ACCENT),
            ),
            Span::raw(prompt.buffer.clone()),
            Span::styled("█", Style::default().fg(ACCENT)),
        ])
    } else if let Some(status) = &app.status {
        let colour = if status.is_error {
            Color::Red
        } else {
            Color::Green
        };
        Line::styled(status.text.clone(), Style::default().fg(colour))
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), message);
}

fn hint_text(app: &App) -> &'static str {
    match app.mode {
        Mode::Browse => {
            "j/k move  gg/G ends  C-d/C-u page  / search  n/N repeat  Enter open  ? help  q quit"
        }
        Mode::Play => {
            "space play  h/l seek  C-h/C-l ±60s  g/G start/end  j/k volume  C-j/C-k song  e edit  m metadata  :w save  Esc back"
        }
        Mode::Edit => {
            "h/l move marker  C-h/C-l large  C-j/C-k song  Tab switch  b/e set here  B/E/i type +10s -1m 50%  a auto  r reset both  :w save  Esc back"
        }
        Mode::Metadata => {
            "j/k field  C-j/C-k song  Enter edit  u revert  :w save  :wq save & leave  Esc back"
        }
    }
}
