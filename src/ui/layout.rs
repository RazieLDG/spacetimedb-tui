/// Main layout: title bar | [sidebar | content] | status bar.
///
/// Exposes [`render_layout`] which draws the chrome and returns the
/// inner [`ContentAreas`] so that individual tab renderers can fill them.
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Tabs, Widget},
};

use crate::state::{AppState, ConnectionStatus, FocusPanel, Tab};

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const BG_TITLE: Color = Color::Rgb(15, 22, 36);
const BG_TAB_BAR: Color = Color::Rgb(20, 28, 42);
const BG_SELECTED_TAB: Color = Color::Rgb(28, 40, 58);
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_MUTED: Color = Color::Rgb(110, 110, 110);
const SUCCESS: Color = Color::Rgb(152, 195, 121);
const WARNING: Color = Color::Rgb(229, 192, 123);
const ERROR_FG: Color = Color::Rgb(224, 108, 117);
const BORDER_NORMAL: Color = Color::Rgb(40, 50, 65);

// ── Tab definitions ───────────────────────────────────────────────────────────

/// Maps [`Tab`] variants to their display labels (with shortcut hints).
pub fn tab_title(tab: Tab) -> &'static str {
    match tab {
        Tab::Tables => " 1:Tables ",
        Tab::Sql => " 2:SQL ",
        Tab::Logs => " 3:Logs ",
        Tab::Metrics => " 4:Metrics ",
        Tab::Module => " 5:Module ",
        Tab::Live => " 6:Live ",
    }
}

// ── Areas returned to callers ─────────────────────────────────────────────────

/// The sub-areas produced by [`render_layout`] for downstream renderers.
#[derive(Debug, Clone, Copy)]
pub struct ContentAreas {
    pub sidebar: Rect,
    pub content: Rect,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Render the application chrome (title bar, tab bar, sidebar border, status
/// bar) and return the inner content areas for the sidebar and main pane.
///
/// This function does **not** draw tab content or sidebar items — those are
/// handled by the respective sub-modules.
pub fn render_layout(area: Rect, buf: &mut Buffer, app: &AppState) -> ContentAreas {
    // ── Outer vertical split: title | tab_bar | body | status ────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Length(2), // tab bar (including bottom border line)
            Constraint::Min(0),    // body
            Constraint::Length(1), // status bar
        ])
        .split(area);

    let title_area = outer[0];
    let tab_area = outer[1];
    let body_area = outer[2];
    // status_area = outer[3] — drawn by status_bar module

    // ── Title bar ─────────────────────────────────────────────────────────
    render_title_bar(title_area, buf, app);

    // ── Tab bar ───────────────────────────────────────────────────────────
    render_tab_bar(tab_area, buf, app);

    // ── Body: sidebar | content ───────────────────────────────────────────
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(20), // sidebar
            Constraint::Percentage(80), // content
        ])
        .split(body_area);

    let sidebar_area = body[0];
    let content_area = body[1];

    // Draw the sidebar outer border (items rendered by sidebar module)
    let sidebar_focused = app.focus == FocusPanel::Sidebar;
    let sidebar_border = if sidebar_focused {
        ACCENT
    } else {
        BORDER_NORMAL
    };
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(sidebar_border))
        .title(Span::styled(
            " Navigator ",
            Style::default()
                .fg(if sidebar_focused { ACCENT } else { FG_MUTED })
                .add_modifier(Modifier::BOLD),
        ))
        .render(sidebar_area, buf);

    ContentAreas {
        sidebar: sidebar_area,
        content: content_area,
    }
}

// ── Title bar ─────────────────────────────────────────────────────────────────

fn render_title_bar(area: Rect, buf: &mut Buffer, app: &AppState) {
    // Fill background
    for x in area.x..area.x + area.width {
        buf[(x, area.y)]
            .set_char(' ')
            .set_style(Style::default().bg(BG_TITLE));
    }

    // Left: app name + version
    let name = Span::styled(
        " ◈ SpacetimeDB TUI ",
        Style::default()
            .fg(ACCENT)
            .bg(BG_TITLE)
            .add_modifier(Modifier::BOLD),
    );
    let ver = Span::styled("v0.1 ", Style::default().fg(FG_MUTED).bg(BG_TITLE));
    let left = Line::from(vec![name, ver]);
    buf.set_line(area.x, area.y, &left, area.width / 2);

    // Right: connection status + server URL
    let (status_text, status_color) = match &app.connection.status {
        ConnectionStatus::Connected => ("● Connected", SUCCESS),
        ConnectionStatus::Connecting => ("◌ Connecting…", WARNING),
        ConnectionStatus::Disconnected => ("○ Disconnected", FG_MUTED),
        ConnectionStatus::Error(_) => ("✗ Error", ERROR_FG),
    };

    let url_text = format!("  {}  ", app.connection.base_url);
    let url_span = Span::styled(url_text, Style::default().fg(FG_MUTED).bg(BG_TITLE));
    let status_span = Span::styled(
        format!("{status_text} "),
        Style::default()
            .fg(status_color)
            .bg(BG_TITLE)
            .add_modifier(Modifier::BOLD),
    );
    let right_line = Line::from(vec![url_span, status_span]);
    let right_w = right_line.width() as u16;
    let right_x = area.x + area.width.saturating_sub(right_w);
    buf.set_line(right_x, area.y, &right_line, right_w);
}

// ── Tab bar ───────────────────────────────────────────────────────────────────

fn render_tab_bar(area: Rect, buf: &mut Buffer, app: &AppState) {
    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            buf[(x, y)]
                .set_char(' ')
                .set_style(Style::default().bg(BG_TAB_BAR));
        }
    }

    let tabs_titles: Vec<Line> = Tab::ALL
        .iter()
        .map(|t| {
            let title = tab_title(*t);
            if *t == app.current_tab {
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(ACCENT)
                        .bg(BG_SELECTED_TAB)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(
                    title,
                    Style::default().fg(FG_PRIMARY).bg(BG_TAB_BAR),
                ))
            }
        })
        .collect();

    let selected_idx = Tab::ALL
        .iter()
        .position(|t| *t == app.current_tab)
        .unwrap_or(0);

    let tabs_widget = Tabs::new(tabs_titles)
        .select(selected_idx)
        .style(Style::default().bg(BG_TAB_BAR).fg(FG_PRIMARY))
        .highlight_style(
            Style::default()
                .fg(ACCENT)
                .bg(BG_SELECTED_TAB)
                .add_modifier(Modifier::BOLD),
        )
        .divider(Span::styled("│", Style::default().fg(BORDER_NORMAL)));

    tabs_widget.render(area, buf);
}
