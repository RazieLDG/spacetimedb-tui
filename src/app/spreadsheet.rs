//! Spreadsheet (cell-grid) editing: enter/exit, cell editor lifecycle,
//! typed save pipeline, and error formatting.

use super::*;

impl super::App {
    // ── Spreadsheet edit mode (Faz 10) ───────────────────────────────────

    /// Open spreadsheet edit mode on the Tables tab. No-op if the
    /// user hasn't loaded any table data yet (nothing to edit).
    pub(crate) fn enter_edit_mode(&mut self) {
        if self.state.table_browse_result.is_none() {
            self.state
                .set_notification("Nothing to edit — load a table first".to_string());
            return;
        }
        if self.state.selected_table().is_none() {
            self.state.set_notification("No table selected".to_string());
            return;
        }
        let Some(selected_table) = self.state.selected_table().cloned() else {
            return;
        };
        if let Some(qualified_table) = self.qualified_selected_table(&selected_table) {
            if self.refuse_guided_write_if_unavailable(&qualified_table) {
                return;
            }
        }
        self.state.edit_mode = Some(crate::state::edit_mode::EditMode::new());
        self.state
            .set_notification("EDIT MODE — Ctrl+E to exit".to_string());
    }

    pub(crate) fn active_edit_row_target(&self) -> Result<EditRowTarget, String> {
        let data_row = self
            .active_data_row_index()
            .ok_or_else(|| "No row selected".to_string())?;
        self.edit_row_target_at(data_row)
    }

    pub(crate) fn edit_row_target_at(&self, data_row: usize) -> Result<EditRowTarget, String> {
        let table = self
            .state
            .selected_table()
            .ok_or_else(|| "No table selected".to_string())?;
        let row = self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|result| result.rows.get(data_row))
            .ok_or_else(|| "No row selected".to_string())?;
        let primary_key = complete_primary_key_from_table_row(table, row)
            .map_err(|error| Self::explain_primary_key_error(table, error))?;
        Ok(EditRowTarget {
            primary_key,
            row_generation: self.table_generation,
            data_row_index: data_row,
        })
    }

    /// Leave edit mode. If there are uncommitted edits, pops up a
    /// confirm dialog so the user doesn't lose them by accident.
    pub(crate) async fn exit_edit_mode(&mut self) {
        if self.block_if_spreadsheet_edits_frozen("exit edit mode") {
            return;
        }
        let pending = self
            .state
            .edit_mode
            .as_ref()
            .map(|em| em.pending_count())
            .unwrap_or(0);
        if pending == 0 {
            self.state.edit_mode = None;
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        // Pending changes — ask before discarding. The confirm modal
        // carries ModalAction::DiscardPendingEdits so modal dispatch can
        // either discard the spreadsheet edits or leave edit mode active.
        self.state.modal = Some(crate::state::modal::Modal::confirm(
            "Discard pending edits?",
            format!(
                "You have {pending} uncommitted cell edit(s).\n\
                 Leaving edit mode without saving will drop them.\n\n\
                 Press [y] to discard, [n] to stay in edit mode."
            ),
            crate::state::modal::ModalAction::DiscardPendingEdits,
        ));
    }

    /// Route a key event through the edit-mode key map.
    pub(crate) async fn handle_edit_mode_key(&mut self, key: KeyEvent) {
        // If the inline cell editor is open, keystrokes go into the
        // input buffer instead of the outer key map.
        let editor_open = self
            .state
            .edit_mode
            .as_ref()
            .map(|em| em.editor.is_some())
            .unwrap_or(false);
        if editor_open {
            self.handle_cell_editor_key(key);
            return;
        }

        match key.code {
            KeyCode::Esc => {
                self.exit_edit_mode().await;
            }
            KeyCode::Enter | KeyCode::Char('i') => {
                self.begin_cell_edit();
            }
            KeyCode::Char('s') => {
                self.save_pending_edits().await;
            }
            KeyCode::Char('u') => {
                self.revert_active_cell();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.tables_grid.prev_col();
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let cc = self
                    .state
                    .table_browse_result
                    .as_ref()
                    .map(|qr| qr.column_count())
                    .unwrap_or(0);
                self.tables_grid.next_col(cc);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.tables_grid.prev_row();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let rc = self
                    .state
                    .table_browse_result
                    .as_ref()
                    .map(|qr| qr.row_count())
                    .unwrap_or(0);
                self.tables_grid.next_row(rc);
            }
            _ => {}
        }
    }

    /// Keystrokes routed into the inline cell editor's `InputState`.
    pub(crate) fn handle_cell_editor_key(&mut self, key: KeyEvent) {
        let Some(em) = self.state.edit_mode.as_mut() else {
            return;
        };
        let Some(editor) = em.editor.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                em.editor = None;
            }
            KeyCode::Enter => {
                self.commit_cell_edit();
            }
            KeyCode::Left => editor.move_left(),
            KeyCode::Right => editor.move_right(),
            KeyCode::Home => editor.home(),
            KeyCode::End => editor.end(),
            KeyCode::Backspace => editor.backspace(),
            KeyCode::Delete => editor.delete(),
            KeyCode::Char(ch) => editor.insert(ch),
            _ => {}
        }
    }

    /// Open the inline editor over the currently selected cell,
    /// pre-filled with the cell's current display value (or any
    /// existing pending edit value).
    pub(crate) fn begin_cell_edit(&mut self) {
        if self.block_if_spreadsheet_edits_frozen("edit cells") {
            return;
        }
        // Figure out the data row / column and reject declared PK
        // columns before any editor state is created.
        let data_row = match self.active_data_row_index() {
            Some(i) => i,
            None => return,
        };
        let col_idx = self.tables_grid.selected_col;
        let table = match self.state.selected_table().cloned() {
            Some(t) => t,
            None => return,
        };
        let Some(qualified_table) = self.qualified_selected_table(&table) else {
            return;
        };
        if self.refuse_guided_write_if_unavailable(&qualified_table) {
            return;
        }
        if table.columns.is_empty() {
            return;
        }
        let Some(column) = table.columns.get(col_idx) else {
            return;
        };
        if table
            .primary_key_cols
            .iter()
            .any(|primary_key_col| u32::from(*primary_key_col) == column.col_id)
        {
            self.state
                .set_notification("PK column is read-only in edit mode".to_string());
            return;
        }
        let target = match self.active_edit_row_target() {
            Ok(target) => target,
            Err(error) => {
                self.state.set_error(format!("Cannot edit row: {error}"));
                return;
            }
        };
        if let Some(em) = self.state.edit_mode.as_ref() {
            match em
                .spreadsheet_edits()
                .begin_edit(target.clone(), column.col_id)
            {
                BeginEditResult::Began => {}
                BeginEditResult::NeedsDirtyRowChoice { .. } => {
                    self.pending_spreadsheet_edit_request = Some(PendingSpreadsheetEditRequest {
                        target: target.clone(),
                        column_index: col_idx,
                    });
                    self.state.modal = Some(crate::state::modal::Modal::confirm(
                        "Save pending spreadsheet row?",
                        "[y] Save\n[d] Discard\n[n/Esc] Stay".to_string(),
                        crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
                            requested_row: target.data_row_index,
                            requested_column: column.col_id,
                        },
                    ));
                    return;
                }
            }
        }

        // Prefer the in-flight pending value if one exists, otherwise
        // fall back to the original cell string.
        let cell_text = {
            let em = self.state.edit_mode.as_ref();
            let pending = em.and_then(|e| e.find(data_row, col_idx));
            if let Some(pe) = pending {
                pe.new_value.clone()
            } else {
                self.state
                    .table_browse_result
                    .as_ref()
                    .and_then(|qr| qr.rows.get(data_row))
                    .and_then(|row| row.get(col_idx))
                    .map(crate::ui::tabs::tables::value_to_display)
                    .unwrap_or_default()
            }
        };

        let Some(em) = self.state.edit_mode.as_mut() else {
            return;
        };
        let mut input = InputState::new();
        input.set(cell_text);
        em.editor = Some(input);
    }

    /// Commit whatever's in the inline editor into the pending-edits
    /// list and close the editor.
    pub(crate) fn commit_cell_edit(&mut self) {
        let data_row = match self.active_data_row_index() {
            Some(i) => i,
            None => return,
        };
        let col_idx = self.tables_grid.selected_col;

        // Capture the original display value before we touch `edit_mode`.
        let original = self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|qr| qr.rows.get(data_row))
            .and_then(|row| row.get(col_idx))
            .map(crate::ui::tabs::tables::value_to_display)
            .unwrap_or_default();

        let Some(editor_value) = self
            .state
            .edit_mode
            .as_ref()
            .and_then(|em| em.editor.as_ref())
            .map(|editor| editor.value.clone())
        else {
            return;
        };
        let Some(table) = self.state.selected_table().cloned() else {
            return;
        };
        let Some(column) = table.columns.get(col_idx).cloned() else {
            return;
        };
        let Some(raw_old) = self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|qr| qr.rows.get(data_row))
            .and_then(|row| row.get(col_idx))
            .cloned()
        else {
            return;
        };
        let target = match self.edit_row_target_at(data_row) {
            Ok(target) => target,
            Err(error) => {
                self.state.set_error(format!("Cannot edit row: {error}"));
                return;
            }
        };
        let old_value = match typed_sql_value_from_json(&column, &raw_old) {
            Ok(value) => value,
            Err(error) => {
                self.state.set_error(format!(
                    "Cannot read original value for {} as {}: {error:?}",
                    column.col_name,
                    type_tag(&column.col_type)
                ));
                return;
            }
        };
        let new_value = match guided_form_sql_value_from_user_input(&column, &editor_value) {
            Ok(value) => value,
            Err(error) => {
                self.state.set_error(format!(
                    "Cannot parse {} as {}: {error:?}",
                    column.col_name,
                    type_tag(&column.col_type)
                ));
                return;
            }
        };
        let Some(em) = self.state.edit_mode.as_mut() else {
            return;
        };
        if let Err(error) = em.set_cell(
            target,
            EditCellProjection {
                data_row,
                column_index: col_idx,
                original_display: original,
                new_display: editor_value,
            },
            EditCellValues {
                column_id: column.col_id,
                old_value: Some(old_value),
                new_value,
            },
        ) {
            self.state
                .set_error(Self::format_spreadsheet_edit_error(error));
        } else if let Some(em) = self.state.edit_mode.as_mut() {
            em.editor = None;
        }
    }

    /// Drop the pending edit (if any) on the active cell.
    pub(crate) fn revert_active_cell(&mut self) {
        if self.block_if_spreadsheet_edits_frozen("revert cells") {
            return;
        }
        let data_row = match self.active_data_row_index() {
            Some(i) => i,
            None => return,
        };
        let col_idx = self.tables_grid.selected_col;
        let Some(column_id) = self
            .state
            .selected_table()
            .and_then(|table| table.columns.get(col_idx))
            .map(|column| column.col_id)
        else {
            return;
        };
        let target = match self.active_edit_row_target() {
            Ok(target) => target,
            Err(reason) => {
                self.record_spreadsheet_definitely_not_sent(reason);
                return;
            }
        };
        let Some(em) = self.state.edit_mode.as_mut() else {
            return;
        };
        match em.remove_cell_change_for_target(&target, data_row, col_idx, column_id) {
            Ok(true) => self.state.set_notification("Reverted".to_string()),
            Ok(false) => {}
            Err(error) => self
                .state
                .set_error(Self::format_spreadsheet_edit_error(error)),
        }
    }

    /// Build one guided update plan for the dirty spreadsheet row.
    pub(crate) async fn save_pending_edits(&mut self) {
        if self.block_if_spreadsheet_edits_frozen("save again") {
            return;
        }
        if let Some(Err(error)) = self
            .state
            .edit_mode
            .as_ref()
            .map(|em| em.spreadsheet_edits().save_all())
        {
            match error {
                SpreadsheetEditError::MultiRowSaveAllDisabled => {}
                other => {
                    self.state
                        .set_error(Self::format_spreadsheet_edit_error(other));
                    return;
                }
            }
        }
        if self
            .state
            .edit_mode
            .as_ref()
            .and_then(|em| em.pending_single_row_save())
            .is_none()
        {
            self.state.set_notification("No pending edits".to_string());
            return;
        }
        self.open_spreadsheet_guided_save().await;
    }

    pub(crate) async fn open_spreadsheet_guided_save(&mut self) {
        let Some(dirty_target) = self
            .state
            .edit_mode
            .as_ref()
            .and_then(|em| em.spreadsheet_edits().dirty_target().cloned())
        else {
            self.state.set_notification("No pending edits".to_string());
            return;
        };
        let current_target = match self.edit_row_target_at(dirty_target.data_row_index) {
            Ok(target) => target,
            Err(reason) => {
                self.record_spreadsheet_definitely_not_sent(reason);
                return;
            }
        };
        let Some(edit_mode) = self.state.edit_mode.as_ref() else {
            self.state.set_notification("No pending edits".to_string());
            return;
        };
        let save = match edit_mode.pending_single_row_save_for_current_target(&current_target) {
            Ok(Some(save)) => save,
            Ok(None) => {
                self.state.set_notification("No pending edits".to_string());
                return;
            }
            Err(SpreadsheetEditError::DefinitelyNotSent { reason }) => {
                self.record_spreadsheet_definitely_not_sent(reason);
                return;
            }
            Err(error) => {
                self.state
                    .set_error(Self::format_spreadsheet_edit_error(error));
                return;
            }
        };
        let Some(table_info) = self.state.selected_table().cloned() else {
            self.record_spreadsheet_definitely_not_sent("No table selected".to_string());
            return;
        };
        let Some(qualified_table) = self.qualified_selected_table(&table_info) else {
            return;
        };
        if self.refuse_guided_write_if_unavailable(&qualified_table) {
            return;
        }
        let Some(row) = self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|result| result.rows.get(save.target.data_row_index))
            .cloned()
        else {
            self.record_spreadsheet_definitely_not_sent("No row selected".to_string());
            return;
        };
        let plan_id = self.next_write_plan_id();
        let changes: Vec<ColumnChange> = save
            .changes
            .into_iter()
            .map(|change| ColumnChange {
                column_id: change.column_id,
                old_value: change.old_value,
                new_value: change.new_value,
            })
            .collect();
        let plan = match build_guided_update_plan(
            plan_id,
            qualified_table,
            &table_info,
            &row,
            self.current_generations(),
            changes,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.record_spreadsheet_definitely_not_sent(Self::explain_write_plan_build_error(
                    error,
                ));
                return;
            }
        };
        let plan_id = plan.id;
        let prompt = format!(
            "{}\n\nPress [y] to confirm, [n] to cancel.",
            format_guided_update_confirmation(&plan)
        );
        self.write_plans.insert(plan_id, plan);
        self.spreadsheet_save_lifecycle =
            Some(SpreadsheetSaveLifecycle::AwaitingConfirmation(plan_id));
        if let Some(display_row) = self.display_row_for_data_idx(save.target.data_row_index) {
            self.tables_grid.selected_row = display_row;
        }
        self.state.modal = Some(crate::state::modal::Modal::confirm(
            "Confirm row update",
            prompt,
            crate::state::modal::ModalAction::Safety(
                crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
            ),
        ));
    }

    pub(crate) fn record_spreadsheet_definitely_not_sent(&mut self, reason: String) {
        self.spreadsheet_save_lifecycle = None;
        self.state.set_error(format_mutation_outcome_not_sent(
            MutationOutcome::DefinitelyNotSent { reason },
        ));
    }

    pub(crate) fn format_spreadsheet_edit_error(error: SpreadsheetEditError) -> String {
        match error {
            SpreadsheetEditError::MultiRowSaveAllDisabled => {
                "Saving all spreadsheet rows is disabled".to_string()
            }
            SpreadsheetEditError::DefinitelyNotSent { reason } => {
                format_mutation_outcome_not_sent(MutationOutcome::DefinitelyNotSent { reason })
            }
        }
    }
}
