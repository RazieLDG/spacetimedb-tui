/// Status bar rendered at the very bottom of the screen.
///
/// Shows: last query duration | table count | client count | current mode.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::state::{AppState, ConnectionStatus};

// ── Theme ─────────────────────────────────────────────────────────────────────
const BG: Color = Color::Rgb(22, 30, 42);
const FG_PRIMARY: Color = Color::Rgb(200, 200, 200);
const FG_MUTED: Color = Color::Rgb(110, 110, 110);
const ACCENT: Color = Color::Cyan;
const SUCCESS: Color = Color::Rgb(152, 195, 121);
const WARNING: Color = Color::Rgb(229, 192, 123);
const ERROR_FG: Color = Color::Rgb(224, 108, 117);
const SEP: Color = Color::Rgb(60, 60, 60);

// ── Widget ─────────────────────────────────────────────────────────────────────

/// Status bar widget — reads display data directly from [`AppState`].
pub struct StatusBar<'a> {
    state: &'a AppState,
}

impl<'a> StatusBar<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for StatusBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        // Fill background
        for x in area.x..area.x + area.width {
            buf[(x, area.y)]
                .set_char(' ')
                .set_style(Style::default().bg(BG));
        }

        let app = self.state;

        // ── Left section ─────────────────────────────────────────────────
        let mut left_spans: Vec<Span> = Vec::new();

        // Connection status pill
        let (conn_text, conn_color) = match &app.connection.status {
            ConnectionStatus::Connected => ("● Connected", SUCCESS),
            ConnectionStatus::Connecting => ("◌ Connecting…", WARNING),
            ConnectionStatus::Disconnected => ("○ Disconnected", FG_MUTED),
            ConnectionStatus::Error(_) => ("✗ Error", ERROR_FG),
        };
        left_spans.push(Span::styled(
            format!(" {conn_text} "),
            Style::default().fg(conn_color).bg(BG).add_modifier(Modifier::BOLD),
        ));
        left_spans.push(sep());

        // Database name
        if let Some(db) = app.selected_database() {
            left_spans.push(Span::styled(
                format!(" 🗄 {db} "),
                Style::default().fg(ACCENT).bg(BG),
            ));
            left_spans.push(sep());
        }

        // Table name
        if let Some(tbl) = app.selected_table() {
            left_spans.push(Span::styled(
                format!(" 📋 {} ", tbl.table_name),
                Style::default().fg(FG_PRIMARY).bg(BG),
            ));
            left_spans.push(sep());
        }

        // Last query duration
        if let Some(ref qr) = app.query_result {
            let micros = qr.total_duration_micros;
            let dur_str = if micros >= 1_000_000 {
                format!("{:.1}s", micros as f64 / 1_000_000.0)
            } else if micros >= 1_000 {
                format!("{:.1}ms", micros as f64 / 1_000.0)
            } else {
                format!("{micros}µs")
            };
            left_spans.push(Span::styled(
                format!(" ⏱ {dur_str} "),
                Style::default().fg(FG_MUTED).bg(BG),
            ));
            left_spans.push(sep());

            // Row count
            left_spans.push(Span::styled(
                format!(" {} rows ", qr.row_count()),
                Style::default().fg(FG_MUTED).bg(BG),
            ));
        }

        // Loading indicator
        if app.query_loading {
            left_spans.push(sep());
            left_spans.push(Span::styled(
                " ⟳ loading… ",
                Style::default()
                    .fg(WARNING)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // ── Right section ─────────────────────────────────────────────────
        let mut right_spans: Vec<Span> = Vec::new();

        // Table count
        let tbl_count = app.tables.len();
        right_spans.push(Span::styled(
            format!(" tables:{tbl_count} "),
            Style::default().fg(FG_MUTED).bg(BG),
        ));
        right_spans.push(sep());

        // Connected clients from metrics
        let client_count = app.metrics.connected_clients;
        right_spans.push(Span::styled(
            format!(" clients:{client_count} "),
            Style::default().fg(FG_MUTED).bg(BG),
        ));
        right_spans.push(sep());

        // Notification / error message
        if let Some(ref err) = app.error_message {
            let truncated: String = err.chars().take(40).collect();
            right_spans.push(Span::styled(
                format!(" ⚠ {truncated} "),
                Style::default().fg(ERROR_FG).bg(BG).add_modifier(Modifier::BOLD),
            ));
        } else if let Some((ref notif, _)) = app.notification {
            let truncated: String = notif.chars().take(40).collect();
            right_spans.push(Span::styled(
                format!(" ✓ {truncated} "),
                Style::default().fg(SUCCESS).bg(BG),
            ));
        }

        // Help hint
        right_spans.push(Span::styled(
            " ? help ",
            Style::default().fg(FG_MUTED).bg(BG),
        ));

        // ── Render both sides ─────────────────────────────────────────────
        let left_line = Line::from(left_spans);
        let right_line = Line::from(right_spans);

        // Measure right side width
        let right_w: usize = right_line
            .spans
            .iter()
            .map(|s| s.content.len())
            .sum();

        buf.set_line(area.x, area.y, &left_line, area.width);

        if right_w < area.width as usize {
            let rx = area.x + area.width - right_w as u16;
            buf.set_line(rx, area.y, &right_line, right_w as u16);
        }
    }
}

fn sep<'a>() -> Span<'a> {
    Span::styled("│", Style::default().fg(SEP).bg(BG))
}
