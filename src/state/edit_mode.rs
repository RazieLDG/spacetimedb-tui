//! Spreadsheet-style edit mode for the Tables tab.
//!
//! Lives here instead of in `app_state.rs` because it has its own
//! non-trivial data model. The Tables tab holds an optional
//! [`EditMode`] which, when `Some`, changes the meaning of every
//! key binding in the main pane:
//!
//! - `h` / `j` / `k` / `l` → move the cell cursor (same as read mode)
//! - `i` / `Enter` → open an inline editor over the active cell
//! - `Esc` → close the inline editor without saving that cell
//! - When the editor is closed:
//!     - `s` → persist every pending edit via `UPDATE ... WHERE pk=...`
//!     - `u` → revert the pending change on the active cell
//!     - `Ctrl+E` or `Esc` → exit edit mode (asks for confirmation
//!       if there are uncommitted edits)
//!
//! The PK column — detected with the existing `pick_primary_key`
//! helper — is read-only in edit mode so nothing corrupts the
//! WHERE clause.

use crate::ui::components::input::InputState;

/// One pending cell edit that hasn't been flushed to the server yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEdit {
    /// Index into the **unsorted** `table_browse_result.rows` — the
    /// sort permutation may change between renders, but the data
    /// index is stable until the next refresh.
    pub data_row_idx: usize,
    /// Column index in the schema (matches `table.columns[col_idx]`).
    pub col_idx: usize,
    /// Original cell value (display form) captured when the edit
    /// was first made — used by `u` / revert and by the renderer to
    /// show the "was X" strikethrough.
    pub original: String,
    /// New value the user typed. Still the raw form — type coercion
    /// to a SQL literal happens at save time via `sql_literal`.
    pub new_value: String,
}

/// Live state of the edit-mode overlay for the Tables tab.
#[derive(Debug, Clone, Default)]
pub struct EditMode {
    /// List of pending edits, in insertion order. Deduplicated per
    /// `(data_row_idx, col_idx)` — writing to the same cell twice
    /// replaces the earlier pending entry.
    pub pending: Vec<PendingEdit>,
    /// When `Some`, an inline input is open over the active cell
    /// and every keystroke goes into this buffer instead of the
    /// outer edit-mode key map.
    pub editor: Option<InputState>,
}

impl EditMode {
    pub fn new() -> Self {
        Self::default()
    }

    /// Find the pending edit targeting a specific cell, if any.
    pub fn find(&self, data_row: usize, col: usize) -> Option<&PendingEdit> {
        self.pending
            .iter()
            .find(|e| e.data_row_idx == data_row && e.col_idx == col)
    }

    /// Idempotently record a pending edit for `(data_row, col)`. If a
    /// pending entry for the same cell already exists, the new value
    /// replaces it (but the `original` field keeps the pre-edit value
    /// so `revert` still works).
    pub fn upsert(&mut self, data_row: usize, col: usize, original: String, new_value: String) {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|e| e.data_row_idx == data_row && e.col_idx == col)
        {
            existing.new_value = new_value;
            return;
        }
        self.pending.push(PendingEdit {
            data_row_idx: data_row,
            col_idx: col,
            original,
            new_value,
        });
    }

    /// Drop the pending edit (if any) targeting `(data_row, col)`.
    /// Returns `true` if something was removed.
    pub fn revert(&mut self, data_row: usize, col: usize) -> bool {
        let before = self.pending.len();
        self.pending
            .retain(|e| !(e.data_row_idx == data_row && e.col_idx == col));
        before != self.pending.len()
    }

    /// Number of pending edits, shown in the status bar. Zero when
    /// the user has no uncommitted changes.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_existing_new_value_but_keeps_original() {
        let mut em = EditMode::new();
        em.upsert(0, 2, "alice".into(), "bob".into());
        em.upsert(0, 2, "alice".into(), "carol".into());
        assert_eq!(em.pending.len(), 1);
        let pe = &em.pending[0];
        assert_eq!(pe.original, "alice");
        assert_eq!(pe.new_value, "carol");
    }

    #[test]
    fn upsert_does_not_touch_other_cells() {
        let mut em = EditMode::new();
        em.upsert(0, 1, "a".into(), "b".into());
        em.upsert(0, 2, "c".into(), "d".into());
        em.upsert(1, 1, "e".into(), "f".into());
        assert_eq!(em.pending.len(), 3);
    }

    #[test]
    fn revert_removes_matching_cell() {
        let mut em = EditMode::new();
        em.upsert(0, 1, "a".into(), "b".into());
        em.upsert(0, 2, "c".into(), "d".into());
        assert!(em.revert(0, 1));
        assert_eq!(em.pending.len(), 1);
        assert_eq!(em.pending[0].col_idx, 2);
    }

    #[test]
    fn revert_returns_false_when_nothing_matches() {
        let mut em = EditMode::new();
        em.upsert(0, 1, "a".into(), "b".into());
        assert!(!em.revert(5, 5));
    }

    #[test]
    fn find_returns_pending_edit() {
        let mut em = EditMode::new();
        em.upsert(3, 4, "old".into(), "new".into());
        let hit = em.find(3, 4).unwrap();
        assert_eq!(hit.new_value, "new");
        assert!(em.find(0, 0).is_none());
    }

    #[test]
    fn pending_count_tracks_upsert() {
        let mut em = EditMode::new();
        assert_eq!(em.pending_count(), 0);
        em.upsert(0, 0, "a".into(), "b".into());
        assert_eq!(em.pending_count(), 1);
        em.upsert(0, 1, "c".into(), "d".into());
        assert_eq!(em.pending_count(), 2);
        em.revert(0, 0);
        assert_eq!(em.pending_count(), 1);
    }
}
