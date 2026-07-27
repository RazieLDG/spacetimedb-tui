//! Live tab — database activity overview.
//!
//! Splits the content area into two panes:
//! - **Disabled notice** (top): explains that automatic Live
//!   subscriptions are temporarily unavailable in Phase 1.
//! - **Connected clients** (bottom): a periodically-refreshed list of
//!   identities pulled from `st_client` via a background SQL query
//!   (metadata polling, not row-level Live).
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

use crate::state::{AppState, FocusPanel};

/// Whether automatic Live subscriptions are available in this build.
///
/// Phase 1 disables unbounded all-table subscriptions while the safety
/// controls are tightened. Bounded, scoped Live will return later.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveAvailability {
    TemporarilyUnavailable,
}

/// Report the current availability of automatic Live subscriptions.
pub fn live_subscription_availability() -> LiveAvailability {
    LiveAvailability::TemporarilyUnavailable
}

/// User-facing explanation shown in place of the Live feed while
/// automatic subscriptions are disabled.
pub fn phase_one_live_disabled_message() -> &'static str {
    "Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable."
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

/// Render the Live tab — disabled notice on top, clients below.
///
/// The top pane branches on [`live_subscription_availability`]: while
/// subscriptions are [`LiveAvailability::TemporarilyUnavailable`] it draws
/// the Phase 1 disabled notice; a future `Available` variant would restore
/// the transaction feed.
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
        LiveAvailability::TemporarilyUnavailable => {
            render_disabled_notice(sections[0], buf, app);
        }
    }
    render_client_list(sections[1], buf, app);
}

// ── Disabled notice ──────────────────────────────────────────────────────────

fn render_disabled_notice(area: Rect, buf: &mut Buffer, app: &AppState) {
    let theme = &app.theme;
    let accent = rgb(theme.accent);
    let fg_muted = rgb(theme.fg_muted);
    let border_normal = rgb(theme.border_normal);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(border_normal))
        .title(Span::styled(
            " 🔁 Transactions  ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    let msg = phase_one_live_disabled_message();
    let y = inner.y + inner.height / 2;
    let line = Line::from(Span::styled(
        format!("  {msg}"),
        Style::default().fg(fg_muted),
    ));
    buf.set_line(inner.x, y, &line, inner.width);
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
mod phase_one_live_tests {
    use super::*;

    #[test]
    fn phase_one_live_surface_explains_disabled_state() {
        let text = phase_one_live_disabled_message();

        assert!(text.contains("Live updates are temporarily unavailable"));
        assert!(text.contains("manual refresh"));
        assert!(text.contains("Bounded scoped Live will return later"));
    }

    #[test]
    fn all_table_subscribe_command_is_not_available() {
        assert_eq!(
            live_subscription_availability(),
            LiveAvailability::TemporarilyUnavailable
        );
    }
}
