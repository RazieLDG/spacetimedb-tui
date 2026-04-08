/// Help overlay popup showing all key bindings.
///
/// Renders as a centred modal box with a scrollable list of bindings
/// grouped by category.
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

// ── Theme ─────────────────────────────────────────────────────────────────────
const ACCENT: Color = Color::Cyan;
const BG: Color = Color::Rgb(18, 24, 36);
const BORDER: Color = Color::Cyan;
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_MUTED: Color = Color::Rgb(140, 140, 140);
const KEY_FG: Color = Color::Rgb(229, 192, 123);
const SECTION_FG: Color = Color::Rgb(86, 182, 194);

// ── Binding data ──────────────────────────────────────────────────────────────

struct Binding {
    key: &'static str,
    desc: &'static str,
}

struct Section {
    title: &'static str,
    bindings: &'static [Binding],
}

const SECTIONS: &[Section] = &[
    Section {
        title: "Navigation",
        bindings: &[
            Binding { key: "Tab / Shift+Tab", desc: "Switch panel focus" },
            Binding { key: "1-6",             desc: "Jump to tab (Tables/SQL/Logs/Metrics/Module/Live)" },
            Binding { key: "j / ↓",           desc: "Move selection down" },
            Binding { key: "k / ↑",           desc: "Move selection up" },
            Binding { key: "h / ←",           desc: "Sidebar: step up (Tables → Databases) / focus sidebar from main" },
            Binding { key: "l / →",           desc: "Focus main pane" },
            Binding { key: "g / Home",        desc: "Jump to first item" },
            Binding { key: "G / End",         desc: "Jump to last item" },
            Binding { key: "Enter",           desc: "Select / confirm" },
            Binding { key: "Esc / Backspace", desc: "Sidebar: step up tree; otherwise focus sidebar" },
        ],
    },
    Section {
        title: "SQL Console",
        bindings: &[
            Binding { key: ":",               desc: "Enter SQL mode (focus input)" },
            Binding { key: "Enter",           desc: "Execute SQL query" },
            Binding { key: "Tab",             desc: "Autocomplete keyword / table / column" },
            Binding { key: "↑ / ↓",           desc: "Browse query history" },
            Binding { key: "Ctrl+L",          desc: "Clear entire input" },
            Binding { key: "Ctrl+K",          desc: "Kill to end of line" },
            Binding { key: "Ctrl+U",          desc: "Kill to start of line" },
            Binding { key: "Ctrl+W",          desc: "Delete previous word" },
            Binding { key: "Ctrl+A / Home",   desc: "Move cursor to start" },
            Binding { key: "Ctrl+E / End",    desc: "Move cursor to end" },
        ],
    },
    Section {
        title: "Data grid (Tables / SQL)",
        bindings: &[
            Binding { key: "h / l / ← →",     desc: "Move cell cursor across columns" },
            Binding { key: "j / k / ↓ ↑",     desc: "Move cell cursor across rows" },
            Binding { key: "y",               desc: "Copy selected cell to clipboard (OSC 52)" },
            Binding { key: "Y (shift-y)",     desc: "Copy selected row as TSV" },
            Binding { key: "e",               desc: "Export current results as CSV to ./exports/" },
            Binding { key: "E (shift-e)",     desc: "Export current results as JSON to ./exports/" },
            Binding { key: "Ctrl+F",          desc: "Open grid search prompt" },
            Binding { key: "n / N",           desc: "Jump to next / previous search match" },
            Binding { key: "s",               desc: "Cycle sort on selected column (off→asc→desc)" },
            Binding { key: "r",               desc: "Refresh current table data" },
            Binding { key: "n / p",           desc: "Next / previous page (when no search active)" },
        ],
    },
    Section {
        title: "Write ops (Tables tab)",
        bindings: &[
            Binding { key: "i",               desc: "Insert new row (opens form)" },
            Binding { key: "U (shift-u)",     desc: "Update selected row (opens edit form)" },
            Binding { key: "d",               desc: "Delete selected row (asks for y/n confirm)" },
        ],
    },
    Section {
        title: "Module tab — reducer calls",
        bindings: &[
            Binding { key: "j / k",           desc: "Move between reducers" },
            Binding { key: "Enter",           desc: "Open reducer call form" },
        ],
    },
    Section {
        title: "Modal dialogs",
        bindings: &[
            Binding { key: "Tab / ↓",         desc: "Next field (form)" },
            Binding { key: "Shift+Tab / ↑",   desc: "Previous field (form)" },
            Binding { key: "Enter",           desc: "Submit form / confirm" },
            Binding { key: "y",               desc: "Confirm (yes/no prompts)" },
            Binding { key: "n / Esc",         desc: "Cancel modal" },
        ],
    },
    Section {
        title: "Logs",
        bindings: &[
            Binding { key: "Space",           desc: "Pause / resume auto-scroll" },
            Binding { key: "f",               desc: "Cycle minimum log level filter" },
            Binding { key: "r",               desc: "Refresh logs" },
            Binding { key: "c",               desc: "Clear log buffer" },
        ],
    },
    Section {
        title: "Live",
        bindings: &[
            Binding { key: "6",               desc: "Jump to the Live tab (tx feed + clients)" },
            Binding { key: "r",               desc: "Force re-subscribe to the WebSocket feed" },
        ],
    },
    Section {
        title: "Global",
        bindings: &[
            Binding { key: "q",               desc: "Quit the application" },
            Binding { key: "Ctrl+C",          desc: "Force quit" },
            Binding { key: "Ctrl+R",          desc: "Force WebSocket reconnect" },
            Binding { key: "Ctrl+P",          desc: "Open command palette (fuzzy search)" },
            Binding { key: "?",               desc: "Toggle this help overlay" },
            Binding { key: "r",               desc: "Refresh current view" },
        ],
    },
];

// ── Widget ─────────────────────────────────────────────────────────────────────

/// Help overlay.  Renders as a centred popup that clears the area beneath it.
pub struct HelpOverlay {
    /// Vertical scroll offset (line index).
    pub scroll: usize,
}

impl HelpOverlay {
    pub fn new(scroll: usize) -> Self {
        Self { scroll }
    }

    /// Total number of lines the overlay would render with the
    /// current section list. Used by `app.rs` to clamp the scroll
    /// offset so a user mashing `↓` doesn't push the state into
    /// nonsense values that take dozens of `↑` presses to recover.
    pub fn total_lines() -> usize {
        let mut n = 0usize;
        for section in SECTIONS {
            n += 1; // header
            n += section.bindings.len();
            n += 1; // blank line between sections
        }
        n
    }
}

impl Widget for HelpOverlay {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Centre the popup
        let popup_w = area.width.min(62);
        let popup_h = area.height.min(32);
        let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
        let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

        // Clear background
        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Span::styled(
                " ⌨  Key Bindings — press ? to close ",
                Style::default()
                    .fg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER))
            .style(Style::default().bg(BG));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        if inner.height == 0 {
            return;
        }

        // Build all lines
        let mut lines: Vec<Line> = Vec::new();
        for section in SECTIONS {
            // Section header
            lines.push(Line::from(Span::styled(
                format!("  {} ", section.title),
                Style::default()
                    .fg(SECTION_FG)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for binding in section.bindings {
                let key_w = 22usize;
                let key_padded = format!("  {:width$}", binding.key, width = key_w);
                lines.push(Line::from(vec![
                    Span::styled(key_padded, Style::default().fg(KEY_FG)),
                    Span::styled(binding.desc, Style::default().fg(FG_PRIMARY)),
                ]));
            }
            // Blank line between sections
            lines.push(Line::from(""));
        }

        // Scroll indicator
        let total_lines = lines.len();
        let visible_h = inner.height as usize;
        let scroll = self.scroll.min(total_lines.saturating_sub(visible_h));

        // Render visible lines
        for (i, line) in lines.iter().skip(scroll).take(visible_h).enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_line(inner.x, y, line, inner.width);
        }

        // Scroll hint
        if total_lines > visible_h {
            let hint = format!(
                " {}/{} ↑↓ scroll ",
                scroll + visible_h.min(total_lines - scroll),
                total_lines
            );
            let hint_x = inner.x + inner.width.saturating_sub(hint.len() as u16);
            let hint_y = inner.y + inner.height - 1;
            let hint_line = Line::from(Span::styled(hint, Style::default().fg(FG_MUTED)));
            buf.set_line(hint_x, hint_y, &hint_line, inner.width);
        }
    }
}
