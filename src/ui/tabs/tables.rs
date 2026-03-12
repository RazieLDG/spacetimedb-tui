/// Tables tab — browse the rows of the currently selected table.
///
/// Layout:
///   ┌─ Table Browser ──────────────────────────────────────────────────────┐
///   │  [pagination info]                                                    │
///   │  ┌── table_grid ──────────────────────────────────────────────────┐  │
///   │  │  col1 │ col2 │ col3 │ …                                        │  │
///   │  │  ──────┼──────┼──────┼                                         │  │
///   │  │  …     │ …    │ …    │                                         │  │
///   │  └────────────────────────────────────────────────────────────────┘  │
///   └───────────────────────────────────────────────────────────────────────┘
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};

use crate::state::AppState;
use crate::ui::components::table_grid::{render_empty, TableGrid, TableGridState};

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const FG_MUTED: Color = Color::Rgb(110, 110, 110);
const BG_INFO: Color = Color::Rgb(20, 28, 42);
const WARNING: Color = Color::Rgb(229, 192, 123);
const BORDER_FOCUSED: Color = Color::Cyan;
const BORDER_NORMAL: Color = Color::Rgb(40, 50, 65);

// ── Public entry point ────────────────────────────────────────────────────────

/// Render the tables tab into `area`.
pub fn render_tables(
    area: Rect,
    buf: &mut Buffer,
    app: &AppState,
    grid_state: &mut TableGridState,
) {
    // Outer block
    let focused = matches!(app.focus, crate::state::FocusPanel::Main);
    let border_color = if focused { BORDER_FOCUSED } else { BORDER_NORMAL };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " 📋 Table Browser ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }

    // ── Info bar ──────────────────────────────────────────────────────────
    let info_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    let grid_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    render_info_bar(info_area, buf, app);

    // ── Grid ──────────────────────────────────────────────────────────────
    match build_table_data(app) {
        Some((headers, rows, title)) => {
            let widget = TableGrid::new(&headers, &rows)
                .title(title)
                .focused(focused);
            widget.render(grid_area, buf, grid_state);
        }
        None => {
            let msg = if app.selected_database().is_none() {
                "  Select a database from the sidebar"
            } else if app.selected_table().is_none() {
                "  Select a table from the sidebar"
            } else if app.query_loading {
                "  Loading table data…"
            } else {
                "  No data — press r to refresh"
            };
            render_empty(grid_area, buf, msg, focused);
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn render_info_bar(area: Rect, buf: &mut Buffer, app: &AppState) {
    // Fill background
    for x in area.x..area.x + area.width {
        buf[(x, area.y)]
            .set_char(' ')
            .set_style(Style::default().bg(BG_INFO));
    }

    let mut spans: Vec<Span> = Vec::new();

    if let Some(db) = app.selected_database() {
        spans.push(Span::styled(
            format!(" 🗄 {db}"),
            Style::default().fg(ACCENT),
        ));
    }
    if let Some(tbl) = app.selected_table() {
        spans.push(Span::styled(
            format!("  ›  {}", tbl.table_name),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!("  ({} columns)", tbl.columns.len()),
            Style::default().fg(FG_MUTED),
        ));
    }

    if let Some(ref qr) = app.query_result {
        spans.push(Span::styled(
            format!("  {} rows", qr.row_count()),
            Style::default().fg(FG_MUTED),
        ));
    }

    if app.query_loading {
        spans.push(Span::styled(
            "  ⟳ loading…",
            Style::default().fg(WARNING).add_modifier(Modifier::BOLD),
        ));
    }

    // Right-align hint
    let hint = Span::styled(
        " r:refresh  n:next  p:prev ",
        Style::default().fg(FG_MUTED),
    );
    let hint_w = hint.content.len() as u16;
    let hint_x = area.x + area.width.saturating_sub(hint_w);

    let line = Line::from(spans);
    buf.set_line(area.x, area.y, &line, area.width.saturating_sub(hint_w));
    buf.set_line(hint_x, area.y, &Line::from(hint), hint_w);
}

/// Build (headers, rows, title) from the current query result or table cache.
fn build_table_data(app: &AppState) -> Option<(Vec<String>, Vec<Vec<String>>, String)> {
    // Prefer the live query result if it's for the current table
    let qr = app.query_result.as_ref().or_else(|| {
        // Try cache
        let db = app.selected_database()?;
        let tbl = app.selected_table()?;
        let key = AppState::cache_key(db, &tbl.table_name);
        app.table_cache.get(&key).map(|c| &c.result)
    })?;

    if qr.schema.is_empty() {
        return None;
    }

    let headers: Vec<String> = qr.column_names().iter().map(|s| s.to_string()).collect();
    let rows: Vec<Vec<String>> = qr
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(value_to_display)
                .collect()
        })
        .collect();

    let title = app
        .selected_table()
        .map(|t| t.table_name.clone())
        .unwrap_or_else(|| "Results".to_string());

    Some((headers, rows, title))
}

/// Convert a `serde_json::Value` to a compact display string.
pub fn value_to_display(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => {
            let items: Vec<String> = a.iter().map(value_to_display).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(o) => {
            let pairs: Vec<String> = o
                .iter()
                .map(|(k, v)| format!("{k}:{}", value_to_display(v)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
    }
}
