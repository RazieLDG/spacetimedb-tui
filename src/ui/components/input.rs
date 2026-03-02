/// Single-line text input widget with visible cursor.
///
/// The widget renders inside a bordered block and shows a blinking-style
/// cursor at the current byte position.  All mutations happen via the
/// [`InputState`] which can be embedded in [`AppState`].
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};
use unicode_width::UnicodeWidthStr;

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const BORDER_FOCUSED: Color = Color::Cyan;
const BORDER_NORMAL: Color = Color::Rgb(60, 60, 60);
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_MUTED: Color = Color::Rgb(120, 120, 120);
const CURSOR_BG: Color = Color::Cyan;
const CURSOR_FG: Color = Color::Black;

// ── State ─────────────────────────────────────────────────────────────────────

/// Mutable state for the text input widget.
/// Mirrors the relevant fields already present in `AppState.sql_input` /
/// `AppState.sql_cursor` but packaged as a standalone struct so components
/// can own their own input state if needed.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// The current text buffer.
    pub value: String,
    /// Byte-offset of the insertion cursor inside `value`.
    pub cursor: usize,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a character at the current cursor position.
    pub fn insert(&mut self, ch: char) {
        self.value.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    /// Delete the character immediately before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Walk backwards to find the previous char boundary.
        let mut bc = self.cursor - 1;
        while bc > 0 && !self.value.is_char_boundary(bc) {
            bc -= 1;
        }
        self.value.remove(bc);
        self.cursor = bc;
    }

    /// Delete the character at the cursor position (delete key).
    pub fn delete(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        self.value.remove(self.cursor);
    }

    /// Move cursor one character to the left.
    pub fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let mut bc = self.cursor - 1;
        while bc > 0 && !self.value.is_char_boundary(bc) {
            bc -= 1;
        }
        self.cursor = bc;
    }

    /// Move cursor one character to the right.
    pub fn move_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        let mut bc = self.cursor + 1;
        while bc < self.value.len() && !self.value.is_char_boundary(bc) {
            bc += 1;
        }
        self.cursor = bc;
    }

    /// Move cursor to the beginning.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to the end.
    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }

    /// Clear the entire buffer.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Replace the buffer with `text` and move cursor to end.
    pub fn set(&mut self, text: impl Into<String>) {
        self.value = text.into();
        self.cursor = self.value.len();
    }

    /// Return the current text as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Delete from cursor to end of line (Ctrl+K).
    pub fn kill_to_end(&mut self) {
        self.value.truncate(self.cursor);
    }

    /// Delete from start to cursor (Ctrl+U).
    pub fn kill_to_start(&mut self) {
        self.value.drain(..self.cursor);
        self.cursor = 0;
    }
}

// ── Widget ─────────────────────────────────────────────────────────────────────

/// A text-input widget backed by [`InputState`].
pub struct InputWidget<'a> {
    state: &'a InputState,
    title: Option<&'a str>,
    placeholder: Option<&'a str>,
    focused: bool,
}

impl<'a> InputWidget<'a> {
    pub fn new(state: &'a InputState) -> Self {
        Self {
            state,
            title: None,
            placeholder: None,
            focused: false,
        }
    }

    pub fn title(mut self, t: &'a str) -> Self {
        self.title = Some(t);
        self
    }

    pub fn placeholder(mut self, p: &'a str) -> Self {
        self.placeholder = Some(p);
        self
    }

    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
}

impl<'a> Widget for InputWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_color = if self.focused {
            BORDER_FOCUSED
        } else {
            BORDER_NORMAL
        };

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        if let Some(t) = self.title {
            block = block.title(Span::styled(
                format!(" {t} "),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
        }

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let text = self.state.as_str();
        let cursor_pos = self.state.cursor;
        let available_w = inner.width as usize;

        // Compute display offset so cursor is always visible.
        // We scroll horizontally if the text is longer than the widget.
        let text_before_cursor = &text[..cursor_pos];
        let display_cursor_x = text_before_cursor.width();

        let scroll_x = if display_cursor_x >= available_w {
            display_cursor_x - available_w + 1
        } else {
            0
        };

        // Build the visible slice of text
        let mut spans: Vec<Span> = Vec::new();

        if text.is_empty() {
            if let Some(ph) = self.placeholder {
                spans.push(Span::styled(ph, Style::default().fg(FG_MUTED)));
            }
        } else {
            // Simpler approach: render char by char with display positions
            let mut x_pos = 0usize; // display position
            let mut byte_pos = 0usize;
            let mut before_str = String::new();
            let mut cursor_char = ' ';
            let mut after_str = String::new();
            let mut phase = 0u8; // 0=before, 1=cursor, 2=after

            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
                let display_x = x_pos.saturating_sub(scroll_x);

                if display_x >= available_w {
                    break;
                }

                if x_pos >= scroll_x {
                    match phase {
                        0 if byte_pos < cursor_pos => {
                            before_str.push(ch);
                        }
                        0 | 1 if byte_pos == cursor_pos => {
                            cursor_char = ch;
                            phase = 1;
                        }
                        _ => {
                            if phase == 1 {
                                phase = 2;
                            }
                            after_str.push(ch);
                        }
                    }
                }

                x_pos += cw;
                byte_pos += ch.len_utf8();
            }

            // Handle cursor at end of text
            if byte_pos == cursor_pos && phase == 0 {
                cursor_char = ' ';
                phase = 1;
            }

            if !before_str.is_empty() {
                spans.push(Span::styled(
                    before_str,
                    Style::default().fg(FG_PRIMARY),
                ));
            }

            if self.focused {
                spans.push(Span::styled(
                    cursor_char.to_string(),
                    Style::default()
                        .fg(CURSOR_FG)
                        .bg(CURSOR_BG)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                // No cursor when not focused — just show the char normally
                if cursor_char != ' ' || !after_str.is_empty() {
                    spans.push(Span::styled(
                        cursor_char.to_string(),
                        Style::default().fg(FG_PRIMARY),
                    ));
                }
            }

            if !after_str.is_empty() {
                spans.push(Span::styled(
                    after_str,
                    Style::default().fg(FG_PRIMARY),
                ));
            }
        }

        let line = Line::from(spans);
        buf.set_line(inner.x, inner.y, &line, inner.width);
    }
}
