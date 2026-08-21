//! Live tab — database activity overview.
//!
//! Splits the content area into two panes:
//! - **Transaction feed** (top): scoped live inserts/deletes for the
//!   selected table, driven by the WebSocket subscription.
//! - **Connected clients** (bottom): a periodically-refreshed list of
//!   identities pulled from `st_client` via a background SQL query.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::state::{AppState, FocusPanel};

/// Whether automatic Live subscriptions are available in this build.
///
/// Live is always scoped to the selected table. All-table subscribe
/// is not offered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveAvailability {
    ScopedToSelectedTable,
}

/// Report the current availability of automatic Live subscriptions.
pub fn live_subscription_availability() -> LiveAvailability {
    LiveAvailability::ScopedToSelectedTable
}

/// User-facing explanation of the Live contract.
pub fn live_status_message() -> &'static str {
    "Live updates are scoped to the selected table. Ctrl+L toggles them. Broad automatic all-table subscriptions are unavailable."
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// Render the Live tab — disabled notice on top, clients below.
///
/// The top pane is the scoped transaction feed for the selected table.
pub fn render_live(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let border_focused = rgb(theme.border_focused);
    let border_normal = rgb(theme.border_normal);

    let focused = app.focus == FocusPanel::Main;
    let border_color = if focused {
        border_focused
    } else {
        border_normal
    };

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

    // Split: the top pane (notice or feed) gets 2/3, clients get the rest.
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
        .split(inner);

    match live_subscription_availability() {
        LiveAvailability::ScopedToSelectedTable => {
            render_transaction_feed(sections[0], buf, app);
        }
    }
    render_client_list(sections[1], buf, app);
}

// ── Transaction feed ─────────────────────────────────────────────────────────

fn render_transaction_feed(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let fg_muted = rgb(theme.fg_muted);
    let fg_primary = rgb(theme.fg_primary);
    let border_normal = rgb(theme.border_normal);
    let success = rgb(theme.success);
    let warning = rgb(theme.warning);

    let status = if !app.live_enabled {
        "off"
    } else if app.ws_connected {
        "live"
    } else {
        "connecting"
    };
    let table = app
        .selected_table()
        .map(|t| t.table_name.as_str())
        .unwrap_or("no table");

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_normal))
        .title(Span::styled(
            format!(" 🔁 Transactions  [{status}]  {table}  "),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    if app.live_events.is_empty() {
        let msg = if !app.live_enabled {
            "  Live is off — press Ctrl+L to subscribe to the selected table."
        } else if app.selected_table().is_none() {
            "  Select a table in the sidebar to start scoped live updates."
        } else {
            "  Waiting for live events… row changes on the selected table appear here."
        };
        let y = inner.y + inner.height.saturating_sub(2) / 2;
        buf.set_line(
            inner.x,
            y,
            &Line::from(Span::styled(msg, Style::default().fg(fg_muted))),
            inner.width,
        );
        if inner.height > 2 {
            buf.set_line(
                inner.x,
                y.saturating_add(1),
                &Line::from(Span::styled(
                    format!("  {}", live_status_message()),
                    Style::default().fg(fg_muted),
                )),
                inner.width,
            );
        }
        return;
    }

    let visible = inner.height as usize;
    let skip = app.live_events.len().saturating_sub(visible);
    for (row, event) in app.live_events.iter().skip(skip).enumerate() {
        let y = inner.y + row as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let ts = event.at.format("%H:%M:%S").to_string();
        let kind = if event.snapshot { "snapshot" } else { "update" };
        let line = Line::from(vec![
            Span::styled(format!(" {ts} "), Style::default().fg(fg_muted)),
            Span::styled(
                format!("{kind:<8} "),
                Style::default().fg(if event.snapshot { accent } else { fg_primary }),
            ),
            Span::styled(
                format!("{}  ", event.table_name),
                Style::default().fg(fg_primary),
            ),
            Span::styled(format!("+{} ", event.inserts), Style::default().fg(success)),
            Span::styled(format!("-{}", event.deletes), Style::default().fg(warning)),
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

    let block = Block::default().borders(Borders::NONE).title(Span::styled(
        format!(" 👥 Connected clients ({}) ", app.live_clients.len()),
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
        let t = format!(" 👥 Connected clients ({}) ", app.live_clients.len());
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
            Span::styled(format!("{id_preview:<22}"), Style::default().fg(fg_primary)),
            Span::styled(format!(" connected {since}"), Style::default().fg(fg_muted)),
        ]);
        buf.set_line(area.x, y, &line, area.width);
    }
}

#[cfg(test)]
mod live_tab_tests {
    use super::*;

    #[test]
    fn live_surface_explains_scoped_state() {
        let text = live_status_message();

        assert!(text.contains("scoped to the selected table"));
        assert!(text.contains("Ctrl+L"));
        assert!(text.contains("all-table subscriptions are unavailable"));
    }

    #[test]
    fn all_table_subscribe_command_is_not_available() {
        assert_eq!(
            live_subscription_availability(),
            LiveAvailability::ScopedToSelectedTable
        );
    }
}
