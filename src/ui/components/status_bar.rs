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
// Most rendering still uses these constants; the few user-facing colours
// that change between themes (accent, success/warning/error, foreground)
// are pulled from `app.theme` at render time so that `--theme light` /
// `--theme high-contrast` actually flip the visible palette.
const BG: Color = Color::Rgb(22, 30, 42);
const SEP: Color = Color::Rgb(60, 60, 60);

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

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
        let theme = &app.theme;
        let accent = rgb(theme.accent);
        let success = rgb(theme.success);
        let warning = rgb(theme.warning);
        let error_fg = rgb(theme.error);
        let fg_primary = rgb(theme.fg_primary);
        let fg_muted = rgb(theme.fg_muted);

        // ── Left section ─────────────────────────────────────────────────
        let mut left_spans: Vec<Span> = Vec::new();

        // Connection status pill
        let (conn_text, conn_color) = match &app.connection.status {
            ConnectionStatus::Connected => ("● Connected", success),
            ConnectionStatus::Connecting => ("◌ Connecting…", warning),
            ConnectionStatus::Disconnected => ("○ Disconnected", fg_muted),
            ConnectionStatus::Error(_) => ("✗ Error", error_fg),
        };
        left_spans.push(Span::styled(
            format!(" {conn_text} "),
            Style::default()
                .fg(conn_color)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ));
        left_spans.push(sep());

        // Live WebSocket subscription pill — shows one of three states:
        //   ● LIVE              — subscribed, receiving updates
        //   ◌ reconnect in Ns   — waiting to reconnect after a drop
        //   ○ live              — idle (no connection)
        let (live_text, live_color) = if app.ws_connected {
            (" ● LIVE ".to_string(), success)
        } else if let Some(deadline) = app.ws_reconnect_deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let secs = remaining.as_secs().max(1);
            (format!(" ◌ reconnect in {secs}s "), warning)
        } else {
            (" ○ live ".to_string(), fg_muted)
        };
        left_spans.push(Span::styled(
            live_text,
            Style::default()
                .fg(live_color)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ));
        left_spans.push(sep());

        // Database name
        if let Some(db) = app.selected_database() {
            left_spans.push(Span::styled(
                format!(" 🗄 {db} "),
                Style::default().fg(accent).bg(BG),
            ));
            left_spans.push(sep());
        }

        // Table name
        if let Some(tbl) = app.selected_table() {
            left_spans.push(Span::styled(
                format!(" 📋 {} ", tbl.table_name),
                Style::default().fg(fg_primary).bg(BG),
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
                Style::default().fg(fg_muted).bg(BG),
            ));
            left_spans.push(sep());

            // Row count
            left_spans.push(Span::styled(
                format!(" {} rows ", qr.row_count()),
                Style::default().fg(fg_muted).bg(BG),
            ));
        }

        // Loading indicator
        if app.query_loading {
            left_spans.push(sep());
            left_spans.push(Span::styled(
                " ⟳ loading… ",
                Style::default()
                    .fg(warning)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        // Spreadsheet edit mode indicator — always visible while edit
        // mode is open, with a live pending-edit counter.
        if let Some(ref em) = app.edit_mode {
            left_spans.push(sep());
            left_spans.push(Span::styled(
                format!(" ✎ EDIT — {} pending ", em.pending_count()),
                Style::default()
                    .fg(Color::Rgb(255, 220, 100))
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
            Style::default().fg(fg_muted).bg(BG),
        ));
        right_spans.push(sep());

        // Connected clients from metrics
        let client_count = app.metrics.connected_clients;
        right_spans.push(Span::styled(
            format!(" clients:{client_count} "),
            Style::default().fg(fg_muted).bg(BG),
        ));
        right_spans.push(sep());

        // Notification / error message
        if let Some(ref err) = app.error_message {
            let truncated: String = err.chars().take(40).collect();
            right_spans.push(Span::styled(
                format!(" ⚠ {truncated} "),
                Style::default()
                    .fg(error_fg)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if let Some((ref notif, _)) = app.notification {
            let truncated: String = notif.chars().take(40).collect();
            right_spans.push(Span::styled(
                format!(" ✓ {truncated} "),
                Style::default().fg(success).bg(BG),
            ));
        }

        // Help hint
        right_spans.push(Span::styled(
            " ? help ",
            Style::default().fg(fg_muted).bg(BG),
        ));

        // ── Render both sides ─────────────────────────────────────────────
        let left_line = Line::from(left_spans);
        let right_line = Line::from(right_spans);

        // Measure right side width
        let right_w: usize = right_line.spans.iter().map(|s| s.content.len()).sum();

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
