/// Metrics tab — server/module dashboard.
///
/// Shows:
/// - Connection count (from MetricsSnapshot.connected_clients)
/// - Table count
/// - Reducer call count
/// - Memory usage
/// - Sparkline placeholders for tx stats
/// - Raw extra metrics key-value pairs
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Sparkline, Widget},
};

use crate::state::{AppState, FocusPanel};

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const BORDER_FOCUSED: Color = Color::Cyan;
const BORDER_NORMAL: Color = Color::Rgb(40, 50, 65);
const FG_MUTED: Color = Color::Rgb(110, 110, 110);
const FG_VALUE: Color = Color::Rgb(229, 192, 123);
const SUCCESS: Color = Color::Rgb(152, 195, 121);
const BG_CARD: Color = Color::Rgb(20, 28, 42);
const BG_SPARK: Color = Color::Rgb(15, 22, 34);

// ── Public entry point ────────────────────────────────────────────────────────

/// Render the metrics dashboard tab.
pub fn render_metrics(area: Rect, buf: &mut Buffer, app: &AppState) {
    let focused = app.focus == FocusPanel::Main;
    let border_color = if focused {
        BORDER_FOCUSED
    } else {
        BORDER_NORMAL
    };

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " 📊 Metrics ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = outer.inner(area);
    outer.render(area, buf);

    if inner.height < 4 {
        return;
    }

    // ── Layout: top stat cards | sparklines | extra ───────────────────────
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // stat cards row
            Constraint::Length(6), // sparklines
            Constraint::Min(0),    // extra key-value pairs
        ])
        .split(inner);

    render_stat_cards(sections[0], buf, app);
    render_sparklines(sections[1], buf, app);
    render_extra_metrics(sections[2], buf, app);
}

// ── Stat cards ────────────────────────────────────────────────────────────────

fn render_stat_cards(area: Rect, buf: &mut Buffer, app: &AppState) {
    // 4 cards side by side
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let m = &app.metrics;

    render_card(
        cards[0],
        buf,
        "Connected Clients",
        &m.connected_clients.to_string(),
        "👥",
        ACCENT,
    );
    render_card(
        cards[1],
        buf,
        "Tables",
        &app.tables.len().to_string(),
        "📋",
        SUCCESS,
    );
    render_card(
        cards[2],
        buf,
        "Reducer Calls",
        &m.total_reducer_calls.to_string(),
        "⚡",
        FG_VALUE,
    );
    render_card(
        cards[3],
        buf,
        "Memory",
        &format_bytes(m.memory_bytes),
        "💾",
        Color::Rgb(86, 182, 194),
    );
}

fn render_card(area: Rect, buf: &mut Buffer, label: &str, value: &str, icon: &str, color: Color) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(40, 55, 75)))
        .style(Style::default().bg(BG_CARD));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 2 {
        return;
    }

    // Icon + label
    let label_line = Line::from(Span::styled(
        format!(" {icon} {label}"),
        Style::default().fg(FG_MUTED),
    ));
    buf.set_line(inner.x, inner.y, &label_line, inner.width);

    // Value (large, centred)
    let val_line = Line::from(Span::styled(
        format!("  {value}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    buf.set_line(inner.x, inner.y + 1, &val_line, inner.width);
}

// ── Sparklines ────────────────────────────────────────────────────────────────

/// Convert a series of cumulative counters into per-tick deltas, so a
/// sparkline actually moves up and down with activity instead of
/// drawing a monotone ramp. The first sample gets a delta of 0 because
/// we don't know what came before it.
fn deltas(series: impl Iterator<Item = u64>) -> Vec<u64> {
    let mut out = Vec::new();
    let mut prev: Option<u64> = None;
    for v in series {
        match prev {
            None => out.push(0),
            Some(p) => out.push(v.saturating_sub(p)),
        }
        prev = Some(v);
    }
    out
}

fn render_sparklines(area: Rect, buf: &mut Buffer, app: &AppState) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    // Build per-tick deltas from the cumulative history so the chart
    // shows real activity instead of an ever-rising ramp.
    let reducer_deltas = deltas(app.metrics_history.iter().map(|s| s.total_reducer_calls));
    let energy_deltas = deltas(app.metrics_history.iter().map(|s| s.total_energy_used));

    render_sparkline_panel(cols[0], buf, "Reducer Δ /10s", &reducer_deltas, ACCENT);
    render_sparkline_panel(cols[1], buf, "Energy Δ /10s", &energy_deltas, FG_VALUE);
}

fn render_sparkline_panel(area: Rect, buf: &mut Buffer, title: &str, data: &[u64], color: Color) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(40, 55, 75)))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(FG_MUTED),
        ))
        .style(Style::default().bg(BG_SPARK));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 || data.is_empty() {
        let msg = Line::from(Span::styled(
            "  (no data yet)",
            Style::default().fg(FG_MUTED),
        ));
        if inner.height > 0 {
            buf.set_line(inner.x, inner.y, &msg, inner.width);
        }
        return;
    }

    let spark = Sparkline::default()
        .data(data)
        .style(Style::default().fg(color).bg(BG_SPARK));
    spark.render(inner, buf);
}

// ── Extra metrics ─────────────────────────────────────────────────────────────

fn render_extra_metrics(area: Rect, buf: &mut Buffer, app: &AppState) {
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::Rgb(40, 55, 75)))
        .title(Span::styled(" Raw Metrics ", Style::default().fg(FG_MUTED)));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    // Fill background
    for y in inner.y..inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            buf[(x, y)]
                .set_char(' ')
                .set_style(Style::default().bg(Color::Rgb(16, 20, 30)));
        }
    }

    let m = &app.metrics;

    // Build key-value lines — always-present fields first
    let mut lines: Vec<Line> = vec![
        kv_line("connected_clients", &m.connected_clients.to_string()),
        kv_line("total_reducer_calls", &m.total_reducer_calls.to_string()),
        kv_line("total_energy_used", &m.total_energy_used.to_string()),
        kv_line("memory_bytes", &format_bytes(m.memory_bytes)),
    ];

    if let Some(ref ts) = m.sampled_at {
        lines.push(kv_line("sampled_at", &ts.to_rfc3339()));
    }

    // Extra dynamic fields
    for (k, v) in &m.extra {
        lines.push(kv_line(k, &v.to_string()));
    }

    if lines.is_empty() {
        let msg = Line::from(Span::styled(
            "  (no metrics — connect to a database)",
            Style::default().fg(FG_MUTED),
        ));
        buf.set_line(inner.x, inner.y, &msg, inner.width);
        return;
    }

    for (i, line) in lines.iter().take(inner.height as usize).enumerate() {
        buf.set_line(inner.x, inner.y + i as u16, line, inner.width);
    }
}

fn kv_line<'a>(key: &str, value: &str) -> Line<'a> {
    let key_w = 30usize;
    let key_padded = format!("  {:<width$}", key, width = key_w);
    Line::from(vec![
        Span::styled(key_padded, Style::default().fg(FG_MUTED)),
        Span::styled(
            value.to_string(),
            Style::default().fg(FG_VALUE).add_modifier(Modifier::BOLD),
        ),
    ])
}

// ── Utilities ─────────────────────────────────────────────────────────────────

fn format_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_are_pairwise_differences() {
        let out = deltas([10u64, 12, 15, 15, 20].into_iter());
        assert_eq!(out, vec![0, 2, 3, 0, 5]);
    }

    #[test]
    fn deltas_handle_counter_reset_as_zero() {
        // Counter reset (e.g. server restart) — `saturating_sub` keeps
        // us from panicking and surfaces the dip as a zero-delta.
        let out = deltas([100u64, 50, 60].into_iter());
        assert_eq!(out, vec![0, 0, 10]);
    }

    #[test]
    fn deltas_empty_input() {
        let out = deltas(std::iter::empty::<u64>());
        assert!(out.is_empty());
    }

    #[test]
    fn format_bytes_picks_human_unit() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
