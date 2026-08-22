//! Guided-write execution pipeline: modal action dispatch, plan
//! confirmation, background spawn helpers, and transport error classification.

use super::*;

impl super::App {
    /// Dispatch a finished modal action — runs the underlying API
    /// call on a background task and surfaces the result via
    /// `AppEvent::WriteOpSuccess` / `WriteOpError`. The modal is
    /// dropped (the caller already moved it out of `state.modal`).
    pub(crate) async fn dispatch_modal_action(&mut self, modal: crate::state::modal::Modal) {
        use crate::state::modal::{Modal, ModalAction, SafetyModalAction};

        let op_label = modal.action().op_label();

        match modal {
            Modal::Form {
                title,
                fields,
                focus,
                action,
            } => match action {
                ModalAction::CallReducer {
                    reducer,
                    param_types,
                } => {
                    let args: Vec<serde_json::Value> = fields
                        .iter()
                        .zip(param_types.iter())
                        .map(|(f, t)| coerce_field_to_json(&f.input.value, t))
                        .collect();
                    let Some(db) = self.selected_database_for_modal() else {
                        return;
                    };
                    self.spawn_call_reducer(db, reducer, args, op_label);
                }
                ModalAction::InsertRow {
                    table,
                    column_types,
                } => {
                    let columns: Vec<String> = fields
                        .iter()
                        .map(|f| extract_field_name(&f.label))
                        .collect();
                    let values: Vec<String> = fields
                        .iter()
                        .zip(column_types.iter())
                        .map(|(f, t)| sql_literal(&f.input.value, t))
                        .collect();
                    let sql = format!(
                        "INSERT INTO {table} ({}) VALUES ({})",
                        columns.join(", "),
                        values.join(", ")
                    );
                    let Some(db) = self.selected_database_for_modal() else {
                        return;
                    };
                    self.spawn_write_sql(db, sql, op_label);
                }
                ModalAction::DeleteDatabase { database } => {
                    // Typed-confirm: the user must type the database
                    // name verbatim into the single form field.
                    let typed = fields.first().map(|f| f.input.value.trim().to_string());
                    if typed.as_deref() != Some(database.as_str()) {
                        self.state
                            .set_error(format!("Type '{database}' exactly to confirm"));
                        return;
                    }
                    self.spawn_delete_database(database, op_label);
                }
                ModalAction::AddDatabaseAlias { database } => {
                    // Non-destructive: accept any non-empty alias
                    // and forward to the server. Validation (uniqueness,
                    // formatting) is the server's job.
                    let alias = fields
                        .first()
                        .map(|f| f.input.value.trim().to_string())
                        .unwrap_or_default();
                    if alias.is_empty() {
                        self.state
                            .set_notification("Alias cannot be empty".to_string());
                        return;
                    }
                    self.spawn_add_alias(database, alias, op_label);
                }
                ModalAction::TruncateTable { table } => {
                    // Same typed-confirm pattern as DeleteDatabase.
                    let typed = fields.first().map(|f| f.input.value.trim().to_string());
                    if typed.as_deref() != Some(table.as_str()) {
                        self.state
                            .set_error(format!("Type '{table}' exactly to confirm"));
                        return;
                    }
                    let sql = format!("DELETE FROM {table}");
                    let Some(db) = self.selected_database_for_modal() else {
                        return;
                    };
                    self.spawn_write_sql(db, sql, op_label);
                }
                ModalAction::DiscardPendingEdits => {
                    // DiscardPendingEdits is always a Confirm, never a Form.
                }
                ModalAction::Safety(SafetyModalAction::DirtyRowChoice {
                    requested_row,
                    requested_column,
                }) => {
                    self.submit_guided_update_form(
                        title,
                        fields,
                        focus,
                        requested_row,
                        requested_column,
                    );
                }
                ModalAction::Safety(SafetyModalAction::ConfirmWritePlan { .. }) => {
                    self.state
                        .set_error("Internal: ConfirmWritePlan inside a Form".to_string());
                }
                ModalAction::SpreadsheetDirtyRowChoice { .. } => {
                    self.state
                        .set_error("Internal: SpreadsheetDirtyRowChoice inside a Form".to_string());
                }
            },
            Modal::Confirm { action, .. } => match action {
                ModalAction::DiscardPendingEdits => {
                    // User confirmed leaving edit mode — drop the
                    // pending list and exit.
                    self.state.edit_mode = None;
                    self.state
                        .set_notification("Pending edits discarded".to_string());
                    self.flush_deferred_guided_refresh_if_unowned().await;
                }
                ModalAction::Safety(SafetyModalAction::ConfirmWritePlan { plan_id }) => {
                    self.dispatch_confirmed_write_plan(plan_id, op_label).await;
                }
                ModalAction::Safety(SafetyModalAction::DirtyRowChoice { .. }) => {
                    self.state
                        .set_error("Internal: DirtyRowChoice inside Confirm".to_string());
                }
                _ => {
                    self.state
                        .set_error("Internal: unsupported Confirm action".to_string());
                }
            },
        }
    }

    pub(crate) fn submit_guided_update_form(
        &mut self,
        title: String,
        fields: Vec<crate::state::modal::FormField>,
        focus: usize,
        requested_row: usize,
        requested_column: u32,
    ) {
        let Some(draft) = self.pending_update_draft.as_ref().cloned() else {
            self.state.set_error("No pending update draft".to_string());
            return;
        };
        if draft.requested_row != requested_row || draft.requested_column != requested_column {
            self.state
                .set_error("Update draft no longer matches the modal".to_string());
            return;
        }
        let changes = match build_guided_update_changes_from_form_fields(
            &draft.table_info,
            &draft.original_raw_row,
            &draft.original_form_values,
            &fields,
        ) {
            Ok(changes) => changes,
            Err(error) => {
                self.state.set_error(error);
                self.restore_guided_update_form(
                    title,
                    fields,
                    focus,
                    requested_row,
                    requested_column,
                );
                return;
            }
        };
        let plan = match build_guided_update_plan(
            draft.plan_id,
            draft.qualified_table.clone(),
            &draft.table_info,
            &draft.original_raw_row,
            draft.generations,
            changes,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                self.state.set_error(format!(
                    "Cannot update row: {}",
                    Self::explain_write_plan_build_error(error)
                ));
                self.restore_guided_update_form(
                    title,
                    fields,
                    focus,
                    requested_row,
                    requested_column,
                );
                return;
            }
        };
        let plan_id = plan.id;
        self.pending_update_draft = None;
        self.pending_update_confirmation_form = Some(PendingGuidedUpdateConfirmationForm {
            draft,
            title,
            fields,
            focus,
        });
        self.write_plans.remove(&plan_id);
        let prompt = format!(
            "{}\n\nPress [y] to confirm, [n] to cancel.",
            format_guided_update_confirmation(&plan)
        );
        self.write_plans.insert(plan_id, plan);
        self.state.modal = Some(crate::state::modal::Modal::confirm(
            "Confirm row update",
            prompt,
            crate::state::modal::ModalAction::Safety(
                crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
            ),
        ));
    }

    pub(crate) fn restore_guided_update_form(
        &mut self,
        title: String,
        fields: Vec<crate::state::modal::FormField>,
        focus: usize,
        requested_row: usize,
        requested_column: u32,
    ) {
        let action = crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::DirtyRowChoice {
                requested_row,
                requested_column,
            },
        );
        self.state.modal = Some(crate::state::modal::Modal::Form {
            title,
            fields,
            focus,
            action,
        });
    }

    pub(crate) fn restore_pending_update_confirmation_form(&mut self, plan_id: WritePlanId) {
        if let Some(snapshot) = self
            .pending_update_confirmation_form
            .as_ref()
            .filter(|snapshot| snapshot.draft.plan_id == plan_id)
            .cloned()
        {
            self.pending_update_draft = Some(snapshot.draft.clone());
            self.restore_guided_update_form(
                snapshot.title,
                snapshot.fields,
                snapshot.focus,
                snapshot.draft.requested_row,
                snapshot.draft.requested_column,
            );
        }
    }

    pub(crate) async fn dispatch_confirmed_write_plan(
        &mut self,
        plan_id: WritePlanId,
        op_label: String,
    ) {
        let Some(plan) = self.write_plans.remove(&plan_id) else {
            self.state
                .set_error("Write plan is no longer available; nothing sent".to_string());
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        };
        let clear_spreadsheet_marker = |app: &mut Self| {
            if app.spreadsheet_lifecycle_plan_id() == Some(plan_id) {
                app.spreadsheet_save_lifecycle = None;
            }
        };
        if self.guided_write_blocked_generation(&plan.table).is_some() {
            self.restore_guided_update_plan_after_local_rejection(plan.clone());
            clear_spreadsheet_marker(self);
            self.set_guided_write_unavailable_error(
                &plan.table,
                GuidedWriteUnavailable::RequiresManualRefresh,
            );
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        if self.guided_write_data_is_stale(&plan.table) {
            self.restore_guided_update_plan_after_local_rejection(plan.clone());
            clear_spreadsheet_marker(self);
            self.set_guided_write_unavailable_error(
                &plan.table,
                GuidedWriteUnavailable::StaleAfterVerifiedWrite,
            );
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        if self.state.schema_loading || self.state.query_loading {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state
                .set_error("Data is loading; write plan was not sent".to_string());
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        let Some(table_info) = self.state.selected_table().cloned() else {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state
                .set_error("No table selected; write plan was not sent".to_string());
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        };
        let Some(data_idx) = self.active_data_row_index() else {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state
                .set_error("No row selected; write plan was not sent".to_string());
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        };
        let Some(current_raw_row) = self
            .state
            .table_browse_result
            .as_ref()
            .and_then(|result| result.rows.get(data_idx))
            .cloned()
        else {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state
                .set_error("No row selected; write plan was not sent".to_string());
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        };
        let snapshot = ConfirmationSnapshot {
            generations: self.current_generations(),
            table: &table_info,
            current_raw_row: &current_raw_row,
            row_locked: self.guided_write_locks.is_locked(&plan),
        };
        if let Err(outcome) = revalidate_before_dispatch(&plan, &snapshot) {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state
                .set_error(format_mutation_outcome_not_sent(outcome));
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        let postcondition_queries = match plan_postcondition_queries(&plan, &table_info) {
            Ok(queries) => queries,
            Err(error) => {
                self.restore_guided_update_plan_after_local_rejection(plan);
                clear_spreadsheet_marker(self);
                self.state.set_error(format!(
                    "Postcondition verification planning failed; nothing sent: {error:?}"
                ));
                self.flush_deferred_guided_refresh_if_unowned().await;
                return;
            }
        };
        let sql = match &plan.mutation {
            GuidedMutation::Update { .. } => build_update_sql(&plan, &table_info),
            GuidedMutation::Delete => build_delete_sql(&plan, &table_info),
        };
        let sql = match sql {
            Ok(sql) => sql,
            Err(error) => {
                self.restore_guided_update_plan_after_local_rejection(plan);
                clear_spreadsheet_marker(self);
                self.state.set_error(format!(
                    "Write plan encoding failed; nothing sent: {error:?}"
                ));
                self.flush_deferred_guided_refresh_if_unowned().await;
                return;
            }
        };
        if !self.guided_write_locks.acquire(&plan) {
            self.restore_guided_update_plan_after_local_rejection(plan);
            clear_spreadsheet_marker(self);
            self.state.set_error(format_mutation_outcome_not_sent(
                MutationOutcome::DefinitelyNotSent {
                    reason: "row locked before confirmation".to_string(),
                },
            ));
            self.flush_deferred_guided_refresh_if_unowned().await;
            return;
        }
        if self
            .pending_update_confirmation_form
            .as_ref()
            .is_some_and(|snapshot| snapshot.draft.plan_id == plan_id)
        {
            self.pending_update_confirmation_form = None;
        }
        if self.spreadsheet_save_lifecycle
            == Some(SpreadsheetSaveLifecycle::AwaitingConfirmation(plan_id))
        {
            self.spreadsheet_save_lifecycle = Some(SpreadsheetSaveLifecycle::InFlight(plan_id));
        }
        let db = plan.table.database.clone();
        self.spawn_guided_write_sql(plan, table_info, postcondition_queries, db, sql, op_label);
    }

    /// Run `client.add_database_alias` on a background task. On
    /// success we re-fetch the database list so any new alias
    /// shows up in the sidebar immediately, and re-pull the name
    /// list for the currently selected DB.
    ///
    /// The `DatabaseCatalog` context is issued eagerly so that any
    /// earlier in-flight catalog fetch is superseded. If the alias
    /// operation fails, no `DatabasesLoaded` event is sent and the
    /// consumed context acts as an intentional cancellation: the
    /// previous in-flight result is dropped rather than applied
    /// against potentially changed state.
    pub(crate) fn spawn_add_alias(&mut self, database: String, alias: String, op_label: String) {
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context =
            self.next_request_context(crate::effects::request::RequestScope::DatabaseCatalog);
        self.task_registry.spawn("add alias", async move {
            match tokio::time::timeout(
                HTTP_REQUEST_TIMEOUT,
                client.add_database_alias(&database, &alias),
            )
            .await
            {
                Ok(Ok(())) => {
                    send_event(
                        &tx,
                        AppEvent::WriteOpSuccess {
                            op: op_label,
                            response: serde_json::json!({
                                "database": database,
                                "new_alias": alias,
                            }),
                        },
                    );
                    // Refresh the sidebar so the new alias can be
                    // discovered without a restart.
                    if let Ok(Ok(dbs)) =
                        tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.list_databases()).await
                    {
                        send_event(
                            &tx,
                            AppEvent::DatabasesLoaded {
                                context,
                                databases: dbs,
                            },
                        );
                    }
                }
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: "request timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Run `client.delete_database` on a background task. On success
    /// re-bootstraps the database list so the now-deleted entry
    /// disappears from the sidebar without a manual refresh.
    ///
    /// The `DatabaseCatalog` context is issued eagerly so that any
    /// earlier in-flight catalog fetch is superseded. If the delete
    /// operation fails, no `DatabasesLoaded` event is sent and the
    /// consumed context acts as an intentional cancellation: the
    /// previous in-flight result is dropped rather than applied
    /// against potentially changed state.
    pub(crate) fn spawn_delete_database(&mut self, database: String, op_label: String) {
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context =
            self.next_request_context(crate::effects::request::RequestScope::DatabaseCatalog);
        self.task_registry.spawn("delete database", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.delete_database(&database))
                .await
            {
                Ok(Ok(())) => {
                    send_event(
                        &tx,
                        AppEvent::WriteOpSuccess {
                            op: op_label,
                            response: serde_json::json!({"deleted": database}),
                        },
                    );
                    // Re-fetch the database list so the sidebar
                    // updates immediately. The bootstrap helper
                    // already does ping → list_databases under a
                    // timeout.
                    if let Ok(Ok(dbs)) =
                        tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.list_databases()).await
                    {
                        send_event(
                            &tx,
                            AppEvent::DatabasesLoaded {
                                context,
                                databases: dbs,
                            },
                        )
                    }
                }
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: "request timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Run `client.call_reducer` on a background task and route the
    /// outcome through `AppEvent::WriteOp{Success,Error}`.
    pub(crate) fn spawn_call_reducer(
        &mut self,
        db: String,
        reducer: String,
        args: Vec<serde_json::Value>,
        op_label: String,
    ) {
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        self.task_registry.spawn("call reducer", async move {
            match tokio::time::timeout(
                HTTP_REQUEST_TIMEOUT,
                client.call_reducer(&db, &reducer, &args),
            )
            .await
            {
                Ok(Ok(response)) => send_event(
                    &tx,
                    AppEvent::WriteOpSuccess {
                        op: op_label,
                        response,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: "request timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Run a write SQL statement (INSERT/UPDATE/DELETE) on a
    /// background task and route the outcome the same way reducer
    /// calls are.
    pub(crate) fn spawn_write_sql(&mut self, db: String, sql: String, op_label: String) {
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        self.task_registry.spawn("write sql", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql)).await {
                Ok(Ok(_result)) => send_event(
                    &tx,
                    AppEvent::WriteOpSuccess {
                        op: op_label,
                        response: serde_json::json!({"sql": sql}),
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::WriteOpError {
                        op: op_label,
                        error: "request timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Run a guided row write and carry the plan id back so the exact
    /// in-flight row lock can be released on either success or failure.
    pub(crate) fn spawn_guided_write_sql(
        &mut self,
        plan: WritePlan,
        table_info: TableInfo,
        postcondition_queries: PostconditionQueries,
        db: String,
        sql: String,
        op_label: String,
    ) {
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let plan_id = plan.id;
        self.task_registry.spawn("guided write", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql)).await {
                Ok(Ok(dml_result)) => {
                    let report = match build_postcondition_evidence(
                        &client,
                        &db,
                        &table_info,
                        postcondition_queries,
                    )
                    .await
                    {
                        Ok(postcondition) => {
                            verify_authoritative_mutation_result(&WriteVerificationRequest {
                                plan: plan.clone(),
                                affected_rows: None,
                                postcondition: Some(postcondition),
                            })
                        }
                        Err(error) => verification_report_from_postcondition_fetch_error(error),
                    };
                    let _discarded_dml_body = dml_result;
                    send_event(
                        &tx,
                        AppEvent::GuidedWriteVerification {
                            plan_id,
                            op: op_label,
                            report,
                        },
                    );
                }
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::GuidedWriteVerification {
                        plan_id,
                        op: op_label,
                        report: WriteVerificationReport {
                            outcome: crate::effects::write_ops::classify_mutation_error(
                                MutationDispatchStage::AfterTransportOwnership,
                                Self::classify_guided_write_sql_error(&e),
                            ),
                        },
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::GuidedWriteVerification {
                        plan_id,
                        op: op_label,
                        report: WriteVerificationReport {
                            outcome: crate::effects::write_ops::classify_mutation_error(
                                MutationDispatchStage::AfterTransportOwnership,
                                TransportFailure::Timeout,
                            ),
                        },
                    },
                ),
            }
        });
    }

    pub(crate) fn classify_guided_write_sql_error(error: &anyhow::Error) -> TransportFailure {
        for cause in error.chain() {
            if let Some(reqwest_error) = cause.downcast_ref::<reqwest::Error>() {
                if let Some(status) = reqwest_error.status() {
                    if status.is_server_error() {
                        return TransportFailure::Http5xx(status.as_u16());
                    }
                }
                if reqwest_error.is_timeout() {
                    return TransportFailure::Timeout;
                }
                if reqwest_error.is_connect() {
                    return TransportFailure::Disconnected;
                }
            }
        }

        let detail = format!("{error:#}");
        let lower = detail.to_ascii_lowercase();
        if let Some(status) = Self::parse_sql_query_http_status(&detail) {
            if (500..=599).contains(&status) {
                return TransportFailure::Http5xx(status);
            }
            return TransportFailure::Validation(detail);
        }
        if lower.contains("cancelled") || lower.contains("canceled") {
            return TransportFailure::Cancelled;
        }
        if lower.contains("timed out") || lower.contains("timeout") {
            return TransportFailure::Timeout;
        }
        if lower.contains("connection refused")
            || lower.contains("connection reset")
            || lower.contains("connection closed")
            || lower.contains("broken pipe")
            || lower.contains("connection aborted")
        {
            return TransportFailure::Disconnected;
        }
        TransportFailure::Validation(detail)
    }

    pub(crate) fn parse_sql_query_http_status(detail: &str) -> Option<u16> {
        let marker = "SQL query HTTP ";
        let start = detail.find(marker)? + marker.len();
        let digits: String = detail[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }
}
