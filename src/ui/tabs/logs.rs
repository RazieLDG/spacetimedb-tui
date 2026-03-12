/// Logs tab — scrollable live log viewer.
///
/// Features:
/// - Auto-scroll (follow mode) — new lines scroll to bottom automatically.
/// - Space to pause/resume auto-scroll.
/// - Log level colour coding.
/// - Timestamp + level + message display.
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::api::types::LogLevel;
use crate::state::{AppState, FocusPanel};

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const BORDER_FOCUSED: Color = Color::Cyan;
const BORDER_NORMAL: Color = Color::Rgb(40, 50, 65);
const FG_MUTED: Color = Color::Rgb(110, 110, 110);
const FG_TS: Color = Color::Rgb(140, 140, 160);
const FG_TRACE: Color = Color::Rgb(120, 120, 140);
const FG_DEBUG: Color = Color::Rgb(86, 182, 194);
const FG_INFO: Color = Color::Rgb(152, 195, 121);
const FG_WARN: Color = Color::Rgb(229, 192, 123);
const FG_ERROR: Color = Color::Rgb(224, 108, 117);
const FG_PANIC: Color = Color::Rgb(255, 80, 80);
const FG_MSG: Color = Color::Rgb(210, 210, 210);
const PAUSED_BG: Color = Color::Rgb(60, 40, 20);

// ── Public entry point ────────────────────────────────────────────────────────

/// Render the log viewer tab.
pub fn render_logs(area: Rect, buf: &mut Buffer, app: &AppState) {
    let focused = app.focus == FocusPanel::Main;
    let border_color = if focused { BORDER_FOCUSED } else { BORDER_NORMAL };
    let paused = !app.log_follow;

    let title = if paused {
        Span::styled(
            " 📜 Logs  [PAUSED — Space to resume] ",
            Style::default()
                .fg(FG_WARN)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " 📜 Logs  [LIVE ▶] ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    };

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = outer_block.inner(area);
    outer_block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    // ── Layout: toolbar (1) | log lines (rest) ────────────────────────────
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    let toolbar_area = sections[0];
    let log_area     = sections[1];

    render_toolbar(toolbar_area, buf, app, paused);
    render_log_lines(log_area, buf, app, paused);
}

// ── Toolbar ───────────────────────────────────────────────────────────────────

fn render_toolbar(area: Rect, buf: &mut Buffer, app: &AppState, paused: bool) {
    for x in area.x..area.x + area.width {
        buf[(x, area.y)]
            .set_char(' ')
            .set_style(Style::default().bg(Color::Rgb(20, 26, 38)));
    }

    let total = app.log_buffer.len();
    let db = app.selected_database().unwrap_or("—");

    let left = Line::from(vec![
        Span::styled(
            format!(" {total} lines "),
            Style::default().fg(FG_MUTED),
        ),
        Span::styled("│ ", Style::default().fg(Color::Rgb(50, 50, 60))),
        Span::styled(
            format!("db: {db} "),
            Style::default().fg(ACCENT),
        ),
    ]);
    buf.set_line(area.x, area.y, &left, area.width);

    let hint = if paused {
        " Space:resume  r:refresh  c:clear "
    } else {
        " Space:pause  r:refresh  c:clear "
    };
    let hint_span = Span::styled(hint, Style::default().fg(FG_MUTED));
    let hint_w = hint.len() as u16;
    let hint_x = area.x + area.width.saturating_sub(hint_w);
    buf.set_line(hint_x, area.y, &Line::from(hint_span), hint_w);
}

// ── Log lines ─────────────────────────────────────────────────────────────────

fn render_log_lines(area: Rect, buf: &mut Buffer, app: &AppState, paused: bool) {
    if area.height == 0 {
        return;
    }

    // Fill background — slightly different tint when paused
    let bg = if paused {
        PAUSED_BG
    } else {
        Color::Rgb(15, 18, 26)
    };
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf[(x, y)]
                .set_char(' ')
                .set_style(Style::default().bg(bg));
        }
    }

    if app.log_buffer.is_empty() {
        let msg = Line::from(Span::styled(
            "  (no log entries — connect to a database to stream logs)",
            Style::default().fg(FG_MUTED),
        ));
        let y = area.y + area.height / 2;
        buf.set_line(area.x, y, &msg, area.width);
        return;
    }

    let visible_h = area.height as usize;
    let total = app.log_buffer.len();

    // In auto-scroll mode, always show the tail; otherwise respect log_scroll.
    let scroll = if app.log_follow {
        total.saturating_sub(visible_h)
    } else {
        app.log_scroll.min(total.saturating_sub(1))
    };

    for (row, entry) in app
        .log_buffer
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_h)
    {
        let y = area.y + (row - scroll) as u16;
        if y >= area.y + area.height {
            break;
        }

        // Timestamp
        let ts = entry
            .ts
            .map(|t| t.format("%H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| "??:??:??.???".to_string());

        let (level_str, level_color) = level_display(&entry.level);

        let ts_span = Span::styled(
            format!("{ts} "),
            Style::default().fg(FG_TS).bg(bg),
        );
        let level_span = Span::styled(
            format!("{level_str:<5} "),
            Style::default()
                .fg(level_color)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );

        // Optional target
        let target_span = if let Some(ref t) = entry.target {
            Span::styled(
                format!("[{t}] "),
                Style::default().fg(FG_MUTED).bg(bg),
            )
        } else {
            Span::raw("")
        };

        let msg_span = Span::styled(
            entry.message.clone(),
            Style::default().fg(FG_MSG).bg(bg),
        );

        let line = Line::from(vec![ts_span, level_span, target_span, msg_span]);
        buf.set_line(area.x, y, &line, area.width);
    }

    // Scroll indicator
    if total > visible_h {
        let pct = scroll * 100 / total.max(1);
        let indicator = format!(" {}/{} ({}%) ", scroll + visible_h.min(total - scroll), total, pct);
        let ind_x = area.x + area.width.saturating_sub(indicator.len() as u16);
        let ind_y = area.y + area.height - 1;
        let line = Line::from(Span::styled(
            indicator,
            Style::default().fg(FG_MUTED).bg(bg),
        ));
        buf.set_line(ind_x, ind_y, &line, area.width);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn level_display(level: &LogLevel) -> (&'static str, Color) {
    match level {
        LogLevel::Trace   => ("TRACE", FG_TRACE),
        LogLevel::Debug   => ("DEBUG", FG_DEBUG),
        LogLevel::Info    => ("INFO",  FG_INFO),
        LogLevel::Warn    => ("WARN",  FG_WARN),
        LogLevel::Error   => ("ERROR", FG_ERROR),
        LogLevel::Panic   => ("PANIC", FG_PANIC),
        LogLevel::Unknown => ("?????", FG_MUTED),
    }
}
