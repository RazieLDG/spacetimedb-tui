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

    /// Return the byte range of the "word token" that the cursor is
    /// currently inside of or immediately after, together with the word
    /// text itself. A word here is a maximal run of ASCII alphanumerics
    /// or `_`, matching the characters that can appear in SQL
    /// identifiers and keywords.
    ///
    /// Used by the Tab-complete machinery in `app.rs` to figure out
    /// which prefix the user is trying to finish.
    pub fn current_word(&self) -> (std::ops::Range<usize>, &str) {
        let bytes = self.value.as_bytes();
        let end = self.cursor.min(bytes.len());
        let mut start = end;
        while start > 0 {
            let b = bytes[start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        (start..end, &self.value[start..end])
    }

    /// Replace the byte `range` with `replacement` and park the cursor
    /// at the end of the newly-inserted text. Used to commit an
    /// auto-completion result in place of a prefix token.
    pub fn replace_range(&mut self, range: std::ops::Range<usize>, replacement: &str) {
        self.value.replace_range(range.clone(), replacement);
        self.cursor = range.start + replacement.len();
    }
}

// ── Widget ─────────────────────────────────────────────────────────────────────

/// A text-input widget backed by [`InputState`].
pub struct InputWidget<'a> {
    state: &'a InputState,
    title: Option<&'a str>,
    placeholder: Option<&'a str>,
    focused: bool,
    highlight_sql: bool,
}

impl<'a> InputWidget<'a> {
    pub fn new(state: &'a InputState) -> Self {
        Self {
            state,
            title: None,
            placeholder: None,
            focused: false,
            highlight_sql: false,
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

    /// Enable SQL syntax highlighting: keywords, identifiers, strings,
    /// numbers and punctuation each get their own colour via the
    /// [`super::syntax`] tokenizer. The cursor is painted on top.
    pub fn highlight_sql(mut self, enabled: bool) -> Self {
        self.highlight_sql = enabled;
        self
    }
}

/// Pick a foreground colour for a given token kind using the same
/// "One Dark"-ish palette as the rest of the UI.
fn token_color(kind: super::syntax::TokenKind) -> Color {
    use super::syntax::TokenKind;
    match kind {
        TokenKind::Keyword => Color::Rgb(198, 120, 221), // purple
        TokenKind::Identifier => Color::Rgb(220, 220, 220),
        TokenKind::StringLit => Color::Rgb(152, 195, 121), // green
        TokenKind::Number => Color::Rgb(229, 192, 123),    // yellow
        TokenKind::Punct => Color::Rgb(86, 182, 194),      // cyan
        TokenKind::Whitespace => Color::Rgb(220, 220, 220),
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

        // Empty input → render the placeholder (if any) and bail out.
        if text.is_empty() {
            if let Some(ph) = self.placeholder {
                let line = Line::from(Span::styled(ph, Style::default().fg(FG_MUTED)));
                buf.set_line(inner.x, inner.y, &line, inner.width);
            }
            // When focused but empty, still draw the cursor at col 0.
            if self.focused {
                buf[(inner.x, inner.y)].set_char(' ').set_style(
                    Style::default()
                        .fg(CURSOR_FG)
                        .bg(CURSOR_BG)
                        .add_modifier(Modifier::BOLD),
                );
            }
            return;
        }

        // Horizontal scroll so the cursor stays visible.
        let text_before_cursor = &text[..cursor_pos];
        let display_cursor_x = text_before_cursor.width();
        let scroll_x = if display_cursor_x >= available_w {
            display_cursor_x - available_w + 1
        } else {
            0
        };

        // Build a per-byte colour map so we can colour each char with
        // either its syntax token colour (when highlight_sql is on) or
        // a flat foreground.
        let mut byte_color: Vec<Color> = vec![FG_PRIMARY; text.len()];
        if self.highlight_sql {
            for tok in super::syntax::tokenize(text) {
                let colour = token_color(tok.kind);
                for i in tok.range {
                    if i < byte_color.len() {
                        byte_color[i] = colour;
                    }
                }
            }
        }

        // Walk the characters, placing each one into the buffer at the
        // correct display column. Characters scrolled off to the left
        // are skipped; characters that run past the right edge are
        // truncated.
        let mut x_pos: usize = 0; // display column relative to text start
        let mut byte_pos: usize = 0;
        let mut cursor_drawn = false;

        for ch in text.chars() {
            let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
            let screen_col = x_pos as isize - scroll_x as isize;

            // Paint this character if it's on-screen.
            if screen_col >= 0 && (screen_col as usize) < available_w {
                let cell_x = inner.x + screen_col as u16;
                let is_cursor = self.focused && byte_pos == cursor_pos;
                let style = if is_cursor {
                    cursor_drawn = true;
                    Style::default()
                        .fg(CURSOR_FG)
                        .bg(CURSOR_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(byte_color[byte_pos])
                };
                buf[(cell_x, inner.y)].set_char(ch).set_style(style);
                // For double-width characters, clear the second cell so
                // we don't leave stale content behind.
                if cw == 2 && (screen_col as usize + 1) < available_w {
                    buf[(cell_x + 1, inner.y)]
                        .set_char(' ')
                        .set_style(Style::default());
                }
            }

            x_pos += cw;
            byte_pos += ch.len_utf8();
        }

        // Draw the cursor at end-of-text if we haven't already drawn it.
        if self.focused && !cursor_drawn && cursor_pos == text.len() {
            let screen_col = x_pos as isize - scroll_x as isize;
            if screen_col >= 0 && (screen_col as usize) < available_w {
                let cell_x = inner.x + screen_col as u16;
                buf[(cell_x, inner.y)].set_char(' ').set_style(
                    Style::default()
                        .fg(CURSOR_FG)
                        .bg(CURSOR_BG)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}
