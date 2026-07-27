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
            Binding { key: "/ (slash)",       desc: "Sidebar search: filter databases / tables as you type" },
        ],
    },
    Section {
        title: "Help overlay",
        bindings: &[
            Binding { key: "?",               desc: "Toggle this overlay" },
            Binding { key: "j / k / ↓ ↑",     desc: "Scroll the help text line by line" },
            Binding { key: "g / Home",        desc: "Jump to the top" },
            Binding { key: "G / End",         desc: "Jump to the bottom" },
            Binding { key: "Esc / q",         desc: "Close the overlay" },
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
            Binding { key: "Ctrl+E / End",    desc: "Move cursor to end (SQL input only)" },
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
            Binding { key: "D (shift-d)",     desc: "Truncate table (typed-confirm: type the name)" },
            Binding { key: "Ctrl+E",          desc: "Enter spreadsheet edit mode (Main focus only — in SQL input it moves cursor to end)" },
        ],
    },
    Section {
        title: "Spreadsheet edit mode (Tables tab)",
        bindings: &[
            Binding { key: "h / j / k / l",   desc: "Move cell cursor (Vim style)" },
            Binding { key: "Enter / i",       desc: "Open inline editor on selected cell" },
            Binding { key: "Enter",           desc: "Commit inline editor value to pending list" },
            Binding { key: "Esc (in editor)", desc: "Cancel inline edit without committing" },
            Binding { key: "s",               desc: "Save one typed dirty row through one guided update confirmation" },
            Binding { key: "u",               desc: "Revert pending edit on active cell" },
            Binding { key: "Ctrl+E / Esc",    desc: "Exit edit mode (asks if pending edits > 0)" },
        ],
    },
    Section {
        title: "Admin ops (sidebar — Databases)",
        bindings: &[
            Binding { key: "a",               desc: "Add a new alias / human name to the selected database" },
            Binding { key: "D (shift-d)",     desc: "DELETE database (typed-confirm: type the name)" },
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
            Binding { key: "6",               desc: "Jump to the Live tab (disabled notice + clients)" },
            Binding { key: "r",               desc: "Force refresh of the connected-clients metadata poll" },
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

    /// Total number of lines the overlay can render, used by `app.rs`
    /// to clamp the scroll offset so a user mashing `↓` doesn't push
    /// the state into nonsense values that take dozens of `↑` presses
    /// to recover.
    ///
    /// The binding sections have a fixed, width-independent line count.
    /// The Phase 1 notice is word-wrapped to the popup width at render
    /// time, so its exact line count is not known here; we count one
    /// line per word (plus the header and trailing blank line) as a
    /// strict upper bound. The render path clamps the *displayed* scroll
    /// to the real wrapped line count, so overestimating here only lets
    /// the stored offset run a few lines past the visible end on wide
    /// terminals — it never hides the last bindings (the bug an
    /// underestimate would cause).
    pub fn total_lines() -> usize {
        let mut n = 0usize;

        // Phase 1 safety notice: header + one line per word (upper
        // bound on the wrapped line count) + trailing blank line.
        n += 1;
        n += phase_one_safety_help_text().split_whitespace().count();
        n += 1;

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
        // Centre the popup — grow to consume almost the whole screen
        // so the long binding list fits without forcing the user to
        // scroll through a narrow peephole.
        let popup_w = area.width.saturating_sub(4).min(78);
        let popup_h = area.height.saturating_sub(2);
        let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
        let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

        // Clear background
        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Span::styled(
                " ⌨  Key Bindings — press ? to close ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
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

        // Phase 1 safety notice — kept truthful with README/CHANGELOG by
        // `phase_one_truth_gate_tests`.
        let notice_width = inner.width.saturating_sub(2) as usize;
        lines.push(Line::from(Span::styled(
            "  Phase 1 safety ",
            Style::default()
                .fg(SECTION_FG)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for wrapped in wrap_notice(phase_one_safety_help_text(), notice_width) {
            lines.push(Line::from(Span::styled(
                format!("  {wrapped}"),
                Style::default().fg(FG_MUTED),
            )));
        }
        lines.push(Line::from(""));

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

        // Scroll hint — clamped to the last row of the inner area so
        // an `inner.height == 0` edge case (e.g. a 1-row popup on a
        // tiny terminal) can't underflow and overwrite the border.
        if total_lines > visible_h && inner.height > 0 {
            let hint = format!(
                " {}/{} ↑↓ scroll ",
                scroll + visible_h.min(total_lines - scroll),
                total_lines
            );
            let hint_x = inner.x + inner.width.saturating_sub(hint.len() as u16);
            let hint_y = inner.y + inner.height.saturating_sub(1);
            let hint_line = Line::from(Span::styled(hint, Style::default().fg(FG_MUTED)));
            buf.set_line(hint_x, hint_y, &hint_line, inner.width);
        }
    }
}

/// Phase 1 safety behavior summary shown in help/docs.
///
/// Kept in sync with README.md and CHANGELOG.md by
/// `phase_one_truth_gate_tests`.
pub fn phase_one_safety_help_text() -> &'static str {
    "Phase 1 safety: Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable. Spreadsheet editing supports one-row spreadsheet Save only: changed cells in one row are saved as one guided WritePlan, and changing rows prompts Save, Discard, or Stay. Guided update/delete require declared primary keys and matching generations, with no unsafe override. Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried. Unknown mutation outcomes are not automatically retried; refresh the affected scope before another guided attempt."
}

/// Phase 2 help note: shortcuts, palette, and help share one command registry.
#[allow(dead_code)]
pub const PHASE2_HELP_NOTE: &str = "Phase 2: shortcuts, palette, and help share one command registry. The layout is unchanged until the workbench UX phase.";

/// Word-wrap `text` to at most `width` columns, breaking on spaces.
///
/// A `width` of 0 yields a single line containing the whole text (the
/// renderer clamps overflow). Words longer than `width` are kept intact
/// rather than split mid-word so no line is silently truncated inside a
/// token.
fn wrap_notice(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_owned()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

// ── Phase 2: registry-driven contextual help ─────────────────────────────────

use crate::app::command::{CommandContext, CommandRegistry, ContextualHelpLine};

/// Generate contextual help lines from the command registry for the given
/// context. Phase 3 UI will call this instead of the static binding list.
#[allow(dead_code)]
pub fn contextual_help_lines(context: &CommandContext) -> Vec<ContextualHelpLine> {
    CommandRegistry.contextual_help(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_lines_counts_the_phase_one_notice_section() {
        // Binding sections alone: header + bindings + trailing blank
        // per section.
        let bindings_only: usize = SECTIONS
            .iter()
            .map(|section| 1 + section.bindings.len() + 1)
            .sum();

        // The overlay also renders the Phase 1 safety notice (header +
        // wrapped body + trailing blank line). total_lines() must count
        // it, otherwise app.rs clamps the scroll offset too low and the
        // last bindings become unreachable.
        assert!(
            HelpOverlay::total_lines() > bindings_only,
            "total_lines() {} must exceed bindings-only {} to include the Phase 1 notice",
            HelpOverlay::total_lines(),
            bindings_only
        );

        // Upper-bound accounting: notice header + one line per word +
        // trailing blank, then the binding sections.
        let notice_words = phase_one_safety_help_text().split_whitespace().count();
        assert_eq!(HelpOverlay::total_lines(), bindings_only + notice_words + 2);
    }

    fn spreadsheet_s_description() -> &'static str {
        SECTIONS
            .iter()
            .find(|section| section.title == "Spreadsheet edit mode (Tables tab)")
            .and_then(|section| section.bindings.iter().find(|binding| binding.key == "s"))
            .map(|binding| binding.desc)
            .expect("spreadsheet s binding exists")
    }

    #[test]
    fn spreadsheet_save_binding_describes_one_typed_dirty_row_guided_update() {
        let desc = spreadsheet_s_description();
        assert!(desc.contains("one typed dirty row"), "desc was: {desc}");
        assert!(
            desc.contains("one guided update confirmation"),
            "desc was: {desc}"
        );
        for forbidden in [
            "Save all",
            "Save All",
            "all pending edits",
            "batched UPDATE",
            "spawn UPDATE",
            "spawns UPDATE",
        ] {
            assert!(
                !desc.contains(forbidden),
                "desc must not contain {forbidden:?}: {desc}"
            );
        }
    }
}

#[cfg(test)]
mod phase_one_truth_gate_tests {
    use super::*;
    use std::fs;

    #[test]
    fn phase_one_behavior_claims_are_truthful_in_docs_and_help() {
        let readme = fs::read_to_string("README.md").unwrap();
        let changelog = fs::read_to_string("CHANGELOG.md").unwrap();
        let help = phase_one_safety_help_text();

        for text in [&readme, &changelog, help] {
            assert!(text.contains("Live updates are temporarily unavailable"));
            assert!(text.contains("one-row spreadsheet Save"));
            assert!(text.contains("Unknown mutation outcomes are not automatically retried"));
            assert!(text.contains("Guided update/delete require declared primary keys and matching generations, with no unsafe override"));
            assert!(text.contains("Raw SQL is a separately labeled expert path"));
            assert!(text.contains("Raw SQL does not receive the guided CRUD guarantee and is never automatically retried"));
        }
    }
}
