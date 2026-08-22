//! Modal dialog lifecycle: opening forms/confirmations and routing keys.

use super::*;

impl super::App {
    // ── Modal dialogs (Faz 5: write operations) ──────────────────────────

    /// Drop every piece of state that belongs to one guided update
    /// confirmation: the draft, the restore snapshot, and the correlated write
    /// plan. Cancellation must clear all three together, otherwise a later
    /// same-row success can act on the leftovers.
    pub(crate) fn discard_guided_update_confirmation_state(&mut self, plan_id: WritePlanId) {
        self.write_plans.remove(&plan_id);
        if self
            .pending_update_confirmation_form
            .as_ref()
            .is_some_and(|snapshot| snapshot.draft.plan_id == plan_id)
        {
            self.pending_update_confirmation_form = None;
        }
        if self
            .pending_update_draft
            .as_ref()
            .is_some_and(|draft| draft.plan_id == plan_id)
        {
            self.pending_update_draft = None;
        }
    }

    pub(crate) fn discard_guided_state_for_modal(&mut self, modal: &crate::state::modal::Modal) {
        use crate::state::modal::{Modal, ModalAction, SafetyModalAction};

        match modal.action() {
            ModalAction::Safety(SafetyModalAction::ConfirmWritePlan { plan_id }) => {
                self.discard_guided_update_confirmation_state(*plan_id);
                if self.spreadsheet_lifecycle_plan_id() == Some(*plan_id)
                    && !self.spreadsheet_edits_are_frozen()
                {
                    self.spreadsheet_save_lifecycle = None;
                }
            }
            ModalAction::Safety(SafetyModalAction::DirtyRowChoice { .. }) => {
                if let Some(plan_id) = self
                    .pending_update_draft
                    .as_ref()
                    .map(|draft| draft.plan_id)
                {
                    // A restored form still owns the snapshot and the plan that
                    // were reinserted after a local rejection.
                    self.discard_guided_update_confirmation_state(plan_id);
                }
                self.pending_update_draft = None;
            }
            _ => {}
        }

        if matches!(modal, Modal::Form { .. }) {
            self.pending_update_draft = None;
        }
    }

    /// Route a key event into the active modal dialog. Called from
    /// `handle_key` when `state.modal.is_some()`.
    pub(crate) async fn handle_modal_key(&mut self, key: KeyEvent) {
        // Take the modal out of state so we can mutate its fields
        // freely without holding two borrows at the same time. We
        // put it back at the end unless the user accepted / cancelled.
        let Some(mut modal) = self.state.modal.take() else {
            return;
        };

        match &mut modal {
            crate::state::modal::Modal::Confirm { action, .. } => match key.code {
                KeyCode::Char('d') | KeyCode::Char('D')
                    if matches!(
                        action,
                        crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice { .. }
                    ) =>
                {
                    if let crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
                        requested_row,
                        requested_column,
                    } = action
                    {
                        self.handle_spreadsheet_dirty_row_choice(
                            crate::state::modal::DirtyRowChoice::Discard,
                            *requested_row,
                            *requested_column,
                        )
                        .await;
                    }
                    return;
                }
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                    if let crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
                        requested_row,
                        requested_column,
                    } = action
                    {
                        self.handle_spreadsheet_dirty_row_choice(
                            crate::state::modal::DirtyRowChoice::Save,
                            *requested_row,
                            *requested_column,
                        )
                        .await;
                    } else {
                        self.dispatch_modal_action(modal).await;
                    }
                    return;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    if let crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
                        requested_row,
                        requested_column,
                    } = action
                    {
                        self.handle_spreadsheet_dirty_row_choice(
                            crate::state::modal::DirtyRowChoice::Stay,
                            *requested_row,
                            *requested_column,
                        )
                        .await;
                        return;
                    }
                    // Cancelled — drop the modal entirely.
                    self.discard_guided_state_for_modal(&modal);
                    self.flush_deferred_guided_refresh_if_unowned().await;
                    return;
                }
                _ => {}
            },
            crate::state::modal::Modal::Form { fields, focus, .. } => match key.code {
                KeyCode::Esc => {
                    self.discard_guided_state_for_modal(&modal);
                    self.flush_deferred_guided_refresh_if_unowned().await;
                    return;
                }
                KeyCode::Enter => {
                    self.dispatch_modal_action(modal).await;
                    return;
                }
                KeyCode::Tab | KeyCode::Down if !fields.is_empty() => {
                    *focus = (*focus + 1) % fields.len();
                }
                KeyCode::BackTab | KeyCode::Up if !fields.is_empty() => {
                    *focus = if *focus == 0 {
                        fields.len() - 1
                    } else {
                        *focus - 1
                    };
                }
                KeyCode::Left => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.move_left();
                    }
                }
                KeyCode::Right => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.move_right();
                    }
                }
                KeyCode::Home => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.home();
                    }
                }
                KeyCode::End => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.end();
                    }
                }
                KeyCode::Backspace => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.backspace();
                    }
                }
                KeyCode::Delete => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.delete();
                    }
                }
                KeyCode::Char(ch) => {
                    if let Some(f) = fields.get_mut(*focus) {
                        f.input.insert(ch);
                    }
                }
                _ => {}
            },
        }

        // Key didn't trigger accept/cancel — put the modal back.
        self.state.modal = Some(modal);
    }

    pub(crate) async fn handle_spreadsheet_dirty_row_choice(
        &mut self,
        choice: crate::state::modal::DirtyRowChoice,
        requested_row: usize,
        requested_column: u32,
    ) {
        let matches_pending_request =
            self.pending_spreadsheet_edit_request
                .as_ref()
                .is_some_and(|request| {
                    request.target.data_row_index == requested_row
                        && self
                            .state
                            .selected_table()
                            .and_then(|table| table.columns.get(request.column_index))
                            .map(|column| column.col_id == requested_column)
                            .unwrap_or(false)
                });
        if !matches_pending_request {
            self.pending_spreadsheet_edit_request = None;
            if let Some(dirty_row) = self
                .state
                .edit_mode
                .as_ref()
                .and_then(|em| em.spreadsheet_edits().dirty_target())
                .and_then(|target| self.display_row_for_data_idx(target.data_row_index))
            {
                self.tables_grid.selected_row = dirty_row;
            }
            self.state.set_error(
                "Spreadsheet dirty-row choice no longer matches the pending edit".to_string(),
            );
            return;
        }
        match choice {
            crate::state::modal::DirtyRowChoice::Save => {
                self.pending_spreadsheet_edit_request = None;
                self.open_spreadsheet_guided_save().await;
            }
            crate::state::modal::DirtyRowChoice::Discard => {
                let request = self.pending_spreadsheet_edit_request.take();
                if let Some(em) = self.state.edit_mode.as_mut() {
                    em.discard_spreadsheet_edits();
                }
                if let Some(request) = request {
                    if let Some(display_row) =
                        self.display_row_for_data_idx(request.target.data_row_index)
                    {
                        self.tables_grid.selected_row = display_row;
                    }
                    self.tables_grid.selected_col = request.column_index;
                    self.begin_cell_edit();
                }
            }
            crate::state::modal::DirtyRowChoice::Stay => {
                self.pending_spreadsheet_edit_request = None;
                if let Some(dirty_row) = self
                    .state
                    .edit_mode
                    .as_ref()
                    .and_then(|em| em.spreadsheet_edits().dirty_target())
                    .and_then(|target| self.display_row_for_data_idx(target.data_row_index))
                {
                    self.tables_grid.selected_row = dirty_row;
                }
            }
        }
    }

    /// Open an edit form pre-filled with the currently selected row's
    /// values. The form itself carries only a safety action with the
    /// requested row/column; the selected raw row, table metadata, and
    /// generation snapshot stay in a private app draft.
    pub(crate) fn open_update_form(&mut self) {
        if self.state.schema_loading || self.state.query_loading {
            self.state
                .set_notification("Cannot update while table data is loading".to_string());
            return;
        }
        let Some(table) = self.state.selected_table().cloned() else {
            self.state.set_notification("No table selected".to_string());
            return;
        };
        if table.columns.is_empty() {
            self.state
                .set_error(format!("Table '{}' has no columns", table.table_name));
            return;
        }
        let data_idx = match self.active_data_row_index() {
            Some(i) => i,
            None => {
                self.state.set_notification("No row selected".to_string());
                return;
            }
        };
        let row = match self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|qr| qr.rows.get(data_idx))
        {
            Some(row) => row.clone(),
            None => {
                self.state.set_notification("No row selected".to_string());
                return;
            }
        };
        if let Err(error) = complete_primary_key_from_table_row(&table, &row) {
            self.state.set_error(format!(
                "Cannot update row: {}",
                Self::explain_primary_key_error(&table, error)
            ));
            return;
        }
        let Some(qualified_table) = self.qualified_selected_table(&table) else {
            return;
        };
        if self.refuse_guided_write_if_unavailable(&qualified_table) {
            return;
        }
        let generations = self.current_generations();
        let plan_id = self.next_write_plan_id();
        let requested_column = table
            .columns
            .get(self.tables_grid.selected_col)
            .map(|column| column.col_id)
            .unwrap_or_default();

        let mut original_form_values = Vec::with_capacity(table.columns.len());
        let fields: Vec<crate::state::modal::FormField> = table
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let type_label = type_tag(&column.col_type);
                let pk_marker = if table.primary_key_cols.contains(&(column.col_id as u16)) {
                    " — PK"
                } else {
                    ""
                };
                let mut field = crate::state::modal::FormField::new(format!(
                    "{} ({}{pk_marker})",
                    column.col_name, type_label
                ));
                let form_value = row
                    .get(index)
                    .and_then(|raw| guided_form_value_from_raw(column, raw).ok())
                    .unwrap_or(GuidedFormValue {
                        input: row
                            .get(index)
                            .map_or_else(String::new, serde_json::Value::to_string),
                        typed_value: crate::state::safety::SqlValue::Unrepresentable(
                            row.get(index)
                                .map_or_else(String::new, serde_json::Value::to_string),
                        ),
                    });
                field.input.set(form_value.input.clone());
                original_form_values.push(form_value);
                field
            })
            .collect();

        self.pending_update_draft = Some(PendingGuidedUpdateDraft {
            plan_id,
            qualified_table,
            table_info: table.clone(),
            original_raw_row: row,
            generations,
            original_form_values,
            requested_row: data_idx,
            requested_column,
        });

        let action = crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::DirtyRowChoice {
                requested_row: data_idx,
                requested_column,
            },
        );
        self.state.modal = Some(crate::state::modal::Modal::form(
            format!("Update row in {}", table.table_name),
            fields,
            action,
        ));
    }

    /// Open a one-field form to attach a new alias / human name to
    /// the currently selected database. Non-destructive — no typed
    /// confirm required, just a non-empty string.
    pub(crate) fn open_add_alias_form(&mut self) {
        let Some(db_name) = self.state.selected_database().map(str::to_string) else {
            self.state
                .set_notification("No database selected".to_string());
            return;
        };
        let field =
            crate::state::modal::FormField::new("New alias").with_placeholder("e.g. my-app-prod");
        let action = crate::state::modal::ModalAction::AddDatabaseAlias {
            database: db_name.clone(),
        };
        self.state.modal = Some(crate::state::modal::Modal::form(
            format!("Add alias to {db_name}"),
            vec![field],
            action,
        ));
    }

    /// Shared constructor for the "type the name to confirm" modal
    /// flow used by destructive admin ops. The dispatcher checks
    /// `fields[0].input.value == expected` before running `action`.
    pub(crate) fn open_typed_confirm_form(
        &mut self,
        title: impl Into<String>,
        expected: &str,
        verb: &str,
        action: crate::state::modal::ModalAction,
    ) {
        let field =
            crate::state::modal::FormField::new(format!("Type '{expected}' to confirm {verb}"))
                .with_placeholder("required");
        self.state.modal = Some(crate::state::modal::Modal::form(
            title.into(),
            vec![field],
            action,
        ));
    }

    /// Open a typed-confirm form to permanently delete the currently
    /// selected database. The user has to type the database name
    /// verbatim — a plain `[y/n]` would be too easy to trigger by
    /// accident and the operation is irreversible.
    pub(crate) fn open_delete_db_form(&mut self) {
        let Some(db_name) = self.state.selected_database().map(str::to_string) else {
            self.state
                .set_notification("No database selected".to_string());
            return;
        };
        self.open_typed_confirm_form(
            format!("⚠ DELETE DATABASE {db_name}"),
            &db_name.clone(),
            "DELETE",
            crate::state::modal::ModalAction::DeleteDatabase { database: db_name },
        );
    }

    /// Open a typed-confirm form to delete every row from the
    /// currently selected table. SpacetimeDB tables are part of
    /// the published module schema and cannot be `DROP`ped via SQL,
    /// so this is the closest "delete the table" operation we can
    /// safely offer from a TUI session — it issues `DELETE FROM
    /// <table>` once the user has typed the table name back to us.
    pub(crate) fn open_truncate_table_form(&mut self) {
        let Some(table) = self.state.selected_table().map(|t| t.table_name.clone()) else {
            self.state.set_notification("No table selected".to_string());
            return;
        };
        self.open_typed_confirm_form(
            format!("⚠ TRUNCATE TABLE {table}"),
            &table.clone(),
            "TRUNCATE",
            crate::state::modal::ModalAction::TruncateTable { table },
        );
    }

    /// Open a confirm dialog to delete the currently selected row.
    /// Builds and stores a pure guided write plan immediately; the modal
    /// carries only the stable plan ID.
    pub(crate) fn open_delete_confirm(&mut self) {
        if self.state.schema_loading || self.state.query_loading {
            self.state
                .set_notification("Cannot delete while table data is loading".to_string());
            return;
        }
        let Some(table) = self.state.selected_table().cloned() else {
            self.state.set_notification("No table selected".to_string());
            return;
        };
        if table.columns.is_empty() {
            self.state
                .set_error(format!("Table '{}' has no columns", table.table_name));
            return;
        }

        let data_idx = match self.active_data_row_index() {
            Some(i) => i,
            None => {
                self.state.set_notification("No row selected".to_string());
                return;
            }
        };
        let row = match self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|qr| qr.rows.get(data_idx))
        {
            Some(row) => row.clone(),
            None => {
                self.state.set_notification("No row selected".to_string());
                return;
            }
        };
        let Some(qualified_table) = self.qualified_selected_table(&table) else {
            return;
        };
        if self.refuse_guided_write_if_unavailable(&qualified_table) {
            return;
        }
        let plan_id = self.next_write_plan_id();
        let plan = match build_guided_delete_plan(
            plan_id,
            qualified_table,
            &table,
            &row,
            self.current_generations(),
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.state.set_error(format!(
                    "Cannot delete row: {}",
                    Self::explain_write_plan_build_error(error)
                ));
                return;
            }
        };
        let prompt = format!(
            "{}\n\nPress [y] to confirm, [n] to cancel.",
            format_guided_delete_confirmation(&plan, &row)
        );
        self.write_plans.insert(plan_id, plan);
        let action = crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        );
        self.state.modal = Some(crate::state::modal::Modal::confirm(
            format!("Delete row from {}", table.table_name),
            prompt,
            action,
        ));
    }

    /// Open an insert form for the currently selected table in the
    /// Tables tab. Each user-visible column gets one form field. The
    /// submit handler builds an `INSERT INTO ... VALUES (...)` SQL
    /// statement and runs it via [`spawn_write_sql`].
    pub(crate) fn open_insert_form(&mut self) {
        let Some(table) = self.state.selected_table().cloned() else {
            self.state.set_notification("No table selected".to_string());
            return;
        };

        if table.columns.is_empty() {
            self.state
                .set_error(format!("Table '{}' has no columns", table.table_name));
            return;
        }

        let fields: Vec<crate::state::modal::FormField> = table
            .columns
            .iter()
            .map(|c| {
                let type_label = type_tag(&c.col_type);
                let auto = if c.is_autoinc { " — auto" } else { "" };
                crate::state::modal::FormField::new(format!(
                    "{} ({}{auto})",
                    c.col_name, type_label
                ))
                .with_placeholder(default_placeholder_for_type(&type_label))
            })
            .collect();

        let column_types: Vec<String> = table
            .columns
            .iter()
            .map(|c| type_tag(&c.col_type))
            .collect();

        let action = crate::state::modal::ModalAction::InsertRow {
            table: table.table_name.clone(),
            column_types,
        };

        self.state.modal = Some(crate::state::modal::Modal::form(
            format!("Insert into {}", table.table_name),
            fields,
            action,
        ));
    }

    /// Open a reducer-call form for the currently selected reducer in
    /// the Module tab. No-op if there is no schema or selection.
    pub(crate) fn open_reducer_form(&mut self) {
        let Some(schema) = self.state.current_schema.as_ref() else {
            return;
        };
        let Some(reducer) = schema.reducers.get(self.state.module_selected_reducer) else {
            return;
        };

        let fields: Vec<crate::state::modal::FormField> = reducer
            .params
            .iter()
            .map(|p| {
                let type_label = type_tag(&p.algebraic_type);
                crate::state::modal::FormField::new(format!("{} ({})", p.name, type_label))
                    .with_placeholder(default_placeholder_for_type(&type_label))
            })
            .collect();

        let action = crate::state::modal::ModalAction::CallReducer {
            reducer: reducer.name.clone(),
            param_types: reducer
                .params
                .iter()
                .map(|p| type_tag(&p.algebraic_type))
                .collect(),
        };

        let title = if reducer.params.is_empty() {
            format!("Call {} (no args — Enter to confirm)", reducer.name)
        } else {
            format!("Call {}", reducer.name)
        };

        self.state.modal = Some(crate::state::modal::Modal::form(title, fields, action));
    }
}
