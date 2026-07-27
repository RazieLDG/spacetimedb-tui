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
//!     - `s` → aggregate the one dirty typed row into one guided
//!       `WritePlan` and confirmation
//!     - `u` → revert the pending change on the active cell
//!     - `Ctrl+E` or `Esc` → exit edit mode (asks for confirmation
//!       if there are uncommitted edits)
//!
//! Declared complete primary-key metadata determines which columns are
//! read-only in edit mode. There is no heuristic `pick_primary_key` fallback;
//! in-memory row edit state is kept separate from the guided update plan sent to the server.

use std::collections::BTreeMap;

use crate::state::safety::{CompletePrimaryKey, SqlValue};
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
    /// New value the user typed. It is parsed into the typed guided-update
    /// value when the dirty row is saved.
    pub new_value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditRowTarget {
    pub primary_key: CompletePrimaryKey,
    pub row_generation: u64,
    pub data_row_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellChange {
    pub column_id: u32,
    pub old_value: Option<SqlValue>,
    pub new_value: SqlValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SingleRowSave {
    pub target: EditRowTarget,
    pub changes: Vec<CellChange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCellProjection {
    pub data_row: usize,
    pub column_index: usize,
    pub original_display: String,
    pub new_display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditCellValues {
    pub column_id: u32,
    pub old_value: Option<SqlValue>,
    pub new_value: SqlValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginEditResult {
    Began,
    NeedsDirtyRowChoice {
        dirty_row: EditRowTarget,
        requested_row: EditRowTarget,
        requested_column: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpreadsheetEditError {
    MultiRowSaveAllDisabled,
    DefinitelyNotSent { reason: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpreadsheetEdits {
    dirty_target: Option<EditRowTarget>,
    changes: BTreeMap<u32, CellChange>,
}

impl SpreadsheetEdits {
    pub fn dirty_target(&self) -> Option<&EditRowTarget> {
        self.dirty_target.as_ref()
    }

    pub fn begin_edit(
        &self,
        requested_row: EditRowTarget,
        requested_column: u32,
    ) -> BeginEditResult {
        match &self.dirty_target {
            Some(dirty_row) if dirty_row != &requested_row => {
                BeginEditResult::NeedsDirtyRowChoice {
                    dirty_row: dirty_row.clone(),
                    requested_row,
                    requested_column,
                }
            }
            _ => BeginEditResult::Began,
        }
    }

    pub fn set_cell(
        &mut self,
        target: EditRowTarget,
        column_id: u32,
        old_value: Option<SqlValue>,
        new_value: SqlValue,
    ) -> Result<(), SpreadsheetEditError> {
        if let Some(dirty_target) = &self.dirty_target {
            if dirty_target.primary_key == target.primary_key
                && dirty_target.data_row_index == target.data_row_index
                && dirty_target.row_generation != target.row_generation
            {
                return Err(SpreadsheetEditError::DefinitelyNotSent {
                    reason: "row generation changed before edit".into(),
                });
            }
            if dirty_target != &target {
                return Err(SpreadsheetEditError::MultiRowSaveAllDisabled);
            }
        } else {
            self.dirty_target = Some(target);
        }

        match self.changes.get_mut(&column_id) {
            Some(existing) => {
                if existing.old_value.as_ref() == Some(&new_value) {
                    self.changes.remove(&column_id);
                } else {
                    existing.new_value = new_value;
                }
            }
            None => {
                if old_value.as_ref() != Some(&new_value) {
                    self.changes.insert(
                        column_id,
                        CellChange {
                            column_id,
                            old_value,
                            new_value,
                        },
                    );
                }
            }
        }

        if self.changes.is_empty() {
            self.dirty_target = None;
        }
        Ok(())
    }

    pub fn remove_cell_change(&mut self, column_id: u32) -> bool {
        let removed = self.changes.remove(&column_id).is_some();
        if self.changes.is_empty() {
            self.dirty_target = None;
        }
        removed
    }

    pub fn remove_cell_change_for_target(
        &mut self,
        target: &EditRowTarget,
        column_id: u32,
    ) -> Result<bool, SpreadsheetEditError> {
        match &self.dirty_target {
            Some(dirty_target) if dirty_target == target => Ok(self.remove_cell_change(column_id)),
            Some(_) => Err(SpreadsheetEditError::DefinitelyNotSent {
                reason: "active row does not match dirty spreadsheet row".to_string(),
            }),
            None => Ok(false),
        }
    }

    pub fn discard(&mut self) {
        self.dirty_target = None;
        self.changes.clear();
    }

    pub fn pending_single_row_save(&self) -> Option<SingleRowSave> {
        let target = self.dirty_target.clone()?;
        if self.changes.is_empty() {
            return None;
        }
        Some(SingleRowSave {
            target,
            changes: self.changes.values().cloned().collect(),
        })
    }

    pub fn pending_single_row_save_for_current_target(
        &self,
        current_target: &EditRowTarget,
    ) -> Result<Option<SingleRowSave>, SpreadsheetEditError> {
        let Some(dirty_target) = &self.dirty_target else {
            return Ok(None);
        };
        if dirty_target.primary_key != current_target.primary_key {
            return Err(SpreadsheetEditError::DefinitelyNotSent {
                reason: "row identity changed before save".into(),
            });
        }
        if dirty_target.data_row_index != current_target.data_row_index {
            return Err(SpreadsheetEditError::DefinitelyNotSent {
                reason: "row position changed before save".into(),
            });
        }
        if dirty_target.row_generation != current_target.row_generation {
            return Err(SpreadsheetEditError::DefinitelyNotSent {
                reason: "row generation changed before save".into(),
            });
        }
        Ok(self.pending_single_row_save())
    }

    pub fn save_all(&self) -> Result<(), SpreadsheetEditError> {
        Err(SpreadsheetEditError::MultiRowSaveAllDisabled)
    }
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
    spreadsheet_edits: SpreadsheetEdits,
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

    pub fn spreadsheet_edits(&self) -> &SpreadsheetEdits {
        &self.spreadsheet_edits
    }

    pub fn set_cell(
        &mut self,
        target: EditRowTarget,
        projection: EditCellProjection,
        values: EditCellValues,
    ) -> Result<(), SpreadsheetEditError> {
        self.spreadsheet_edits.set_cell(
            target,
            values.column_id,
            values.old_value,
            values.new_value,
        )?;
        if self
            .spreadsheet_edits
            .changes
            .contains_key(&values.column_id)
        {
            self.upsert(
                projection.data_row,
                projection.column_index,
                projection.original_display,
                projection.new_display,
            );
        } else {
            self.revert(projection.data_row, projection.column_index);
        }
        Ok(())
    }

    pub fn remove_cell_change_for_target(
        &mut self,
        target: &EditRowTarget,
        data_row: usize,
        col: usize,
        column_id: u32,
    ) -> Result<bool, SpreadsheetEditError> {
        let removed = self
            .spreadsheet_edits
            .remove_cell_change_for_target(target, column_id)?;
        if removed {
            self.revert(data_row, col);
        }
        Ok(removed)
    }

    pub fn discard_spreadsheet_edits(&mut self) {
        self.spreadsheet_edits.discard();
        self.pending.clear();
    }

    pub fn pending_single_row_save(&self) -> Option<SingleRowSave> {
        self.spreadsheet_edits.pending_single_row_save()
    }

    pub fn pending_single_row_save_for_current_target(
        &self,
        current_target: &EditRowTarget,
    ) -> Result<Option<SingleRowSave>, SpreadsheetEditError> {
        self.spreadsheet_edits
            .pending_single_row_save_for_current_target(current_target)
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

#[cfg(test)]
mod one_row_spreadsheet_tests {
    use super::*;
    use crate::state::safety::{CompletePrimaryKey, PrimaryKeyPart, SqlValue};

    fn pk(value: i64) -> CompletePrimaryKey {
        CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart {
            column_id: 1,
            value: SqlValue::I64(value),
        }])
        .unwrap()
    }

    fn target(value: i64, generation: u64, data_row_index: usize) -> EditRowTarget {
        EditRowTarget {
            primary_key: pk(value),
            row_generation: generation,
            data_row_index,
        }
    }

    #[test]
    fn set_cell_keeps_one_stable_target_and_first_old_value() {
        let mut edits = SpreadsheetEdits::default();
        let row = target(7, 11, 3);

        assert_eq!(edits.begin_edit(row.clone(), 2), BeginEditResult::Began);
        edits
            .set_cell(
                row.clone(),
                10,
                Some(SqlValue::Text("first".into())),
                SqlValue::Text("second".into()),
            )
            .unwrap();
        edits
            .set_cell(
                row.clone(),
                10,
                Some(SqlValue::Text("ignored".into())),
                SqlValue::Text("third".into()),
            )
            .unwrap();
        edits
            .set_cell(row.clone(), 9, None, SqlValue::Bool(true))
            .unwrap();

        let save = edits.pending_single_row_save().unwrap();
        assert_eq!(save.target, row);
        assert_eq!(
            save.changes.iter().map(|c| c.column_id).collect::<Vec<_>>(),
            vec![9, 10]
        );
        assert_eq!(
            save.changes[1].old_value,
            Some(SqlValue::Text("first".into()))
        );
        assert_eq!(save.changes[1].new_value, SqlValue::Text("third".into()));
    }

    #[test]
    fn reverting_to_original_typed_value_removes_change_and_dirty_target() {
        let mut edits = SpreadsheetEdits::default();
        let row = target(7, 11, 3);

        edits
            .set_cell(row.clone(), 10, Some(SqlValue::I64(1)), SqlValue::I64(2))
            .unwrap();
        assert_eq!(edits.dirty_target(), Some(&row));

        edits
            .set_cell(row, 10, Some(SqlValue::I64(999)), SqlValue::I64(1))
            .unwrap();

        assert!(edits.dirty_target().is_none());
        assert!(edits.pending_single_row_save().is_none());
    }

    #[test]
    fn begin_edit_reports_dirty_row_choice_for_different_stable_row() {
        let mut edits = SpreadsheetEdits::default();
        let dirty = target(1, 5, 0);
        let requested = target(2, 5, 1);
        edits
            .set_cell(dirty.clone(), 10, None, SqlValue::Text("dirty".into()))
            .unwrap();

        assert_eq!(
            edits.begin_edit(requested.clone(), 4),
            BeginEditResult::NeedsDirtyRowChoice {
                dirty_row: dirty,
                requested_row: requested,
                requested_column: 4,
            }
        );
    }

    #[test]
    fn current_target_save_rejects_scope_and_generation_drift() {
        let mut edits = SpreadsheetEdits::default();
        let dirty = target(1, 5, 0);
        edits
            .set_cell(dirty.clone(), 10, None, SqlValue::Text("dirty".into()))
            .unwrap();

        assert_eq!(
            edits
                .pending_single_row_save_for_current_target(&target(2, 5, 0))
                .unwrap_err(),
            SpreadsheetEditError::DefinitelyNotSent {
                reason: "row identity changed before save".into()
            }
        );
        assert_eq!(
            edits
                .pending_single_row_save_for_current_target(&target(1, 5, 2))
                .unwrap_err(),
            SpreadsheetEditError::DefinitelyNotSent {
                reason: "row position changed before save".into()
            }
        );
        assert_eq!(
            edits
                .pending_single_row_save_for_current_target(&target(1, 6, 0))
                .unwrap_err(),
            SpreadsheetEditError::DefinitelyNotSent {
                reason: "row generation changed before save".into()
            }
        );
    }

    #[test]
    fn set_cell_rejects_same_row_with_different_generation_before_edit_reason() {
        let mut edits = SpreadsheetEdits::default();
        edits
            .set_cell(target(1, 5, 0), 10, None, SqlValue::Text("dirty".into()))
            .unwrap();

        assert_eq!(
            edits
                .set_cell(target(1, 6, 0), 11, None, SqlValue::Text("other".into()))
                .unwrap_err(),
            SpreadsheetEditError::DefinitelyNotSent {
                reason: "row generation changed before edit".into()
            }
        );
    }

    #[test]
    fn set_cell_rejects_different_stable_row_as_multi_row_save_disabled() {
        let mut edits = SpreadsheetEdits::default();
        edits
            .set_cell(target(1, 5, 0), 10, None, SqlValue::Text("dirty".into()))
            .unwrap();

        assert_eq!(
            edits
                .set_cell(target(2, 5, 1), 11, None, SqlValue::Text("other".into()))
                .unwrap_err(),
            SpreadsheetEditError::MultiRowSaveAllDisabled
        );
    }

    #[test]
    fn edit_mode_uses_declared_column_id_for_canonical_changes_and_index_for_projection() {
        let mut edit_mode = EditMode::new();
        let row = target(1, 5, 0);

        edit_mode
            .set_cell(
                row.clone(),
                EditCellProjection {
                    data_row: 0,
                    column_index: 2,
                    original_display: "old".into(),
                    new_display: "new".into(),
                },
                EditCellValues {
                    column_id: 99,
                    old_value: Some(SqlValue::Text("old".into())),
                    new_value: SqlValue::Text("new".into()),
                },
            )
            .unwrap();

        let save = edit_mode.pending_single_row_save().unwrap();
        assert_eq!(save.changes[0].column_id, 99);
        assert!(edit_mode.find(0, 2).is_some());
        assert_eq!(
            edit_mode.remove_cell_change_for_target(&row, 0, 2, 99),
            Ok(true)
        );
        assert!(edit_mode.pending_single_row_save().is_none());
        assert!(edit_mode.find(0, 2).is_none());
    }

    #[test]
    fn remove_cell_change_on_same_column_wrong_row_preserves_canonical_and_projection() {
        let mut edit_mode = EditMode::new();
        let dirty_target = target(1, 10, 0);
        let other_target = target(2, 10, 1);
        edit_mode
            .set_cell(
                dirty_target.clone(),
                EditCellProjection {
                    data_row: 0,
                    column_index: 1,
                    original_display: "Ada".to_string(),
                    new_display: "Adele".to_string(),
                },
                EditCellValues {
                    column_id: 700,
                    old_value: Some(SqlValue::Text("Ada".to_string())),
                    new_value: SqlValue::Text("Adele".to_string()),
                },
            )
            .unwrap();

        assert!(matches!(
            edit_mode.remove_cell_change_for_target(&other_target, 1, 1, 700),
            Err(SpreadsheetEditError::DefinitelyNotSent { reason })
                if reason == "active row does not match dirty spreadsheet row"
        ));

        let save = edit_mode
            .pending_single_row_save()
            .expect("dirty row remains");
        assert_eq!(save.target, dirty_target);
        assert_eq!(save.changes.len(), 1);
        assert_eq!(edit_mode.find(0, 1).unwrap().new_value, "Adele");
    }

    #[test]
    fn remove_cell_change_on_same_dirty_row_removes_canonical_and_projection() {
        let mut edit_mode = EditMode::new();
        let dirty_target = target(1, 10, 0);
        edit_mode
            .set_cell(
                dirty_target.clone(),
                EditCellProjection {
                    data_row: 0,
                    column_index: 1,
                    original_display: "Ada".to_string(),
                    new_display: "Adele".to_string(),
                },
                EditCellValues {
                    column_id: 700,
                    old_value: Some(SqlValue::Text("Ada".to_string())),
                    new_value: SqlValue::Text("Adele".to_string()),
                },
            )
            .unwrap();

        assert_eq!(
            edit_mode.remove_cell_change_for_target(&dirty_target, 0, 1, 700),
            Ok(true)
        );

        assert!(edit_mode.pending_single_row_save().is_none());
        assert!(edit_mode.find(0, 1).is_none());
    }

    #[test]
    fn save_all_is_disabled_and_discard_remove_cell_accessors_synchronize_projection() {
        let mut edit_mode = EditMode::new();
        let row = target(1, 5, 0);

        edit_mode
            .set_cell(
                row.clone(),
                EditCellProjection {
                    data_row: 2,
                    column_index: 3,
                    original_display: "old".into(),
                    new_display: "new".into(),
                },
                EditCellValues {
                    column_id: 3,
                    old_value: Some(SqlValue::Text("old".into())),
                    new_value: SqlValue::Text("new".into()),
                },
            )
            .unwrap();
        assert_eq!(edit_mode.pending_count(), 1);
        assert_eq!(edit_mode.spreadsheet_edits().dirty_target(), Some(&row));
        assert_eq!(
            edit_mode.spreadsheet_edits().save_all(),
            Err(SpreadsheetEditError::MultiRowSaveAllDisabled)
        );

        assert_eq!(
            edit_mode.remove_cell_change_for_target(&row, 2, 3, 3),
            Ok(true)
        );
        assert_eq!(edit_mode.pending_count(), 0);
        assert!(edit_mode.spreadsheet_edits().dirty_target().is_none());

        edit_mode
            .set_cell(
                row.clone(),
                EditCellProjection {
                    data_row: 2,
                    column_index: 3,
                    original_display: "old".into(),
                    new_display: "new".into(),
                },
                EditCellValues {
                    column_id: 3,
                    old_value: Some(SqlValue::Text("old".into())),
                    new_value: SqlValue::Text("new".into()),
                },
            )
            .unwrap();
        edit_mode.discard_spreadsheet_edits();
        assert_eq!(edit_mode.pending_count(), 0);
        assert!(edit_mode.spreadsheet_edits().dirty_target().is_none());
    }
}
