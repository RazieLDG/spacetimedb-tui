//! Modal popup renderer.
//!
//! Pure render layer — all state lives in [`crate::state::modal::Modal`].
//! The widget centres a bordered box on top of whatever is underneath
//! and clears the area first so the background doesn't bleed through.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Widget},
};

use crate::state::modal::Modal;

const ACCENT: Color = Color::Cyan;
const BG: Color = Color::Rgb(18, 24, 36);
const FG_MUTED: Color = Color::Rgb(140, 140, 140);
const FG_PRIMARY: Color = Color::Rgb(220, 220, 220);
const FG_FIELD_LABEL: Color = Color::Rgb(86, 182, 194);
const FOCUS_BG: Color = Color::Rgb(40, 60, 90);

/// Render the active modal centred inside `area`.
pub fn render_modal(area: Rect, buf: &mut Buffer, modal: &Modal) {
    // Pick a popup size that depends on the kind of modal:
    // confirm dialogs are short, forms grow with the field count.
    let (w, h) = match modal {
        Modal::Confirm { .. } => (60, 7),
        Modal::Form { fields, .. } => {
            let h = (fields.len() as u16).saturating_mul(3) + 6;
            (70, h.min(area.height.saturating_sub(2)))
        }
    };
    let popup = centered(area, w, h);
    Clear.render(popup, buf);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(BG))
        .title(Span::styled(
            format!(" {} ", modal.title()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    block.render(popup, buf);

    if inner.height == 0 {
        return;
    }

    match modal {
        Modal::Confirm { prompt, .. } => render_confirm(inner, buf, prompt),
        Modal::Form { fields, focus, .. } => render_form(inner, buf, fields, *focus),
    }
}

fn render_confirm(area: Rect, buf: &mut Buffer, prompt: &str) {
    if area.height == 0 {
        return;
    }
    // Wrap the prompt across the available width, line by line.
    let max_w = area.width as usize;
    let mut y = area.y;
    for line in prompt.lines() {
        if y >= area.y + area.height.saturating_sub(2) {
            break;
        }
        let truncated: String = line.chars().take(max_w).collect();
        buf.set_line(
            area.x,
            y,
            &Line::from(Span::styled(
                truncated,
                Style::default().fg(FG_PRIMARY).bg(BG),
            )),
            area.width,
        );
        y += 1;
    }

    // Footer hint
    let hint_y = area.y + area.height - 1;
    let hint = Line::from(vec![
        Span::styled(
            " [y] ",
            Style::default()
                .fg(ACCENT)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("confirm  ", Style::default().fg(FG_PRIMARY).bg(BG)),
        Span::styled(
            " [n / Esc] ",
            Style::default()
                .fg(ACCENT)
                .bg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("cancel ", Style::default().fg(FG_PRIMARY).bg(BG)),
    ]);
    buf.set_line(area.x, hint_y, &hint, area.width);
}

fn render_form(
    area: Rect,
    buf: &mut Buffer,
    fields: &[crate::state::modal::FormField],
    focus: usize,
) {
    if area.height == 0 {
        return;
    }

    // Each field gets a label line + an input line.
    // Reserve the last row for the footer hint.
    let usable_h = area.height.saturating_sub(1);
    let mut y = area.y;

    for (i, field) in fields.iter().enumerate() {
        if y + 1 >= area.y + usable_h {
            break;
        }
        let is_focused = i == focus;
        let label_style = if is_focused {
            Style::default()
                .fg(ACCENT)
                .bg(BG)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG_FIELD_LABEL).bg(BG)
        };
        let arrow = if is_focused { "▶ " } else { "  " };
        let label = Line::from(Span::styled(format!("{arrow}{}", field.label), label_style));
        buf.set_line(area.x, y, &label, area.width);
        y += 1;

        // Input line — show the value, with a visible cursor when
        // focused, plus a placeholder when empty.
        let display_text = if field.input.value.is_empty() {
            field.placeholder.clone().unwrap_or_default()
        } else {
            field.input.value.clone()
        };
        let input_style = if is_focused {
            Style::default().fg(FG_PRIMARY).bg(FOCUS_BG)
        } else {
            Style::default().fg(FG_PRIMARY).bg(BG)
        };
        let placeholder_style = Style::default()
            .fg(FG_MUTED)
            .bg(input_style.bg.unwrap_or(BG));

        // Pad the input to the full width so the focus background
        // covers the whole row.
        let padded = format!(
            "  {display_text:<width$}",
            width = (area.width as usize).saturating_sub(2)
        );
        let input_line = if field.input.value.is_empty() && !is_focused {
            Line::from(Span::styled(padded, placeholder_style))
        } else {
            Line::from(Span::styled(padded, input_style))
        };
        buf.set_line(area.x, y, &input_line, area.width);

        // Draw the cursor as a single bright cell on top of the input
        // when this field has focus.
        if is_focused {
            // Cursor X = 2 (left padding) + cursor display column,
            // clamped to inside the popup.
            let display_col_within_input = field.input.value[..field.input.cursor].chars().count();
            let cursor_x = area.x + 2 + display_col_within_input as u16;
            if cursor_x < area.x + area.width {
                let ch = field
                    .input
                    .value
                    .chars()
                    .nth(display_col_within_input)
                    .unwrap_or(' ');
                buf[(cursor_x, y)].set_char(ch).set_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(ACCENT)
                        .add_modifier(Modifier::BOLD),
                );
            }
        }

        y += 1;
    }

    // Footer hint
    if area.height > 0 {
        let hint_y = area.y + area.height - 1;
        let hint = Line::from(vec![
            Span::styled(
                " [Tab] ",
                Style::default()
                    .fg(ACCENT)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("next field  ", Style::default().fg(FG_PRIMARY).bg(BG)),
            Span::styled(
                " [Enter] ",
                Style::default()
                    .fg(ACCENT)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("submit  ", Style::default().fg(FG_PRIMARY).bg(BG)),
            Span::styled(
                " [Esc] ",
                Style::default()
                    .fg(ACCENT)
                    .bg(BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("cancel ", Style::default().fg(FG_PRIMARY).bg(BG)),
        ]);
        buf.set_line(area.x, hint_y, &hint, area.width);
    }
}

/// Centre a `w × h` rectangle inside `area`, clamped so we never
/// overflow the terminal.
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
