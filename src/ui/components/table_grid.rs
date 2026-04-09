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
    /// Index of the currently selected row (0-based, in display order).
    pub selected_row: usize,
    /// Index of the currently selected column (0-based). Used for
    /// cell-level clipboard copy and highlighting.
    pub selected_col: usize,
    /// First visible column index (horizontal scroll).
    pub scroll_col: usize,
    /// First visible row index (vertical scroll).
    pub scroll_row: usize,
    /// Column index to sort by, if any. `None` means rows are shown in
    /// their original order.
    pub sort_col: Option<usize>,
    /// Whether the sort is descending (true) or ascending (false).
    pub sort_desc: bool,
}

impl TableGridState {
    /// Toggle the sort state on a column: off → ascending → descending
    /// → off. Used by the `s` key binding so the user can step through
    /// the three states without memorising separate keys.
    pub fn cycle_sort(&mut self, col: usize) {
        match (self.sort_col, self.sort_desc) {
            (Some(c), false) if c == col => {
                // asc → desc
                self.sort_desc = true;
            }
            (Some(c), true) if c == col => {
                // desc → off
                self.sort_col = None;
                self.sort_desc = false;
            }
            _ => {
                // any other state → asc on this column
                self.sort_col = Some(col);
                self.sort_desc = false;
            }
        }
    }

}

/// Compute the original-data index for a given display-order row when
/// the grid is sorted by `sort_col` (ascending unless `sort_desc`).
///
/// This is a free function rather than a method on `TableGridState`
/// because it needs access to the raw cell strings to re-derive the
/// permutation, and we don't want to drag a whole `Vec<Vec<String>>`
/// into the state struct.
pub fn sorted_data_index(
    rows: &[Vec<String>],
    sort_col: Option<usize>,
    sort_desc: bool,
    display_row: usize,
) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    let display = display_row.min(rows.len() - 1);
    match sort_col {
        None => Some(display),
        Some(col) => {
            let mut indices: Vec<usize> = (0..rows.len()).collect();
            indices.sort_by(|&a, &b| {
                let av = rows[a].get(col).map(String::as_str).unwrap_or("");
                let bv = rows[b].get(col).map(String::as_str).unwrap_or("");
                compare_cells(av, bv)
            });
            if sort_desc {
                indices.reverse();
            }
            indices.get(display).copied()
        }
    }
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

    /// Move the cell cursor one column to the right (clamped to
    /// `col_count`). Also nudges horizontal scroll if we walked off the
    /// right edge of the visible window.
    pub fn next_col(&mut self, col_count: usize) {
        if col_count == 0 {
            return;
        }
        self.selected_col = (self.selected_col + 1).min(col_count - 1);
        if self.selected_col > self.scroll_col + 8 {
            self.scroll_col = self.selected_col.saturating_sub(8);
        }
    }

    /// Move the cell cursor one column to the left.
    pub fn prev_col(&mut self) {
        self.selected_col = self.selected_col.saturating_sub(1);
        if self.selected_col < self.scroll_col {
            self.scroll_col = self.selected_col;
        }
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
    /// Case-insensitive substring to highlight in cell content. Rows
    /// that contain the query are tinted with a yellow background so
    /// search results are easy to spot at a glance.
    highlight_query: Option<&'a str>,
    /// Per-cell pending edits keyed by `(data_row_idx, col_idx)` →
    /// new value string. Rendered in place of the original cell
    /// text with an amber background so the user can see which
    /// edits haven't been flushed yet.
    pending_edits: &'a [(usize, usize, String)],
    /// When `Some`, an inline cell editor is open over
    /// `(data_row_idx, col_idx)`. The cell is painted as an input
    /// widget instead of plain text.
    active_editor: Option<(usize, usize, &'a str, usize)>,
}

impl<'a> TableGrid<'a> {
    pub fn new(headers: &'a [String], rows: &'a [Vec<String>]) -> Self {
        Self {
            headers,
            rows,
            title: None,
            focused: false,
            max_col_width: 40,
            highlight_query: None,
            pending_edits: &[],
            active_editor: None,
        }
    }

    /// Overlay a list of pending edits on the grid. Each entry is
    /// `(data_row_idx, col_idx, new_value)`; the original cell text
    /// is replaced during render and drawn with an amber tint.
    pub fn pending_edits(mut self, edits: &'a [(usize, usize, String)]) -> Self {
        self.pending_edits = edits;
        self
    }

    /// Draw `(data_row_idx, col_idx)` as an inline text input with
    /// `value` as its content and `cursor` as the byte-cursor
    /// position. Used by spreadsheet edit mode.
    pub fn active_editor(
        mut self,
        data_row: usize,
        col: usize,
        value: &'a str,
        cursor: usize,
    ) -> Self {
        self.active_editor = Some((data_row, col, value, cursor));
        self
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

    /// Tint rows that contain `query` (case-insensitive) with a
    /// distinctive background colour. Passing `None` or an empty
    /// string disables highlighting.
    pub fn highlight_query(mut self, query: Option<&'a str>) -> Self {
        self.highlight_query = query.filter(|q| !q.is_empty());
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
                // Append a sort-direction indicator when this column is
                // the active sort key, so the user knows which column
                // drove the current order.
                let header_text = match state.sort_col {
                    Some(c) if c == ci => {
                        let arrow = if state.sort_desc { " ↓" } else { " ↑" };
                        format!("{}{}", self.headers[ci], arrow)
                    }
                    _ => self.headers[ci].clone(),
                };
                let cell = Self::render_cell(&header_text, w);
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

        // Compute a display-order permutation from the sort state. When
        // no sort is active this is just 0..n and walking it is a no-op.
        // When a sort *is* active, `selected_row` indexes into this
        // permutation, not into `self.rows` directly — the rest of the
        // loop reads `self.rows[order[ri]]` to fetch the actual cells.
        let row_order: Vec<usize> = match state.sort_col {
            Some(col) if col < self.headers.len() && !self.rows.is_empty() => {
                let mut indices: Vec<usize> = (0..self.rows.len()).collect();
                indices.sort_by(|&a, &b| {
                    let av = self.rows[a].get(col).map(String::as_str).unwrap_or("");
                    let bv = self.rows[b].get(col).map(String::as_str).unwrap_or("");
                    compare_cells(av, bv)
                });
                if state.sort_desc {
                    indices.reverse();
                }
                indices
            }
            _ => (0..self.rows.len()).collect(),
        };

        // Ensure scroll so selected row is visible
        state.ensure_visible(available_rows);

        for display_idx in state.scroll_row..(state.scroll_row + available_rows).min(row_order.len()) {
            let ri = row_order[display_idx];
            let row = &self.rows[ri];
            let screen_y = data_start_y + (display_idx - state.scroll_row) as u16;
            if screen_y >= inner.y + inner.height {
                break;
            }

            let is_selected = display_idx == state.selected_row;

            // Check whether the row contains the active search query.
            // Matched rows get a subtle amber tint so the user can spot
            // them at a glance, even without wrapping via n/N.
            let is_match = self
                .highlight_query
                .map(|q| {
                    let q_lower = q.to_ascii_lowercase();
                    row.iter()
                        .any(|cell| cell.to_ascii_lowercase().contains(&q_lower))
                })
                .unwrap_or(false);

            let row_bg = if is_selected {
                SELECTED_BG
            } else if is_match {
                Color::Rgb(60, 50, 20) // subtle amber for search matches
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
                // Pending edit lookup: data row index `ri` + column `ci`.
                let pending_value: Option<&str> = self
                    .pending_edits
                    .iter()
                    .find(|(r, c, _)| *r == ri && *c == ci)
                    .map(|(_, _, v)| v.as_str());

                // Active inline editor takes precedence over pending
                // display — the user is typing into this exact cell.
                let editor_here: Option<(&str, usize)> = self
                    .active_editor
                    .filter(|(r, c, _, _)| *r == ri && *c == ci)
                    .map(|(_, _, v, cur)| (v, cur));

                let cell_text: String = if let Some((val, _)) = editor_here {
                    val.to_string()
                } else if let Some(v) = pending_value {
                    v.to_string()
                } else {
                    row.get(ci).map(|s| s.to_string()).unwrap_or_default()
                };
                let cell = Self::render_cell(&cell_text, w);

                // The "active cell" is the intersection of the selected
                // row and selected column — it gets an extra-bright
                // background so Ctrl+C targets are visible at a glance.
                let is_cell_cursor = is_selected && ci == state.selected_col;
                let cell_bg = if editor_here.is_some() {
                    Color::Rgb(90, 80, 30) // bright amber while typing
                } else if pending_value.is_some() {
                    Color::Rgb(70, 55, 15) // muted amber for pending
                } else if is_cell_cursor {
                    Color::Rgb(72, 100, 130)
                } else {
                    row_bg
                };
                let cell_fg = if pending_value.is_some() || editor_here.is_some() {
                    Color::Rgb(255, 220, 100) // yellow text for edits
                } else {
                    row_fg
                };
                let style = Style::default().fg(cell_fg).bg(cell_bg);
                let span = if is_selected {
                    Span::styled(cell, style.add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(cell, style)
                };
                let line = Line::from(span);
                buf.set_line(x, screen_y, &line, w as u16);

                // If this is the active inline editor, overlay a
                // block cursor on top of the rendered cell text so
                // the user knows where typing will land.
                if let Some((val, cursor)) = editor_here {
                    let before = &val[..cursor.min(val.len())];
                    let display_col = unicode_width::UnicodeWidthStr::width(before) as u16;
                    if (display_col as usize) < w {
                        let cur_x = x + display_col;
                        let cur_ch = val[cursor.min(val.len())..]
                            .chars()
                            .next()
                            .unwrap_or(' ');
                        buf[(cur_x, screen_y)]
                            .set_char(cur_ch)
                            .set_style(
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(Color::Rgb(255, 220, 100))
                                    .add_modifier(Modifier::BOLD),
                            );
                    }
                }
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

/// Three-way comparison that tries a numeric ordering first (so that
/// `"2" < "10"`) and falls back to case-insensitive lexicographic
/// comparison when one or both sides aren't numbers. Used by the sort
/// helper in the grid renderer.
fn compare_cells(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
        _ => a
            .to_ascii_lowercase()
            .cmp(&b.to_ascii_lowercase()),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_sort_off_asc_desc_off() {
        let mut s = TableGridState::new();
        assert_eq!(s.sort_col, None);

        s.cycle_sort(2);
        assert_eq!(s.sort_col, Some(2));
        assert!(!s.sort_desc);

        s.cycle_sort(2);
        assert_eq!(s.sort_col, Some(2));
        assert!(s.sort_desc);

        s.cycle_sort(2);
        assert_eq!(s.sort_col, None);
        assert!(!s.sort_desc);
    }

    #[test]
    fn cycle_sort_switching_columns_resets_direction() {
        let mut s = TableGridState::new();
        s.cycle_sort(0);
        s.cycle_sort(0); // asc → desc
        s.cycle_sort(3); // switch to a new column → asc
        assert_eq!(s.sort_col, Some(3));
        assert!(!s.sort_desc);
    }

    #[test]
    fn sorted_data_index_ascending_numeric() {
        let rows = vec![
            vec!["10".to_string(), "b".to_string()],
            vec!["2".to_string(), "a".to_string()],
            vec!["30".to_string(), "c".to_string()],
        ];
        // Ascending numeric order: 2, 10, 30 → original indices 1, 0, 2.
        assert_eq!(sorted_data_index(&rows, Some(0), false, 0), Some(1));
        assert_eq!(sorted_data_index(&rows, Some(0), false, 1), Some(0));
        assert_eq!(sorted_data_index(&rows, Some(0), false, 2), Some(2));
    }

    #[test]
    fn sorted_data_index_descending() {
        let rows = vec![
            vec!["a".to_string()],
            vec!["c".to_string()],
            vec!["b".to_string()],
        ];
        // Descending lex: c, b, a → original indices 1, 2, 0.
        assert_eq!(sorted_data_index(&rows, Some(0), true, 0), Some(1));
        assert_eq!(sorted_data_index(&rows, Some(0), true, 1), Some(2));
        assert_eq!(sorted_data_index(&rows, Some(0), true, 2), Some(0));
    }

    #[test]
    fn sorted_data_index_no_sort_is_identity() {
        let rows = vec![vec!["x".to_string()], vec!["y".to_string()]];
        assert_eq!(sorted_data_index(&rows, None, false, 0), Some(0));
        assert_eq!(sorted_data_index(&rows, None, false, 1), Some(1));
    }

    #[test]
    fn compare_cells_numeric_beats_lex() {
        // Lex order would put "10" before "2"; numeric order is the
        // other way round. We want numeric.
        assert_eq!(
            compare_cells("2", "10"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn compare_cells_falls_back_to_lex_when_non_numeric() {
        assert_eq!(
            compare_cells("alice", "Bob"),
            std::cmp::Ordering::Less
        );
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
