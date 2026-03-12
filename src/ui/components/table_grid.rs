/// Reusable data-table widget.
///
/// Renders a bordered table with:
/// - Column headers (bold, cyan)
/// - Auto-sized column widths based on content (using unicode-width)
/// - Row highlighting for the selected row
/// - Horizontal + vertical scroll offsets
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;

// ── Theme constants ───────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const HEADER_BG: Color = Color::Rgb(28, 40, 58);
const SELECTED_BG: Color = Color::Rgb(44, 62, 80);
const BORDER_FOCUSED: Color = Color::Cyan;
const BORDER_NORMAL: Color = Color::Rgb(60, 60, 60);
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_MUTED: Color = Color::Rgb(120, 120, 120);

// ── Public state ─────────────────────────────────────────────────────────────

/// Mutable state for a [`TableGrid`] widget.
#[derive(Debug, Clone, Default)]
pub struct TableGridState {
    /// Index of the currently selected row (0-based).
    pub selected_row: usize,
    /// First visible column index (horizontal scroll).
    pub scroll_col: usize,
    /// First visible row index (vertical scroll).
    pub scroll_row: usize,
}

impl TableGridState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Move selection down by one row, clamped to `max_row`.
    pub fn next_row(&mut self, max_row: usize) {
        if max_row == 0 {
            return;
        }
        self.selected_row = (self.selected_row + 1).min(max_row.saturating_sub(1));
    }

    /// Move selection up by one row, clamped to 0.
    pub fn prev_row(&mut self) {
        self.selected_row = self.selected_row.saturating_sub(1);
    }

    /// Scroll right by one column.
    ///
    /// `col_count` is the total number of columns in the current data set.
    pub fn scroll_right(&mut self, col_count: usize) {
        if col_count > 0 {
            self.scroll_col = (self.scroll_col + 1).min(col_count.saturating_sub(1));
        }
    }

    /// Scroll left by one column.
    pub fn scroll_left(&mut self) {
        self.scroll_col = self.scroll_col.saturating_sub(1);
    }

    /// Ensure the selected row is visible within `visible_height` rows.
    pub fn ensure_visible(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        if self.selected_row < self.scroll_row {
            self.scroll_row = self.selected_row;
        } else if self.selected_row >= self.scroll_row + visible_height {
            self.scroll_row = self.selected_row - visible_height + 1;
        }
    }
}

// ── Widget ────────────────────────────────────────────────────────────────────

/// A stateful table widget that renders column headers and data rows.
pub struct TableGrid<'a> {
    /// Column header labels.
    headers: &'a [String],
    /// Data rows; each inner slice must have the same length as `headers`.
    rows: &'a [Vec<String>],
    /// Optional title shown in the block border.
    title: Option<String>,
    /// Whether this widget currently owns keyboard focus.
    focused: bool,
    /// Maximum column width cap (default: 40).
    max_col_width: usize,
}

impl<'a> TableGrid<'a> {
    pub fn new(headers: &'a [String], rows: &'a [Vec<String>]) -> Self {
        Self {
            headers,
            rows,
            title: None,
            focused: false,
            max_col_width: 40,
        }
    }

    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = Some(t.into());
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }

    pub fn max_col_width(mut self, w: usize) -> Self {
        self.max_col_width = w;
        self
    }

    // ── Internal helpers ────────────────────────────────────────────────────

    /// Compute display widths for every column, capped at `max_col_width`.
    fn column_widths(&self) -> Vec<usize> {
        let n = self.headers.len();
        if n == 0 {
            return vec![];
        }
        let mut widths: Vec<usize> = self
            .headers
            .iter()
            .map(|h| h.width().max(4))
            .collect();

        for row in self.rows {
            for (i, cell) in row.iter().enumerate() {
                if i < n {
                    let w = cell.width().min(self.max_col_width);
                    if w > widths[i] {
                        widths[i] = w;
                    }
                }
            }
        }
        // Enforce cap
        for w in &mut widths {
            *w = (*w).min(self.max_col_width);
        }
        widths
    }

    /// Render a single cell, truncating with '…' if it exceeds `width`.
    fn render_cell(text: &str, width: usize) -> String {
        let display_w = text.width();
        if display_w <= width {
            // Pad with spaces
            let pad = width - display_w;
            format!("{}{}", text, " ".repeat(pad))
        } else {
            // Truncate; reserve 1 char for '…'
            let mut out = String::new();
            let mut used = 0usize;
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                if used + cw + 1 > width {
                    break;
                }
                out.push(ch);
                used += cw;
            }
            out.push('…');
            used += 1;
            // Pad remainder
            if used < width {
                out.push_str(&" ".repeat(width - used));
            }
            out
        }
    }
}

impl<'a> StatefulWidget for TableGrid<'a> {
    type State = TableGridState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut TableGridState) {
        let border_color = if self.focused {
            BORDER_FOCUSED
        } else {
            BORDER_NORMAL
        };
        let block = {
            let mut b = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));
            if let Some(ref t) = self.title {
                b = b.title(Span::styled(
                    format!(" {t} "),
                    Style::default()
                        .fg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            b
        };

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let col_widths = self.column_widths();
        let n_cols = col_widths.len();

        if n_cols == 0 {
            // Nothing to render
            let msg = Line::from(Span::styled(
                "  (no columns)",
                Style::default().fg(FG_MUTED),
            ));
            buf.set_line(inner.x, inner.y, &msg, inner.width);
            return;
        }

        // Determine which columns are visible given horizontal scroll
        let visible_cols = {
            let mut cols: Vec<usize> = Vec::new();
            let mut x_used = 0u16;
            for (ci, &w_raw) in col_widths.iter().enumerate().skip(state.scroll_col) {
                let w = w_raw as u16 + 1; // +1 for separator
                if x_used + w > inner.width {
                    break;
                }
                cols.push(ci);
                x_used += w;
            }
            cols
        };

        if visible_cols.is_empty() {
            return;
        }

        // ── Header row ────────────────────────────────────────────────────
        let header_y = inner.y;
        {
            let mut x = inner.x;
            for &ci in &visible_cols {
                let w = col_widths[ci];
                let cell = Self::render_cell(&self.headers[ci], w);
                let span = Span::styled(
                    cell,
                    Style::default()
                        .fg(ACCENT)
                        .bg(HEADER_BG)
                        .add_modifier(Modifier::BOLD),
                );
                let line = Line::from(span);
                buf.set_line(x, header_y, &line, w as u16);
                x += w as u16;
                // Column separator
                if x < inner.x + inner.width {
                    buf[(x, header_y)]
                        .set_char('│')
                        .set_style(Style::default().fg(BORDER_NORMAL).bg(HEADER_BG));
                    x += 1;
                }
            }
            // Fill rest of header row
            while x < inner.x + inner.width {
                buf[(x, header_y)]
                    .set_char(' ')
                    .set_style(Style::default().bg(HEADER_BG));
                x += 1;
            }
        }

        // Separator line under headers
        let sep_y = inner.y + 1;
        if sep_y < inner.y + inner.height {
            let mut x = inner.x;
            for &ci in &visible_cols {
                let w = col_widths[ci] as u16;
                for _ in 0..w {
                    buf[(x, sep_y)]
                        .set_char('─')
                        .set_style(Style::default().fg(BORDER_NORMAL));
                    x += 1;
                }
                if x < inner.x + inner.width {
                    buf[(x, sep_y)]
                        .set_char('┼')
                        .set_style(Style::default().fg(BORDER_NORMAL));
                    x += 1;
                }
            }
            // Fill remainder
            while x < inner.x + inner.width {
                buf[(x, sep_y)]
                    .set_char('─')
                    .set_style(Style::default().fg(BORDER_NORMAL));
                x += 1;
            }
        }

        // ── Data rows ─────────────────────────────────────────────────────
        let data_start_y = inner.y + 2; // after header + separator
        let available_rows = inner.height.saturating_sub(2) as usize;

        // Ensure scroll so selected row is visible
        state.ensure_visible(available_rows);

        for (ri, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(state.scroll_row)
            .take(available_rows)
        {
            let screen_y = data_start_y + (ri - state.scroll_row) as u16;
            if screen_y >= inner.y + inner.height {
                break;
            }

            let is_selected = ri == state.selected_row;
            let row_bg = if is_selected {
                SELECTED_BG
            } else if ri % 2 == 0 {
                Color::Reset
            } else {
                Color::Rgb(22, 22, 24)
            };
            let row_fg = if is_selected {
                Color::White
            } else {
                FG_PRIMARY
            };

            let mut x = inner.x;
            for &ci in &visible_cols {
                let w = col_widths[ci];
                let cell_text = row.get(ci).map(|s| s.as_str()).unwrap_or("");
                let cell = Self::render_cell(cell_text, w);
                let style = Style::default().fg(row_fg).bg(row_bg);
                let span = if is_selected {
                    Span::styled(cell, style.add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(cell, style)
                };
                let line = Line::from(span);
                buf.set_line(x, screen_y, &line, w as u16);
                x += w as u16;
                // Separator
                if x < inner.x + inner.width {
                    buf[(x, screen_y)]
                        .set_char('│')
                        .set_style(Style::default().fg(BORDER_NORMAL).bg(row_bg));
                    x += 1;
                }
            }
            // Fill row remainder
            while x < inner.x + inner.width {
                buf[(x, screen_y)]
                    .set_char(' ')
                    .set_style(Style::default().bg(row_bg));
                x += 1;
            }
        }

        // ── Scroll indicators ─────────────────────────────────────────────
        if self.rows.len() > available_rows && available_rows > 0 {
            let total = self.rows.len();
            let shown = available_rows.min(total);
            let pct = state.scroll_row * 100 / total.max(1);
            let indicator = format!(" {}/{} ({}%) ", state.scroll_row + shown, total, pct);
            let ind_x = inner.x + inner.width.saturating_sub(indicator.len() as u16);
            let ind_y = inner.y + inner.height - 1;
            if ind_y < inner.y + inner.height {
                let line = Line::from(Span::styled(
                    indicator,
                    Style::default().fg(FG_MUTED),
                ));
                buf.set_line(ind_x, ind_y, &line, inner.width);
            }
        }
    }
}

// ── Empty-state helper ────────────────────────────────────────────────────────

/// Render a placeholder message inside `area` when there is no data.
pub fn render_empty(area: Rect, buf: &mut Buffer, msg: &str, focused: bool) {
    let border_color = if focused { BORDER_FOCUSED } else { BORDER_NORMAL };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height == 0 {
        return;
    }
    let y = inner.y + inner.height / 2;
    let x_pad = (inner.width as usize).saturating_sub(msg.len()) / 2;
    let line = Line::from(Span::styled(msg, Style::default().fg(FG_MUTED)));
    buf.set_line(inner.x + x_pad as u16, y, &line, inner.width);
}
