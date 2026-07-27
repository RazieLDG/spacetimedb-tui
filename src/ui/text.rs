//! Unicode-safe text truncation and panic-safe rectangle math for the UI.
//!
//! These helpers keep rendering from ever panicking on narrow terminals or
//! on input containing wide / multi-byte characters.

#[cfg(test)]
use ratatui::layout::Rect;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate `input` so its terminal display width is at most `max_width`,
/// appending a single `…` when truncation occurs.
///
/// Width is measured per character via [`UnicodeWidthChar`], so wide CJK
/// characters count as two columns and combining marks count as zero. The
/// result never exceeds `max_width` columns and never slices inside a UTF-8
/// code point, because it is built by pushing whole `char`s.
pub fn truncate_display_width(input: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(input) <= max_width {
        return input.to_string();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let mut output = String::new();
    let mut used = 0usize;
    let budget = max_width.saturating_sub(1);
    for ch in input.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(width) > budget {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push('…');
    output
}

/// Shrink `area` by `margin` on every side using saturating arithmetic.
///
/// Unlike [`Rect::inner`] (which can underflow on tiny rectangles), this
/// never wraps: a rectangle too small for the margin collapses to a
/// zero-sized rect at the offset position instead of panicking.
///
/// Production layout code uses Ratatui's own [`Block::inner`], which is
/// already safe; this helper is retained for tests that construct raw
/// [`Rect`]s directly.
#[cfg(test)]
pub fn saturating_inner(area: Rect, margin: u16) -> Rect {
    let double = margin.saturating_mul(2);
    Rect {
        x: area.x.saturating_add(margin),
        y: area.y.saturating_add(margin),
        width: area.width.saturating_sub(double),
        height: area.height.saturating_sub(double),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn truncation_respects_unicode_display_width() {
        assert_eq!(truncate_display_width("abcdef", 4), "abc…");
        assert_eq!(truncate_display_width("数据表", 4), "数…");
        // The crab emoji is a width-2 grapheme; truncating to width 3 keeps
        // only the leading "a" (width 1) before the ellipsis.
        assert_eq!(truncate_display_width("a🦀b", 3), "a…");
        assert!(truncate_display_width("e\u{301}clair", 3).ends_with('…'));
    }

    #[test]
    fn truncation_returns_input_when_it_fits() {
        assert_eq!(truncate_display_width("abc", 4), "abc");
        assert_eq!(truncate_display_width("abc", 3), "abc");
        assert_eq!(truncate_display_width("", 4), "");
    }

    #[test]
    fn truncation_never_slices_inside_utf8() {
        for width in 0..8 {
            let truncated = truncate_display_width("é🦀数据", width);
            assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
        }
    }

    #[test]
    fn truncation_output_never_exceeds_budget() {
        use unicode_width::UnicodeWidthStr;
        for width in 1..12 {
            let truncated = truncate_display_width("数据表🦀abcdef", width);
            assert!(
                UnicodeWidthStr::width(truncated.as_str()) <= width,
                "width {} produced {truncated:?} wider than the budget",
                width
            );
        }
    }

    #[test]
    fn saturating_inner_rect_handles_tiny_areas() {
        assert_eq!(
            saturating_inner(Rect::new(0, 0, 1, 1), 2),
            Rect::new(2, 2, 0, 0)
        );
        assert_eq!(
            saturating_inner(Rect::new(2, 3, 10, 5), 1),
            Rect::new(3, 4, 8, 3)
        );
    }
}
