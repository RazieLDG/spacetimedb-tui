//! Live tab — real-time view of the database.
//!
//! Splits the content area into two panes:
//! - **Transaction feed** (top, scrollable): the most recent
//!   [`TxLogEntry`]s from the WebSocket subscription, one line per
//!   transaction with caller identity, inserts/deletes counts, and
//!   affected-table names.
//! - **Connected clients** (bottom): a periodically-refreshed list of
//!   identities pulled from `st_client` via a background SQL query.
//!
//! Both panels read from `AppState` and therefore need no local state
//! of their own — hitting `1`..`6` to switch tabs always returns to
//! the latest snapshot.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::state::{AppState, FocusPanel, TxLogEntry};

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// Render the Live tab — transaction feed on top, clients below.
pub fn render_live(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let border_focused = rgb(theme.border_focused);
    let border_normal = rgb(theme.border_normal);

    let focused = app.focus == FocusPanel::Main;
    let border_color = if focused { border_focused } else { border_normal };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " ⚡ Live ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 4 {
        return;
    }

    // Split: transactions get 2/3 of the height, clients get the rest.
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Ratio(2, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(inner);

    render_tx_feed(sections[0], buf, app);
    render_client_list(sections[1], buf, app);
}

// ── Transaction feed ──────────────────────────────────────────────────────────

fn render_tx_feed(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let fg_primary = rgb(theme.fg_primary);
    let fg_muted = rgb(theme.fg_muted);
    let success = rgb(theme.success);
    let error_fg = rgb(theme.error);
    let border_normal = rgb(theme.border_normal);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_normal))
        .title(Span::styled(
            format!(" 🔁 Transactions ({})  ", app.tx_log.len()),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    if app.tx_log.is_empty() {
        let msg = if app.ws_connected {
            "  (waiting for transaction updates…)"
        } else {
            "  (no WebSocket subscription yet — select a database to connect)"
        };
        let line = Line::from(Span::styled(msg, Style::default().fg(fg_muted)));
        let y = inner.y + inner.height / 2;
        buf.set_line(inner.x, y, &line, inner.width);
        return;
    }

    // Newest-last ordering, so the freshest transaction sits at the
    // bottom of the pane (same convention as the Logs tab).
    let visible = inner.height as usize;
    let total = app.tx_log.len();
    let skip = total.saturating_sub(visible);

    for (i, entry) in app.tx_log.iter().skip(skip).enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let ts = entry.observed_at.format("%H:%M:%S%.3f").to_string();
        let status_span = match entry.committed {
            Some(true) => Span::styled(" ✓ ", Style::default().fg(success)),
            Some(false) => Span::styled(" ✗ ", Style::default().fg(error_fg)),
            None => Span::styled(" • ", Style::default().fg(fg_muted)),
        };
        let ts_span = Span::styled(
            format!("{ts} "),
            Style::default().fg(fg_muted),
        );
        let caller_preview: String = if entry.caller.is_empty() {
            "(system)".to_string()
        } else {
            entry.caller.chars().take(12).collect()
        };
        let caller_span = Span::styled(
            format!("{caller_preview:<12} "),
            Style::default().fg(accent),
        );
        let inserts = entry.total_inserts();
        let deletes = entry.total_deletes();
        let counts_span = Span::styled(
            format!("+{inserts} −{deletes}  "),
            Style::default().fg(fg_primary).add_modifier(Modifier::BOLD),
        );
        let tables: Vec<String> = entry
            .tables
            .iter()
            .map(|(t, i, d)| format!("{t}(+{i}/−{d})"))
            .collect();
        let tables_span = Span::styled(
            truncate(&tables.join(", "), inner.width as usize - 40),
            Style::default().fg(fg_muted),
        );

        let line = Line::from(vec![
            status_span,
            ts_span,
            caller_span,
            counts_span,
            tables_span,
        ]);
        buf.set_line(inner.x, y, &line, inner.width);
    }
}

// ── Client list ───────────────────────────────────────────────────────────────

fn render_client_list(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let fg_muted = rgb(theme.fg_muted);
    let fg_primary = rgb(theme.fg_primary);
    let border_normal = rgb(theme.border_normal);

    let block = Block::default()
        .borders(Borders::NONE)
        .title(Span::styled(
            format!(
                " 👥 Connected clients ({}) ",
                app.live_clients.len()
            ),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    // Draw a top separator manually since BORDERS::NONE skips it.
    if area.height > 0 {
        for x in area.x..area.x + area.width {
            buf[(x, area.y)]
                .set_char('─')
                .set_style(Style::default().fg(border_normal));
        }
    }
    let inner_y = area.y + 1;
    let inner_h = area.height.saturating_sub(1);
    let title = block;
    // Title goes on the separator line itself.
    {
        let t = format!(
            " 👥 Connected clients ({}) ",
            app.live_clients.len()
        );
        buf.set_line(
            area.x + 2,
            area.y,
            &Line::from(Span::styled(
                t,
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )),
            area.width.saturating_sub(4),
        );
    }
    let _ = title; // silence unused var warning; Block::title was for docs

    if inner_h == 0 {
        return;
    }

    if app.live_clients.is_empty() {
        let line = Line::from(Span::styled(
            "  (no clients connected — or st_client not yet polled)",
            Style::default().fg(fg_muted),
        ));
        buf.set_line(area.x, inner_y + inner_h / 2, &line, area.width);
        return;
    }

    for (i, client) in app.live_clients.iter().take(inner_h as usize).enumerate() {
        let y = inner_y + i as u16;
        let id_preview: String = client.identity.chars().take(20).collect();
        let since = client
            .connected_at
            .map(|t| t.format("%H:%M:%S").to_string())
            .unwrap_or_else(|| "?".to_string());
        let line = Line::from(vec![
            Span::styled(" • ", Style::default().fg(accent)),
            Span::styled(
                format!("{id_preview:<22}"),
                Style::default().fg(fg_primary),
            ),
            Span::styled(
                format!(" connected {since}"),
                Style::default().fg(fg_muted),
            ),
        ]);
        buf.set_line(area.x, y, &line, area.width);
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

#[allow(dead_code)]
fn _assert_tx_entry_type(_e: &TxLogEntry) {}
