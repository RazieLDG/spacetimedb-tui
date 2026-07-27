//! Command palette overlay renderer.
//!
//! Pure render layer — state lives in
//! [`crate::state::palette::CommandPalette`]. Drawn on top of every
//! other widget, just like the modal popup.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::state::palette::{Command, CommandPalette};

const ACCENT: Color = Color::Cyan;
const BG: Color = Color::Rgb(18, 24, 36);
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_MUTED: Color = Color::Rgb(140, 140, 140);
const SELECTED_BG: Color = Color::Rgb(40, 60, 90);

/// Render the command palette centred inside `area`.
pub fn render_palette(area: Rect, buf: &mut Buffer, palette: &CommandPalette) {
    let (w, h) = (60u16, 14u16);
    let popup = centered(area, w, h);
    Clear.render(popup, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG))
        .title(Span::styled(
            " Command palette ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    block.render(popup, buf);

    if inner.height < 3 {
        return;
    }

    // ── Query line (top) ──────────────────────────────────────────────
    let query_y = inner.y;
    let prompt = "▶ ";
    let typed = &palette.query.value;
    buf.set_line(
        inner.x,
        query_y,
        &Line::from(vec![
            Span::styled(
                prompt,
                Style::default()
                    .fg(ACCENT)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(typed.clone(), Style::default().fg(FG_PRIMARY).bg(BG)),
            // Visible cursor block
            Span::styled(
                "▏",
                Style::default()
                    .fg(ACCENT)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        inner.width,
    );

    // Separator under the query.
    let sep_y = query_y + 1;
    if sep_y < inner.y + inner.height {
        for x in inner.x..inner.x + inner.width {
            buf[(x, sep_y)]
                .set_char('─')
                .set_style(Style::default().fg(Color::Rgb(40, 50, 65)).bg(BG));
        }
    }

    // ── Filtered command list ─────────────────────────────────────────
    let list_top = sep_y + 1;
    let list_h = inner.height.saturating_sub(2) as usize;
    let results = palette.filter();
    let selected_idx = palette.selected.min(results.len().saturating_sub(1));

    if results.is_empty() {
        let line = Line::from(Span::styled(
            "  (no matches)",
            Style::default().fg(FG_MUTED).bg(BG),
        ));
        buf.set_line(inner.x, list_top, &line, inner.width);
        return;
    }

    for (i, cmd) in results.iter().take(list_h).enumerate() {
        let y = list_top + i as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let is_selected = i == selected_idx;
        let row_bg = if is_selected { SELECTED_BG } else { BG };

        // Fill the row background.
        for x in inner.x..inner.x + inner.width {
            buf[(x, y)]
                .set_char(' ')
                .set_style(Style::default().bg(row_bg));
        }

        let arrow = if is_selected { "▶ " } else { "  " };
        let label_span = Span::styled(
            format!("{arrow}{}", cmd.label()),
            Style::default()
                .fg(if is_selected { ACCENT } else { FG_PRIMARY })
                .bg(row_bg)
                .add_modifier(if is_selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        );
        let shortcut = cmd.shortcut();
        let shortcut_span = Span::styled(
            format!(" [{shortcut}]"),
            Style::default().fg(FG_MUTED).bg(row_bg),
        );

        // Render label on the left, shortcut on the right.
        let label_line = Line::from(label_span);
        buf.set_line(
            inner.x,
            y,
            &label_line,
            inner.width.saturating_sub(shortcut.len() as u16 + 4),
        );
        let sc_x = inner.x + inner.width.saturating_sub(shortcut.len() as u16 + 4);
        buf.set_line(sc_x, y, &Line::from(shortcut_span), inner.width);

        // Silence Command unused-imports complaints when this file
        // is the only consumer.
        let _ = Command::Quit;
    }
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}
