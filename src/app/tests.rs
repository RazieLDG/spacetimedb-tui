use super::*;

use crate::api::types::{ColumnInfo, TableInfo};

use guided_write::{GuidedWriteTarget, PostconditionFetchError};

fn make_table(name: &str, cols: Vec<ColumnInfo>) -> TableInfo {
    TableInfo {
        table_name: name.to_string(),
        product_type_ref: 0,
        table_type: "user".to_string(),
        table_access: "public".to_string(),
        columns: cols,
        primary_key_cols: vec![],
        indexes: vec![],
        constraints: vec![],
    }
}

fn make_table_with_pk(name: &str, cols: Vec<ColumnInfo>, pk_col_ids: Vec<u16>) -> TableInfo {
    let mut table = make_table(name, cols);
    table.primary_key_cols = pk_col_ids;
    table
}

#[test]
fn type_tag_handles_string_and_object_forms() {
    assert_eq!(type_tag(&serde_json::json!("String")), "String");
    assert_eq!(type_tag(&serde_json::json!({"U64": []})), "U64");
    assert_eq!(type_tag(&serde_json::json!(null)), "Unknown");
}

#[test]
fn guided_update_form_changes_skip_unchanged_display_text_and_parse_only_changed_fields() {
    use crate::state::modal::FormField;
    use crate::state::safety::{ColumnChange, SqlValue};

    fn typed_col(id: u32, name: &str, type_name: &str) -> ColumnInfo {
        ColumnInfo {
            col_id: id,
            col_name: name.to_string(),
            col_type: serde_json::json!({ "AlgebraicType": { type_name: {} } }),
            is_autoinc: false,
        }
    }

    let table = make_table_with_pk(
        "sessions",
        vec![
            typed_col(1, "id", "Identity"),
            typed_col(2, "connection", "ConnectionId"),
            typed_col(3, "visits", "U64"),
            typed_col(4, "name", "String"),
        ],
        vec![1],
    );
    let raw_row = vec![
        serde_json::json!({ "__identity__": "1234" }),
        serde_json::json!({ "__connection_id__": "abcd" }),
        serde_json::json!(5),
        serde_json::json!("Ada"),
    ];
    let mut original_form_values = Vec::new();
    let mut fields: Vec<FormField> = table
        .columns
        .iter()
        .enumerate()
        .map(|(idx, column)| {
            let mut field = FormField::new(format!(
                "{} ({})",
                column.col_name,
                type_tag(&column.col_type)
            ));
            let form_value = guided_form_value_from_raw(column, &raw_row[idx]).unwrap();
            field.input.set(form_value.input.clone());
            original_form_values.push(form_value);
            field
        })
        .collect();
    fields[2].input.set("6".to_string());

    let changes = build_guided_update_changes_from_form_fields(
        &table,
        &raw_row,
        &original_form_values,
        &fields,
    )
    .expect("changed U64 field should parse without touching unchanged hex fields");

    assert_eq!(
        changes,
        vec![ColumnChange {
            column_id: 3,
            old_value: Some(SqlValue::U64(5)),
            new_value: SqlValue::U64(6),
        }]
    );
}

#[test]
fn guided_update_form_skips_unchanged_null_non_pk_while_supported_field_changes() {
    use crate::state::modal::FormField;
    use crate::state::safety::{ColumnChange, SqlValue};

    let table = make_table_with_pk(
        "sessions",
        vec![
            typed_col_at(1, "id", "U64"),
            typed_col_at(2, "nickname", "String"),
            typed_col_at(3, "visits", "U64"),
        ],
        vec![1],
    );
    let raw_row = vec![
        serde_json::json!(1),
        serde_json::Value::Null,
        serde_json::json!(5),
    ];
    let mut original_form_values = Vec::new();
    let mut fields: Vec<FormField> = table
        .columns
        .iter()
        .enumerate()
        .map(|(idx, column)| {
            let mut field = FormField::new(column.col_name.clone());
            let form_value = raw_row
                .get(idx)
                .and_then(|raw| guided_form_value_from_raw(column, raw).ok())
                .unwrap_or(GuidedFormValue {
                    input: raw_row[idx].to_string(),
                    typed_value: SqlValue::Unrepresentable(raw_row[idx].to_string()),
                });
            field.input.set(form_value.input.clone());
            original_form_values.push(form_value);
            field
        })
        .collect();
    fields[2].input.set("6".to_string());

    let changes = build_guided_update_changes_from_form_fields(
        &table,
        &raw_row,
        &original_form_values,
        &fields,
    )
    .expect("unchanged null non-PK should be skipped");

    assert_eq!(
        changes,
        vec![ColumnChange {
            column_id: 3,
            old_value: Some(SqlValue::U64(5)),
            new_value: SqlValue::U64(6),
        }]
    );
}

#[test]
fn guided_update_form_skips_unchanged_opaque_non_pk_while_supported_field_changes() {
    use crate::state::modal::FormField;
    use crate::state::safety::{ColumnChange, SqlValue};

    let table = make_table_with_pk(
        "sessions",
        vec![
            typed_col_at(1, "id", "U64"),
            typed_col_at(2, "metadata", "String"),
            typed_col_at(3, "visits", "U64"),
        ],
        vec![1],
    );
    let raw_row = vec![
        serde_json::json!(1),
        serde_json::json!({"opaque": {"nested": true}}),
        serde_json::json!(5),
    ];
    let mut original_form_values = Vec::new();
    let mut fields: Vec<FormField> = table
        .columns
        .iter()
        .enumerate()
        .map(|(idx, column)| {
            let mut field = FormField::new(column.col_name.clone());
            let form_value = raw_row
                .get(idx)
                .and_then(|raw| guided_form_value_from_raw(column, raw).ok())
                .unwrap_or(GuidedFormValue {
                    input: raw_row[idx].to_string(),
                    typed_value: SqlValue::Unrepresentable(raw_row[idx].to_string()),
                });
            field.input.set(form_value.input.clone());
            original_form_values.push(form_value);
            field
        })
        .collect();
    fields[2].input.set("6".to_string());

    let changes = build_guided_update_changes_from_form_fields(
        &table,
        &raw_row,
        &original_form_values,
        &fields,
    )
    .expect("unchanged opaque non-PK should be skipped");

    assert_eq!(
        changes,
        vec![ColumnChange {
            column_id: 3,
            old_value: Some(SqlValue::U64(5)),
            new_value: SqlValue::U64(6),
        }]
    );
}

#[test]
fn guided_update_form_rejects_changed_opaque_non_pk_clearly() {
    use crate::state::modal::FormField;
    use crate::state::safety::SqlValue;

    let table = make_table_with_pk(
        "sessions",
        vec![
            typed_col_at(1, "id", "U64"),
            typed_col_at(2, "metadata", "String"),
        ],
        vec![1],
    );
    let raw_row = vec![serde_json::json!(1), serde_json::json!({"opaque": true})];
    let original_form_values = vec![
        guided_form_value_from_raw(&table.columns[0], &raw_row[0]).unwrap(),
        GuidedFormValue {
            input: raw_row[1].to_string(),
            typed_value: SqlValue::Unrepresentable(raw_row[1].to_string()),
        },
    ];
    let mut fields = vec![FormField::new("id"), FormField::new("metadata")];
    fields[0].input.set(original_form_values[0].input.clone());
    fields[1].input.set("edited".to_string());

    let error = build_guided_update_changes_from_form_fields(
        &table,
        &raw_row,
        &original_form_values,
        &fields,
    )
    .unwrap_err();

    assert!(error.contains("metadata"));
    assert!(error.contains("unsupported") || error.contains("unrepresentable"));
}

#[test]
fn guided_update_form_unchanged_values_never_fabricate_changes() {
    use crate::state::modal::FormField;
    use crate::state::safety::SqlValue;

    let table = make_table_with_pk(
        "sessions",
        vec![
            typed_col_at(1, "id", "U64"),
            typed_col_at(2, "metadata", "String"),
        ],
        vec![1],
    );
    let raw_row = vec![serde_json::json!(1), serde_json::json!({"opaque": true})];
    let original_form_values = vec![
        guided_form_value_from_raw(&table.columns[0], &raw_row[0]).unwrap(),
        GuidedFormValue {
            input: raw_row[1].to_string(),
            typed_value: SqlValue::Unrepresentable(raw_row[1].to_string()),
        },
    ];
    let mut fields = vec![FormField::new("id"), FormField::new("metadata")];
    fields[0].input.set(original_form_values[0].input.clone());
    fields[1].input.set(original_form_values[1].input.clone());

    let changes = build_guided_update_changes_from_form_fields(
        &table,
        &raw_row,
        &original_form_values,
        &fields,
    )
    .expect("unchanged unrepresentable values should be ignored");

    assert!(changes.is_empty());
}

fn typed_col_at(id: u32, name: &str, type_name: &str) -> ColumnInfo {
    ColumnInfo {
        col_id: id,
        col_name: name.to_string(),
        col_type: serde_json::json!({ "AlgebraicType": { type_name: {} } }),
        is_autoinc: false,
    }
}

fn test_app() -> App {
    let config = crate::config::Config {
        server_url: "http://localhost:3000".to_string(),
        ws_url: "ws://localhost:3000".to_string(),
        database: None,
        auth_token: None,
        theme: crate::config::ThemeColors::dark(),
        theme_name: crate::config::ThemeName::Dark,
        log_level: "off".to_string(),
        user_config: crate::user_config::UserConfig::default(),
    };
    let client = crate::api::client::SpacetimeClient::new(config.server_url.clone(), None)
        .expect("test client should construct");
    App::new(&config, client)
}

fn schema_with_table(table_name: &str) -> crate::api::types::SchemaResponse {
    crate::api::types::SchemaResponse {
        typespace: serde_json::json!({}),
        tables: vec![make_table_with_pk(
            table_name,
            vec![
                typed_col_at(1, "id", "U64"),
                typed_col_at(2, "name", "String"),
            ],
            vec![1],
        )],
        reducers: vec![],
    }
}

#[test]
fn open_update_form_refuses_invalid_primary_key_before_creating_draft_or_modal() {
    let mut app = test_app();
    app.state.databases = vec!["db".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = schema_with_table("users").tables;
    app.state.selected_table_idx = Some(0);
    app.state.current_tab = Tab::Tables;
    app.state.table_browse_result = Some(crate::api::types::QueryResult {
        schema: vec![
            crate::api::types::SchemaElement {
                name: "id".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "U64": {} } }),
            },
            crate::api::types::SchemaElement {
                name: "name".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "String": {} } }),
            },
        ],
        rows: vec![vec![serde_json::Value::Null, serde_json::json!("Ada")]],
        total_duration_micros: 10,
    });

    app.open_update_form();

    assert!(app.state.modal.is_none());
    assert!(app.pending_update_draft.is_none());
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("Cannot update row: primary key column 'id' is null")
    );
}

fn guided_success_event(plan_id: WritePlanId, op: &str) -> AppEvent {
    AppEvent::GuidedWriteVerification {
        plan_id,
        op: op.to_string(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::SentAndConfirmed {
                affected_rows: None,
            },
        },
    }
}

fn guided_unknown_outcome_event(plan_id: WritePlanId, op: &str, reason: &str) -> AppEvent {
    AppEvent::GuidedWriteVerification {
        plan_id,
        op: op.to_string(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::Unknown {
                reason: reason.to_string(),
            },
        },
    }
}

fn query_result(name: &str) -> crate::api::types::QueryResult {
    crate::api::types::QueryResult {
        schema: vec![
            crate::api::types::SchemaElement {
                name: "id".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "U64": {} } }),
            },
            crate::api::types::SchemaElement {
                name: "name".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "String": {} } }),
            },
        ],
        rows: vec![vec![serde_json::json!(1), serde_json::json!(name)]],
        total_duration_micros: 10,
    }
}

fn query_result_two_rows() -> crate::api::types::QueryResult {
    crate::api::types::QueryResult {
        schema: vec![
            crate::api::types::SchemaElement {
                name: "id".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "U64": {} } }),
            },
            crate::api::types::SchemaElement {
                name: "name".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "String": {} } }),
            },
            crate::api::types::SchemaElement {
                name: "score".to_string(),
                algebraic_type: serde_json::json!({ "AlgebraicType": { "U64": {} } }),
            },
        ],
        rows: vec![
            vec![
                serde_json::json!(1),
                serde_json::json!("Ada"),
                serde_json::json!(5),
            ],
            vec![
                serde_json::json!(2),
                serde_json::json!("Bob"),
                serde_json::json!(6),
            ],
        ],
        total_duration_micros: 10,
    }
}

fn spreadsheet_app() -> App {
    let mut app = test_app();
    app.state.databases = vec!["db".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = vec![make_table_with_pk(
        "users",
        vec![
            typed_col_at(511, "id", "U64"),
            typed_col_at(700, "name", "String"),
            typed_col_at(900, "score", "U64"),
        ],
        vec![511],
    )];
    app.state.selected_table_idx = Some(0);
    app.state.current_tab = Tab::Tables;
    app.state.table_browse_result = Some(query_result_two_rows());
    app.state.edit_mode = Some(crate::state::edit_mode::EditMode::new());
    app
}

#[test]
fn spreadsheet_declared_pk_is_read_only_and_no_pk_refuses_before_editor() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 0;
    app.begin_cell_edit();
    assert!(app.state.edit_mode.as_ref().unwrap().editor.is_none());
    assert_eq!(
        app.state
            .notification
            .as_ref()
            .map(|(message, _)| message.as_str()),
        Some("PK column is read-only in edit mode")
    );

    app.state.tables[0].primary_key_cols.clear();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    assert!(app.state.edit_mode.as_ref().unwrap().editor.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("missing declared primary key"));
}

#[tokio::test]
async fn spreadsheet_different_row_modal_choices_and_save_plan_lifecycle() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();

    app.tables_grid.selected_row = 1;
    app.tables_grid.selected_col = 2;
    app.begin_cell_edit();
    let modal = app.state.modal.as_ref().expect("dirty row choice modal");
    match modal.action() {
        crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
            requested_row,
            requested_column,
        } => {
            assert_eq!((*requested_row, *requested_column), (1, 900));
        }
        other => panic!("unexpected modal action: {other:?}"),
    }

    app.handle_spreadsheet_dirty_row_choice(crate::state::modal::DirtyRowChoice::Stay, 1, 900)
        .await;
    assert_eq!(app.tables_grid.selected_row, 0);
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);

    app.tables_grid.selected_row = 1;
    app.begin_cell_edit();
    app.handle_spreadsheet_dirty_row_choice(crate::state::modal::DirtyRowChoice::Discard, 1, 900)
        .await;
    assert_eq!(app.tables_grid.selected_row, 1);
    assert!(app.state.edit_mode.as_ref().unwrap().editor.is_some());
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 0);

    app.state.edit_mode.as_mut().unwrap().editor = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Ada2".to_string());
    app.commit_cell_edit();
    app.tables_grid.selected_col = 2;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("7".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app
        .spreadsheet_lifecycle_plan_id()
        .expect("spreadsheet-origin plan");
    let plan = app.write_plans.get(&plan_id).expect("write plan stored");
    match &plan.mutation {
        GuidedMutation::Update { changes } => {
            assert_eq!(
                changes.iter().map(|c| c.column_id).collect::<Vec<_>>(),
                vec![700, 900]
            );
        }
        _ => panic!("expected guided update plan"),
    }
    assert!(
        app.state.edit_mode.is_some(),
        "save keeps edits until success"
    );
    let modal = app.state.modal.clone().unwrap();
    app.discard_guided_state_for_modal(&modal);
    assert!(
        app.state.edit_mode.is_some(),
        "cancel preserves spreadsheet edits"
    );
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());

    let plan = app
        .write_plans
        .get(&plan_id)
        .cloned()
        .unwrap_or_else(|| make_write_plan(plan_id, "db", None, "users", 1, true));
    let _ = app.guided_write_locks.acquire(&plan);
    app.spreadsheet_save_lifecycle = Some(SpreadsheetSaveLifecycle::InFlight(plan_id));
    app.handle_app_event(guided_unknown_outcome_event(plan_id, "edit", "nope"))
        .await;
    assert!(app.state.edit_mode.is_some());
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    let plan = app
        .write_plans
        .get(&plan_id)
        .cloned()
        .unwrap_or_else(|| make_write_plan(plan_id, "db", None, "users", 1, true));
    let _ = app.guided_write_locks.acquire(&plan);
    app.spreadsheet_save_lifecycle = Some(SpreadsheetSaveLifecycle::InFlight(plan_id));
    app.handle_app_event(guided_success_event(plan_id, "edit"))
        .await;
    assert!(app.state.edit_mode.is_none());
}

#[tokio::test]
async fn spreadsheet_stale_generation_creates_no_plan_and_selection_invalidation_clears_state() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.table_generation += 1;
    app.save_pending_edits().await;
    assert!(app.write_plans.is_empty());
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("Write plan was not sent: row generation changed before save")
    );

    app.pending_spreadsheet_edit_request = Some(PendingSpreadsheetEditRequest {
        target: app.edit_row_target_at(0).unwrap(),
        column_index: 1,
    });
    app.clear_table_browse_for_selection_change();
    assert!(app.state.table_browse_result.is_none());
    assert!(app.state.edit_mode.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
}

#[tokio::test]
async fn spreadsheet_table_generation_invalidation_clears_non_inflight_spreadsheet_modal_state_only(
) {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app
        .spreadsheet_lifecycle_plan_id()
        .expect("awaiting spreadsheet confirmation");
    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::AwaitingConfirmation(plan_id))
    );
    assert!(matches!(
        app.state.modal.as_ref().map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id: modal_plan }
        )) if *modal_plan == plan_id
    ));

    app.bump_table_generation();

    assert!(app.state.edit_mode.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(!app.write_plans.contains_key(&plan_id));
    assert!(
        app.state.modal.is_none(),
        "stale spreadsheet confirm modal cleared"
    );

    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.pending_spreadsheet_edit_request = Some(PendingSpreadsheetEditRequest {
        target: app.edit_row_target_at(1).unwrap(),
        column_index: 2,
    });
    app.state.modal = Some(crate::state::modal::Modal::confirm(
        "Dirty row",
        "Choose what to do",
        crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice {
            requested_row: 1,
            requested_column: 900,
        },
    ));

    app.bump_table_generation();

    assert!(app.state.edit_mode.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(
        app.state.modal.is_none(),
        "stale spreadsheet dirty-row modal cleared"
    );

    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.state.modal = Some(crate::state::modal::Modal::confirm(
        "Unrelated",
        "Keep me",
        crate::state::modal::ModalAction::AddDatabaseAlias {
            database: "db".to_string(),
        },
    ));

    app.bump_table_generation();

    assert!(matches!(
        app.state.modal.as_ref().map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::AddDatabaseAlias { database })
            if database == "db"
    ));
}

#[tokio::test]
async fn spreadsheet_inflight_table_generation_invalidation_preserves_until_matching_result() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();
    app.dispatch_confirmed_write_plan(plan_id, "edit".to_string())
        .await;

    app.bump_table_generation();

    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::InFlightInvalidated(plan_id))
    );
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    assert!(app
        .guided_write_locks
        .targets_by_plan_id
        .contains_key(&plan_id));

    app.handle_app_event(guided_unknown_outcome_event(plan_id, "edit", "nope"))
        .await;

    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(app.state.edit_mode.is_none());
    assert!(!app
        .guided_write_locks
        .targets_by_plan_id
        .contains_key(&plan_id));

    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();
    app.dispatch_confirmed_write_plan(plan_id, "edit".to_string())
        .await;

    app.bump_table_generation();
    app.handle_app_event(guided_success_event(plan_id, "edit"))
        .await;

    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(app.state.edit_mode.is_none());
    assert!(!app
        .guided_write_locks
        .targets_by_plan_id
        .contains_key(&plan_id));
}

#[tokio::test]
async fn spreadsheet_stale_guided_success_does_not_refresh_or_mutate_active_spreadsheet_state() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_a = app.spreadsheet_lifecycle_plan_id().unwrap();
    let mut plan_b = app
        .write_plans
        .get(&plan_a)
        .expect("plan A awaiting confirmation")
        .clone();
    app.dispatch_confirmed_write_plan(plan_a, "edit A".to_string())
        .await;
    app.state.edit_mode = None;

    let plan_b_id = WritePlanId(plan_a.0 + 100);
    plan_b.id = plan_b_id;
    plan_b.original_primary_key =
        crate::state::safety::CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(2),
            },
        ])
        .expect("test primary key");
    app.write_plans.insert(plan_b_id, plan_b.clone());
    app.tables_grid.selected_row = 1;
    app.tables_grid.selected_col = 1;
    let target_b = app.edit_row_target_at(1).expect("row B target");
    let mut edit_mode = crate::state::edit_mode::EditMode::new();
    edit_mode
        .set_cell(
            target_b,
            crate::state::edit_mode::EditCellProjection {
                data_row: 1,
                column_index: 1,
                original_display: "Bob".to_string(),
                new_display: "Bobby".to_string(),
            },
            crate::state::edit_mode::EditCellValues {
                column_id: 700,
                old_value: Some(crate::state::safety::SqlValue::Text("Bob".to_string())),
                new_value: crate::state::safety::SqlValue::Text("Bobby".to_string()),
            },
        )
        .expect("seed B edit");
    app.state.edit_mode = Some(edit_mode);
    assert!(app.guided_write_locks.acquire(&plan_b));
    app.spreadsheet_save_lifecycle = Some(SpreadsheetSaveLifecycle::InFlight(plan_b_id));
    app.state.modal = Some(crate::state::modal::Modal::confirm(
        "Confirm B",
        "B is active",
        crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id: plan_b_id },
        ),
    ));
    app.state.query_loading = true;
    app.state.notification = None;

    app.handle_app_event(guided_success_event(plan_a, "edit A"))
        .await;

    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::InFlight(plan_b_id))
    );
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    assert!(matches!(
        app.state.modal.as_ref().map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id }
        )) if *plan_id == plan_b_id
    ));
    assert!(
        app.state.query_loading,
        "guided success must preserve unrelated loading state"
    );
    assert!(
        app.state
            .notification
            .as_ref()
            .is_some_and(|(message, _)| message.contains("✓ edit A")),
        "owned unrelated success must surface its success notification"
    );
    assert!(
        app.event_rx.try_recv().is_err(),
        "stale success must not refresh the table"
    );
}

#[tokio::test]
async fn guided_success_defers_refresh_while_unrelated_guided_update_form_owns_current_table() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_b_id = app
        .pending_update_draft
        .as_ref()
        .expect("guided update form draft")
        .plan_id;
    let draft_b_generation = app
        .pending_update_draft
        .as_ref()
        .expect("guided update form draft")
        .generations
        .row;
    let modal = app.state.modal.as_mut().expect("guided update form modal");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided update form");
    };
    fields[1].input.set("Alicia".to_string());

    let mut plan_a = make_write_plan(
        WritePlanId(draft_b_id.0 + 100),
        "db",
        None,
        "users",
        2,
        true,
    );
    plan_a.original_primary_key =
        crate::state::safety::CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(2),
            },
        ])
        .expect("plan A primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    app.state.query_loading = true;
    app.state.notification = None;
    let generation_before = app.table_generation;

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert_eq!(app.table_generation, generation_before);
    assert_eq!(
        app.pending_update_draft.as_ref().map(|draft| draft.plan_id),
        Some(draft_b_id)
    );
    assert_eq!(
        app.pending_update_draft
            .as_ref()
            .map(|draft| draft.generations.row),
        Some(draft_b_generation)
    );
    let modal = app.state.modal.as_ref().expect("guided update form kept");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided update form to remain open");
    };
    assert_eq!(fields[1].input.value, "Alicia");
    assert!(app.write_plans.is_empty());
    assert!(app.state.query_loading);
    assert!(
        app.state
            .notification
            .as_ref()
            .is_some_and(|(message, _)| message.contains("✓ edit A")),
        "owned unrelated success must surface its success notification"
    );
}

#[tokio::test]
async fn guided_success_invalidates_same_row_update_form_before_unlock_and_blocks_stale_dispatch() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_plan = app.pending_update_draft.as_ref().unwrap().plan_id;
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, focus, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    *focus = 1;

    let mut plan_a = make_write_plan(
        WritePlanId(draft_plan.0 + 100),
        "db",
        None,
        "users",
        1,
        true,
    );
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(app.pending_update_draft.is_none());
    assert!(app.state.modal.is_none());
    assert!(app.write_plans.is_empty());
    assert!(
        app.guided_write_blocked_generation(&users_table())
            .is_none(),
        "a verified success must not demand a manual refresh"
    );
    assert!(
        app.state.table_browse_result.is_none() || app.guided_write_data_is_stale(&users_table()),
        "rows that predate the verified write must not remain writable"
    );
    app.dispatch_confirmed_write_plan(draft_plan, "stale edit".to_string())
        .await;
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.event_rx.try_recv().is_err());
}

#[tokio::test]
async fn guided_success_off_tables_invalidates_same_row_confirm_and_blocks_stale_dispatch() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let confirm_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected guided confirm, got {other:?}"),
    };
    app.state.current_tab = Tab::Sql;
    let mut plan_a = make_write_plan(
        WritePlanId(confirm_plan.0 + 100),
        "db",
        None,
        "users",
        1,
        true,
    );
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(app.state.modal.is_none());
    assert!(!app.write_plans.contains_key(&confirm_plan));
    assert!(
        app.guided_write_blocked_generation(&users_table())
            .is_none(),
        "a verified success must not demand a manual refresh"
    );
    assert!(
        app.state.table_browse_result.is_none() || app.guided_write_data_is_stale(&users_table()),
        "rows that predate the verified write must not remain writable"
    );
    app.dispatch_confirmed_write_plan(confirm_plan, "stale edit".to_string())
        .await;
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.event_rx.try_recv().is_err());
}

#[tokio::test]
async fn verified_success_marks_data_stale_without_manual_refresh_block_and_reload_restores_guided_writes(
) {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let confirm_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected guided confirm, got {other:?}"),
    };
    assert!(app.pending_update_confirmation_form.is_some());
    let stale_plan = app
        .write_plans
        .get(&confirm_plan)
        .cloned()
        .expect("confirmed write plan");
    app.state.current_tab = Tab::Sql;
    let mut plan_a = make_write_plan(
        WritePlanId(confirm_plan.0 + 100),
        "db",
        None,
        "users",
        1,
        true,
    );
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    // A read owner is active, so the post-success reload defers instead of
    // dispatching a real request from the test.
    app.state.query_loading = true;

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(
        app.guided_write_blocked_generation(&users_table())
            .is_none(),
        "a verified success must never demand a manual refresh"
    );
    assert_eq!(
        app.guided_write_unavailable_reason(&users_table()),
        Some(GuidedWriteUnavailable::StaleAfterVerifiedWrite),
        "cached rows behind the invalidated owner must be treated as stale"
    );
    assert!(
        app.deferred_guided_refresh.is_some(),
        "the success must queue the reload that clears the stale barrier"
    );

    // The queued reload has not landed yet, so the stale same-row plan must
    // still be refused, and the user must not be blamed for it.
    app.state.error_message = None;
    app.state.current_tab = Tab::Tables;
    app.write_plans.insert(confirm_plan, stale_plan);
    app.dispatch_confirmed_write_plan(confirm_plan, "stale edit".to_string())
        .await;
    assert!(
        app.guided_write_locks.targets_by_plan_id.is_empty(),
        "a stale same-row plan must never acquire transport ownership"
    );
    assert!(app.event_rx.try_recv().is_err());
    let message = app.state.error_message.clone().unwrap_or_default();
    assert!(
        !message.contains("manual refresh"),
        "success must not tell the user to refresh manually: {message}"
    );
    assert!(
        message.contains("reload"),
        "user must learn the reload is automatic: {message}"
    );

    app.write_plans.remove(&confirm_plan);
    app.state.query_loading = false;
    app.deferred_guided_refresh = None;
    app.table_generation += 1;
    let reload_context =
        users_browse_context(&app, TableBrowseOrigin::Automatic, app.table_generation);
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: reload_context,
        result: query_result_two_rows(),
        total_rows: None,
    })
    .await;

    assert_eq!(
        app.guided_write_unavailable_reason(&users_table()),
        None,
        "an ordinary reload must restore guided writes without a manual refresh"
    );
    app.state.modal = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    assert!(matches!(
        app.state
            .modal
            .as_ref()
            .map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::DirtyRowChoice { .. }
        ))
    ));
}

/// Issue a context for `scope`, then immediately supersede it by issuing
/// another context for the same scope. Returns the now-stale first context.
fn stale_context_for(
    app: &mut App,
    scope: crate::effects::request::RequestScope,
) -> crate::effects::request::RequestContext {
    let stale = app.next_request_context(scope.clone());
    let _current = app.next_request_context(scope);
    stale
}

#[tokio::test]
async fn stale_query_result_from_superseded_request_is_rejected() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;

    let stale = stale_context_for(
        &mut app,
        crate::effects::request::RequestScope::SqlWorkspace {
            database: "db".to_string(),
            workspace: "main".to_string(),
        },
    );

    app.handle_app_event(AppEvent::QueryResult {
        context: stale,
        result: query_result("from old request"),
        duration: Duration::from_millis(5),
        sql: "SELECT * FROM secrets".to_string(),
    })
    .await;

    assert!(
        app.state.query_result.is_none(),
        "a superseded query result must not replace current state"
    );
    assert!(
        app.state.query_loading,
        "a superseded query result must not clear the loading flag"
    );
    assert_eq!(app.ignored_stale_results, 1);
}

#[tokio::test]
async fn stale_query_error_does_not_clear_newer_query_loading() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;

    let stale = stale_context_for(
        &mut app,
        crate::effects::request::RequestScope::SqlWorkspace {
            database: "db".to_string(),
            workspace: "main".to_string(),
        },
    );

    app.handle_app_event(AppEvent::QueryError {
        context: stale,
        sql: "SELECT 1".to_string(),
        error: "old query failed".to_string(),
    })
    .await;

    assert!(
        app.state.query_loading,
        "a superseded query error must not clear the newer query's loading flag"
    );
    assert!(
        app.state.error_message.is_none(),
        "a superseded query error must not raise a misleading error"
    );
}

#[tokio::test]
async fn stale_logs_from_superseded_request_are_rejected() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;

    let stale = stale_context_for(
        &mut app,
        crate::effects::request::RequestScope::Logs {
            database: "db".to_string(),
        },
    );

    app.handle_app_event(AppEvent::LogsLoaded {
        context: stale,
        logs: vec![crate::api::types::LogEntry {
            ts: None,
            level: crate::api::types::LogLevel::Info,
            message: "secret log from old request".to_string(),
            target: Some("old".to_string()),
            filename: None,
            line_number: None,
        }],
    })
    .await;

    assert!(
        app.state.log_buffer.is_empty(),
        "logs from a superseded request must not be shown"
    );
}

#[tokio::test]
async fn stale_live_clients_from_superseded_request_are_rejected() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;

    let stale = stale_context_for(
        &mut app,
        crate::effects::request::RequestScope::LiveClients {
            database: "db".to_string(),
        },
    );

    app.handle_app_event(AppEvent::LiveClientsLoaded {
        context: stale,
        clients: vec![crate::state::app_state::LiveClientEntry {
            identity: "ghost-client".to_string(),
            connected_at: None,
        }],
    })
    .await;

    assert!(
        app.state.live_clients.is_empty(),
        "live clients from a superseded request must not be shown"
    );
}

#[tokio::test]
async fn stale_metrics_from_superseded_request_are_rejected() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;

    let stale = stale_context_for(&mut app, crate::effects::request::RequestScope::Metrics);

    app.handle_app_event(AppEvent::MetricsLoaded {
        context: stale,
        snapshot: crate::state::MetricsSnapshot::default(),
    })
    .await;

    assert!(
        app.state.metrics_history.is_empty(),
        "metrics from a superseded request must not be applied"
    );
}

#[tokio::test]
async fn stale_databases_loaded_from_superseded_request_is_rejected() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.databases = vec!["db".to_string()];
    app.state.selected_database_idx = Some(0);

    let stale = stale_context_for(
        &mut app,
        crate::effects::request::RequestScope::DatabaseCatalog,
    );

    app.handle_app_event(AppEvent::DatabasesLoaded {
        context: stale,
        databases: vec!["db".to_string(), "ghost".to_string()],
    })
    .await;

    assert_eq!(
        app.state.databases,
        vec!["db".to_string()],
        "a superseded database list must not replace the current one"
    );
}

#[tokio::test]
async fn matching_context_query_result_is_accepted() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;

    let context = app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
        database: "db".to_string(),
        workspace: "main".to_string(),
    });

    app.handle_app_event(AppEvent::QueryResult {
        context,
        result: query_result("current"),
        duration: Duration::from_millis(3),
        sql: "SELECT 1".to_string(),
    })
    .await;

    assert!(
        app.state.query_result.is_some(),
        "a matching query result must be accepted"
    );
    assert!(
        !app.state.query_loading,
        "a matching query result must clear the loading flag"
    );
    assert_eq!(app.ignored_stale_results, 0);
}

#[tokio::test]
async fn stale_barrier_retires_with_its_generation_so_a_dropped_reload_cannot_trap_the_user() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let mut plan_a = make_write_plan(WritePlanId(9_300), "db", None, "users", 1, true);
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    app.state.query_loading = true;

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(
        app.guided_write_data_is_stale(&users_table()),
        "rows behind the verified write must be refused while they are on screen"
    );
    assert!(app.deferred_guided_refresh.is_some());

    // A WebSocket transaction update invalidates and drops the queued
    // reload. The barrier is still refusing writes, so the app must drive
    // the reload itself rather than trapping the user on a table whose
    // queued reload was cancelled.
    app.bump_table_generation();
    app.state.query_loading = false;
    app.flush_deferred_guided_refresh_if_unowned().await;
    assert!(
        app.deferred_guided_refresh.is_none(),
        "the invalidated queue entry must not linger"
    );
    assert!(
        app.state.query_loading,
        "the barrier must drive its own reload"
    );
    let refresh_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: refresh_context,
        result: query_result("fresh"),
        total_rows: None,
    })
    .await;
    assert_eq!(
        app.guided_write_unavailable_reason(&users_table()),
        None,
        "a barrier must never outlive the reload that retires it"
    );

    app.state.error_message = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    assert!(
        matches!(
            app.state
                .modal
                .as_ref()
                .map(crate::state::modal::Modal::action),
            Some(crate::state::modal::ModalAction::Safety(
                crate::state::modal::SafetyModalAction::DirtyRowChoice { .. }
            ))
        ),
        "the user must be able to edit again once the stale rows are gone: {:?}",
        app.state.error_message
    );
}

#[tokio::test]
async fn verified_success_clears_stranded_spreadsheet_awaiting_confirmation_for_same_row() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    if let Some(editor) = app
        .state
        .edit_mode
        .as_mut()
        .and_then(|edit_mode| edit_mode.editor.as_mut())
    {
        editor.set("Adele".to_string());
    } else {
        panic!("expected open cell editor");
    }
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app
        .spreadsheet_lifecycle_plan_id()
        .expect("spreadsheet save plan");
    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::AwaitingConfirmation(plan_id))
    );
    app.state.current_tab = Tab::Sql;
    let mut plan_a = make_write_plan(WritePlanId(plan_id.0 + 100), "db", None, "users", 1, true);
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(!app.write_plans.contains_key(&plan_id));
    assert!(app.state.modal.is_none());
    assert!(
        app.spreadsheet_save_lifecycle.is_none(),
        "an AwaitingConfirmation marker must not outlive its invalidated plan"
    );
    assert_eq!(
        app.state
            .edit_mode
            .as_ref()
            .map(|mode| mode.pending_count()),
        Some(1),
        "the user's own pending edits must survive an unrelated owner's success"
    );
}

#[tokio::test]
async fn deferred_guided_refresh_runs_after_guided_update_form_cancel() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_b_id = app.pending_update_draft.as_ref().unwrap().plan_id;

    let mut plan_a = make_write_plan(
        WritePlanId(draft_b_id.0 + 100),
        "db",
        None,
        "users",
        2,
        true,
    );
    plan_a.original_primary_key =
        crate::state::safety::CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(2),
            },
        ])
        .expect("plan A primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    let generation_before = app.table_generation;

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;
    assert_eq!(app.table_generation, generation_before);
    assert_eq!(
        app.deferred_guided_refresh
            .as_ref()
            .map(|refresh| &refresh.table),
        Some(&plan_a.table)
    );

    app.handle_modal_key(KeyEvent::from(KeyCode::Esc)).await;

    assert!(app.pending_update_draft.is_none());
    assert!(app.state.modal.is_none());
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
}

fn assert_guided_update_form_preserved(
    app: &App,
    expected_plan_id: WritePlanId,
    expected_generation: u64,
    expected_focus: usize,
    expected_values: &[&str],
) {
    assert_eq!(
        app.pending_update_draft.as_ref().map(|draft| draft.plan_id),
        Some(expected_plan_id)
    );
    assert_eq!(
        app.pending_update_draft
            .as_ref()
            .map(|draft| draft.generations.row),
        Some(expected_generation)
    );
    let modal = app.state.modal.as_ref().expect("guided form still open");
    let crate::state::modal::Modal::Form {
        title,
        fields,
        focus,
        action,
    } = modal
    else {
        panic!("expected guided update form, got {modal:?}");
    };
    assert_eq!(title, "Update row in users");
    assert_eq!(*focus, expected_focus);
    assert_eq!(
        action,
        &crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::DirtyRowChoice {
                requested_row: 0,
                requested_column: 700,
            }
        )
    );
    assert_eq!(
        fields
            .iter()
            .map(|field| field.input.value.as_str())
            .collect::<Vec<_>>(),
        expected_values
    );
}

#[tokio::test]
async fn guided_update_invalid_typed_input_keeps_exact_form_draft_and_deferred_refresh_until_correction(
) {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_plan = app.pending_update_draft.as_ref().unwrap().plan_id;
    let draft_generation = app.pending_update_draft.as_ref().unwrap().generations.row;
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, focus, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    fields[2].input.set("not-a-u64".to_string());
    *focus = 2;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;

    assert!(app.write_plans.is_empty());
    assert_eq!(app.table_generation, generation_before);
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_some());
    assert_guided_update_form_preserved(
        &app,
        draft_plan,
        draft_generation,
        2,
        &["1", "Alicia", "not-a-u64"],
    );
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("Cannot parse score"));

    let modal = app.state.modal.as_mut().expect("correctable guided form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[2].input.set("7".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;

    assert!(app.pending_update_draft.is_none());
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert!(matches!(
        app.state.modal.as_ref().map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id }
        )) if *plan_id == draft_plan
    ));
    assert!(app.write_plans.contains_key(&draft_plan));
}

#[tokio::test]
async fn guided_update_empty_update_keeps_exact_form_draft_and_esc_drains_deferred_refresh_once() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_plan = app.pending_update_draft.as_ref().unwrap().plan_id;
    let draft_generation = app.pending_update_draft.as_ref().unwrap().generations.row;
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { focus, .. } = modal else {
        panic!("expected guided form");
    };
    *focus = 1;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;

    assert!(app.write_plans.is_empty());
    assert_eq!(app.table_generation, generation_before);
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_some());
    assert_guided_update_form_preserved(&app, draft_plan, draft_generation, 1, &["1", "Ada", "5"]);
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("No changed fields"));

    app.handle_modal_key(KeyEvent::from(KeyCode::Esc)).await;

    assert!(app.pending_update_draft.is_none());
    assert!(app.state.modal.is_none());
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
    assert_no_extra_table_refresh(&mut app);
}

#[tokio::test]
async fn guided_confirm_local_row_lock_rejection_restores_exact_update_form_without_dispatch() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_plan = app.pending_update_draft.as_ref().unwrap().plan_id;
    let draft_generation = app.pending_update_draft.as_ref().unwrap().generations.row;
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, focus, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    fields[2].input.set("7".to_string());
    *focus = 2;
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let confirm_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected guided confirm, got {other:?}"),
    };
    let mut lock_plan = make_write_plan(
        WritePlanId(confirm_plan.0 + 100),
        "db",
        None,
        "users",
        1,
        true,
    );
    lock_plan.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&lock_plan));

    app.dispatch_confirmed_write_plan(confirm_plan, "guided update".to_string())
        .await;

    assert!(app.write_plans.contains_key(&confirm_plan));
    assert_guided_update_form_preserved(
        &app,
        draft_plan,
        draft_generation,
        2,
        &["1", "Alicia", "7"],
    );
    assert!(app.event_rx.try_recv().is_err());
    app.handle_modal_key(KeyEvent::from(KeyCode::Esc)).await;
    assert!(app.pending_update_draft.is_none());
    assert!(app.state.modal.is_none());
    assert!(
        app.pending_update_confirmation_form.is_none(),
        "cancelling a restored guided form must not strand its confirmation snapshot"
    );
    assert!(
        app.write_plans.is_empty(),
        "cancelling a restored guided form must not strand its reinserted write plan"
    );
}

#[tokio::test]
async fn guided_confirm_cancel_clears_snapshot_plan_and_modal_together() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let confirm_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected guided confirm, got {other:?}"),
    };
    assert!(app.pending_update_confirmation_form.is_some());

    app.handle_modal_key(KeyEvent::from(KeyCode::Esc)).await;

    assert!(app.state.modal.is_none());
    assert!(app.pending_update_draft.is_none());
    assert!(
        app.pending_update_confirmation_form.is_none(),
        "cancelling the guided confirm must not strand its confirmation snapshot"
    );
    assert!(!app.write_plans.contains_key(&confirm_plan));
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.event_rx.try_recv().is_err());
}

#[tokio::test]
async fn cancelled_guided_confirmation_does_not_let_later_same_row_success_close_a_live_confirmation(
) {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    app.handle_modal_key(KeyEvent::from(KeyCode::Esc)).await;

    // A live confirmation for a *different* row must survive a success that
    // only concerns the cancelled row.
    app.tables_grid.selected_row = 1;
    app.open_delete_confirm();
    let live_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected live delete confirm, got {other:?}"),
    };

    let mut plan_a = make_write_plan(WritePlanId(9_001), "db", None, "users", 1, true);
    plan_a.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan_a));
    app.state.query_loading = true;

    app.handle_app_event(guided_success_event(plan_a.id, "edit A"))
        .await;

    assert!(
        matches!(
            app.state.modal.as_ref().map(crate::state::modal::Modal::action),
            Some(crate::state::modal::ModalAction::Safety(
                crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id }
            )) if *plan_id == live_plan
        ),
        "a cancelled confirmation must not let a later success close a live unrelated confirmation"
    );
    assert!(app.write_plans.contains_key(&live_plan));
}

#[tokio::test]
async fn unsafe_outcome_blocks_at_event_time_so_only_a_later_manual_refresh_unblocks() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    let plan_id = WritePlanId(4_242);
    let mut plan = make_write_plan(plan_id, "db", None, "users", 1, true);
    plan.original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![crate::state::safety::PrimaryKeyPart {
            column_id: 511,
            value: crate::state::safety::SqlValue::U64(1),
        }])
        .expect("test primary key");
    assert!(app.guided_write_locks.acquire(&plan));
    // A manual refresh started before the unsafe outcome is known.
    app.table_generation = 7;
    let in_flight_manual = users_browse_context(&app, TableBrowseOrigin::ManualRefresh, 7);

    app.handle_app_event(guided_unknown_outcome_event(
        plan_id,
        "guided update",
        "no proof",
    ))
    .await;

    assert_eq!(
        app.guided_write_blocked_generation(&users_table()),
        Some(7),
        "the block epoch must be the generation observed when the outcome landed"
    );

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: in_flight_manual,
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(
        app.guided_write_blocked_generation(&users_table()),
        Some(7),
        "a manual refresh already in flight cannot prove the post-outcome state"
    );

    app.table_generation = 8;
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: users_browse_context(&app, TableBrowseOrigin::ManualRefresh, 8),
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(
        app.guided_write_blocked_generation(&users_table()),
        None,
        "a manual refresh started after the outcome must unblock"
    );
}

#[tokio::test]
async fn guided_owned_form_and_confirm_modals_clear_when_generation_discards_their_private_state() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let form_plan = app.pending_update_draft.as_ref().unwrap().plan_id;

    app.bump_table_generation();

    assert!(app.pending_update_draft.is_none());
    assert!(!app.write_plans.contains_key(&form_plan));
    assert!(
        app.state.modal.is_none(),
        "stale guided update form cleared"
    );

    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let confirm_plan = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected guided confirm, got {other:?}"),
    };

    app.bump_schema_generation();

    assert!(app.pending_update_draft.is_none());
    assert!(!app.write_plans.contains_key(&confirm_plan));
    assert!(app.state.modal.is_none(), "stale guided confirm cleared");

    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    app.state.modal = Some(crate::state::modal::Modal::form(
        "Unrelated alias",
        vec![crate::state::modal::FormField::new("New alias")],
        crate::state::modal::ModalAction::AddDatabaseAlias {
            database: "db".to_string(),
        },
    ));

    app.bump_server_context_generation();

    assert!(app.pending_update_draft.is_none());
    assert!(matches!(
        app.state.modal.as_ref().map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::AddDatabaseAlias { database }) if database == "db"
    ));
}

fn selected_users_table() -> QualifiedTable {
    QualifiedTable {
        database: "db".to_string(),
        schema: None,
        table: "users".to_string(),
    }
}

fn queue_users_refresh(app: &mut App) {
    app.deferred_guided_refresh = Some(app.deferred_table_refresh_for(selected_users_table()));
}

fn assert_no_extra_table_refresh(app: &mut App) {
    assert!(
        app.event_rx.try_recv().is_err(),
        "deferred refresh should drain exactly once"
    );
}

#[tokio::test]
async fn deferred_guided_refresh_drains_once_after_sql_query_result_owner_clears() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;
    let context = app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
        database: "db".to_string(),
        workspace: "main".to_string(),
    });

    app.handle_app_event(AppEvent::QueryResult {
        context: context.clone(),
        result: query_result("sql"),
        duration: Duration::from_millis(3),
        sql: "SELECT 1".to_string(),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(
        app.state.query_loading,
        "drained refresh starts a new table read after SQL completes"
    );
    assert_eq!(app.table_generation, generation_before + 1);
    assert_no_extra_table_refresh(&mut app);

    app.handle_app_event(AppEvent::QueryResult {
        context,
        result: query_result("duplicate"),
        duration: Duration::from_millis(1),
        sql: "SELECT 2".to_string(),
    })
    .await;

    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_once_after_sql_query_error_owner_clears() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;
    let context = app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
        database: "db".to_string(),
        workspace: "main".to_string(),
    });

    app.handle_app_event(AppEvent::QueryError {
        context: context.clone(),
        sql: "SELECT bad".to_string(),
        error: "boom".to_string(),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(
        app.state.query_loading,
        "drained refresh starts a new table read after SQL error completes"
    );
    assert_eq!(app.table_generation, generation_before + 1);
    assert_no_extra_table_refresh(&mut app);

    app.handle_app_event(AppEvent::QueryError {
        context,
        sql: "SELECT bad again".to_string(),
        error: "boom again".to_string(),
    })
    .await;

    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_after_matching_schema_loaded_owner_clears_and_accepts_terminal(
) {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.schema_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;
    let context = app.current_schema_request_context("db".to_string());

    app.handle_app_event(AppEvent::SchemaLoaded {
        context,
        schema: schema_with_table("users"),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(
        app.state.query_loading,
        "drained refresh starts after matching schema load completes"
    );
    assert!(!app.state.schema_loading);
    assert_eq!(app.table_generation, generation_before + 1);
    let browse_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: browse_context,
        result: query_result("fresh"),
        total_rows: None,
    })
    .await;

    assert_eq!(
        app.state
            .table_browse_result
            .as_ref()
            .and_then(|result| result.rows.first())
            .and_then(|row| row.get(1)),
        Some(&serde_json::json!("fresh"))
    );
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);

    app.handle_app_event(AppEvent::SchemaLoaded {
        context: app.current_schema_request_context("db".to_string()),
        schema: schema_with_table("users"),
    })
    .await;

    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_schema_loaded_rebases_matching_intent_and_accepts_error_terminal()
{
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.schema_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;
    let context = app.current_schema_request_context("db".to_string());

    app.handle_app_event(AppEvent::SchemaLoaded {
        context,
        schema: schema_with_table("users"),
    })
    .await;

    let browse_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );
    app.handle_app_event(AppEvent::TableBrowseError {
        context: browse_context,
        error: "terminal boom".to_string(),
    })
    .await;

    assert_eq!(app.state.error_message.as_deref(), Some("terminal boom"));
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_schema_loaded_drops_stale_intent_instead_of_rebasing() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.schema_loading = true;
    queue_users_refresh(&mut app);
    app.deferred_guided_refresh
        .as_mut()
        .expect("queued refresh")
        .server_context_generation = app.server_context_generation.saturating_add(1);
    let generation_before = app.table_generation;
    let context = app.current_schema_request_context("db".to_string());

    app.handle_app_event(AppEvent::SchemaLoaded {
        context,
        schema: schema_with_table("users"),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(!app.state.query_loading);
    assert_eq!(app.table_generation, generation_before);

    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.schema_loading = true;
    queue_users_refresh(&mut app);
    app.state.selected_table_idx = Some(1);
    let mut posts = schema_with_table("posts").tables.remove(0);
    posts.table_name = "posts".to_string();
    app.state.tables.push(posts);
    let generation_before = app.table_generation;
    let context = app.current_schema_request_context("db".to_string());

    app.handle_app_event(AppEvent::SchemaLoaded {
        context,
        schema: schema_with_table("users"),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(!app.state.query_loading);
    assert_eq!(app.table_generation, generation_before);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_after_matching_schema_error_owner_clears() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.schema_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;
    let context = app.current_schema_request_context("db".to_string());

    app.handle_app_event(AppEvent::SchemaError {
        context,
        error: "schema boom".to_string(),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(
        app.state.query_loading,
        "drained refresh starts after matching schema error completes"
    );
    assert!(!app.state.schema_loading);
    assert!(app.state.schema_load_failed);
    assert_eq!(app.table_generation, generation_before + 1);
    assert_no_extra_table_refresh(&mut app);
}

#[tokio::test]
async fn deferred_guided_refresh_generic_write_result_does_not_clear_unrelated_sql_owner() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.handle_app_event(AppEvent::WriteOpSuccess {
        op: "generic write".to_string(),
        response: serde_json::Value::Null,
    })
    .await;

    assert!(app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert_no_extra_table_refresh(&mut app);

    app.handle_app_event(AppEvent::WriteOpError {
        op: "generic write".to_string(),
        error: "boom".to_string(),
    })
    .await;

    assert!(app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert_no_extra_table_refresh(&mut app);
}

#[tokio::test]
async fn deferred_guided_refresh_generic_write_success_defers_for_active_read_and_drains_once() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    let read_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );
    app.state.query_loading = true;
    let generation_before = app.table_generation;

    app.handle_app_event(AppEvent::WriteOpSuccess {
        op: "generic write".to_string(),
        response: serde_json::Value::Null,
    })
    .await;

    assert!(app.state.query_loading, "existing read owner is preserved");
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: read_context,
        result: query_result("old read"),
        total_rows: None,
    })
    .await;

    assert!(
        app.state.query_loading,
        "deferred refresh starts after read owner clears"
    );
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
    let refresh_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: refresh_context,
        result: query_result("fresh"),
        total_rows: None,
    })
    .await;

    assert!(!app.state.query_loading);
    assert_eq!(app.table_generation, generation_before + 1);
    assert!(app.deferred_guided_refresh.is_none());
}

#[tokio::test]
async fn deferred_guided_refresh_generic_write_success_defers_for_guided_spreadsheet_and_form_owners(
) {
    let mut app = spreadsheet_app();
    let generation_before = app.table_generation;
    app.handle_app_event(AppEvent::WriteOpSuccess {
        op: "generic write".to_string(),
        response: serde_json::Value::Null,
    })
    .await;
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert!(
        app.state.edit_mode.is_some(),
        "spreadsheet input is preserved"
    );

    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let generation_before = app.table_generation;
    app.handle_app_event(AppEvent::WriteOpSuccess {
        op: "generic write".to_string(),
        response: serde_json::Value::Null,
    })
    .await;
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert!(
        app.pending_update_draft.is_some(),
        "guided form input is preserved"
    );

    app.handle_app_event(AppEvent::WriteOpError {
        op: "generic write".to_string(),
        error: "boom".to_string(),
    })
    .await;
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
}

#[tokio::test]
async fn deferred_guided_refresh_generic_write_error_without_queue_creates_no_refresh() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.state.query_loading = true;
    let generation_before = app.table_generation;

    app.handle_app_event(AppEvent::WriteOpError {
        op: "generic write".to_string(),
        error: "boom".to_string(),
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(app.state.query_loading);
    assert_eq!(app.table_generation, generation_before);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_after_clean_spreadsheet_edit_exit() {
    let mut app = spreadsheet_app();
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.exit_edit_mode().await;

    assert!(app.state.edit_mode.is_none());
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_after_spreadsheet_discard_terminal_path() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.exit_edit_mode().await;
    assert!(matches!(
        app.state
            .modal
            .as_ref()
            .map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::DiscardPendingEdits)
    ));
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;

    assert!(app.state.edit_mode.is_none());
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_after_guided_confirmation_local_not_sent() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let draft_plan = app.pending_update_draft.as_ref().unwrap().plan_id;
    let draft_generation = app.pending_update_draft.as_ref().unwrap().generations.row;
    let modal = app.state.modal.as_mut().expect("guided update form modal");
    let crate::state::modal::Modal::Form { fields, focus, .. } = modal else {
        panic!("expected guided update form");
    };
    fields[1].input.set("Alicia".to_string());
    *focus = 1;
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let plan_id = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected confirmation modal, got {other:?}"),
    };
    app.state
        .table_browse_result
        .as_mut()
        .expect("table result")
        .rows[0][1] = serde_json::json!("Adrienne");
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.dispatch_confirmed_write_plan(plan_id, "guided update".to_string())
        .await;

    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert_guided_update_form_preserved(
        &app,
        draft_plan,
        draft_generation,
        1,
        &["1", "Alicia", "5"],
    );
    assert!(app.event_rx.try_recv().is_err());
}

#[tokio::test]
async fn deferred_guided_refresh_waits_for_generic_current_table_guided_lock_then_drains_once() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form modal");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided update form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let plan_id = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected confirmation modal, got {other:?}"),
    };
    app.state.modal = None;
    app.pending_update_draft = None;
    app.dispatch_confirmed_write_plan(plan_id, "guided update".to_string())
        .await;
    assert_eq!(app.guided_write_locks.targets_by_plan_id.len(), 1);
    assert!(app.pending_update_draft.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(app.state.modal.is_none());
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.flush_deferred_guided_refresh_if_unowned().await;

    assert_eq!(app.guided_write_locks.targets_by_plan_id.len(), 1);
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);
    assert!(
        !app.state.query_loading,
        "existing loading owner must not be clobbered while guided lock owns the row"
    );

    app.handle_app_event(guided_success_event(plan_id, "guided update"))
        .await;

    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.deferred_guided_refresh.is_none());
    assert!(app.state.query_loading);
    assert_eq!(app.table_generation, generation_before + 1);
    let generation_after_drain = app.table_generation;

    app.handle_app_event(guided_success_event(plan_id, "duplicate guided update"))
        .await;

    assert_eq!(app.table_generation, generation_after_drain);
}

#[tokio::test]
async fn deferred_guided_refresh_waits_for_existing_table_read_owner_then_drains() {
    let mut app = spreadsheet_app();
    let existing_context = app.current_table_browse_request_context(
        "db".to_string(),
        "users".to_string(),
        TableBrowseOrigin::Automatic,
    );
    app.state.edit_mode = None;
    app.state.query_loading = true;
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.flush_deferred_guided_refresh_if_unowned().await;

    assert!(
        app.state.query_loading,
        "existing read owner must not be clobbered"
    );
    assert!(app.deferred_guided_refresh.is_some());
    assert_eq!(app.table_generation, generation_before);

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: existing_context,
        result: query_result("fresh"),
        total_rows: None,
    })
    .await;

    assert!(
        app.state.query_loading,
        "drained refresh starts after matching read completes"
    );
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_is_dropped_by_generation_invalidations() {
    let mut app = spreadsheet_app();
    queue_users_refresh(&mut app);
    app.bump_table_generation();
    app.flush_deferred_guided_refresh_if_unowned().await;
    assert!(app.deferred_guided_refresh.is_none());

    let mut app = spreadsheet_app();
    queue_users_refresh(&mut app);
    app.bump_schema_generation();
    app.flush_deferred_guided_refresh_if_unowned().await;
    assert!(app.deferred_guided_refresh.is_none());

    let mut app = spreadsheet_app();
    queue_users_refresh(&mut app);
    app.bump_database_generation();
    app.flush_deferred_guided_refresh_if_unowned().await;
    assert!(app.deferred_guided_refresh.is_none());

    let mut app = spreadsheet_app();
    queue_users_refresh(&mut app);
    app.bump_server_context_generation();
    app.flush_deferred_guided_refresh_if_unowned().await;
    assert!(app.deferred_guided_refresh.is_none());
}

#[tokio::test]
async fn deferred_guided_refresh_is_dropped_after_away_and_back_navigation_to_same_named_table() {
    let mut app = test_app();
    app.state.databases = vec!["db".to_string(), "other".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = schema_with_table("users").tables;
    app.state.selected_table_idx = Some(0);
    app.state.current_tab = Tab::Tables;
    queue_users_refresh(&mut app);

    app.state.sidebar_focus = SidebarFocus::Tables;
    app.state.selected_table_idx = Some(0);
    app.nav_down().await;
    app.state.tables = schema_with_table("users").tables;
    app.state.selected_table_idx = Some(0);
    app.nav_up().await;
    app.state.tables = schema_with_table("users").tables;
    app.state.selected_table_idx = Some(0);
    let generation_before_flush = app.table_generation;
    app.flush_deferred_guided_refresh_if_unowned().await;

    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.table_generation, generation_before_flush);
}

#[tokio::test]
async fn spreadsheet_server_context_invalidation_clears_non_inflight_and_invalidated_error_clears_inflight(
) {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let awaiting_plan = app.spreadsheet_lifecycle_plan_id().unwrap();
    app.bump_server_context_generation();
    assert!(app.state.edit_mode.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(!app.write_plans.contains_key(&awaiting_plan));

    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();
    app.dispatch_confirmed_write_plan(plan_id, "edit".to_string())
        .await;
    app.bump_server_context_generation();
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    assert!(
        app.spreadsheet_save_lifecycle.is_some(),
        "invalidated in-flight state remains correlated until result"
    );
    app.handle_app_event(guided_unknown_outcome_event(plan_id, "edit", "stale"))
        .await;
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert!(
        app.state.edit_mode.is_none(),
        "invalidated matching error clears stale edits"
    );
}

#[tokio::test]
async fn spreadsheet_discard_pending_edits_modal_is_cleared_on_table_invalidation() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.exit_edit_mode().await;
    assert!(matches!(
        app.state
            .modal
            .as_ref()
            .map(crate::state::modal::Modal::action),
        Some(crate::state::modal::ModalAction::DiscardPendingEdits)
    ));
    app.bump_table_generation();
    assert!(app.state.edit_mode.is_none());
    assert!(
        app.state.modal.is_none(),
        "discard-pending-edits modal is spreadsheet-owned"
    );
}

#[tokio::test]
async fn spreadsheet_stale_dirty_row_choice_does_not_discard_or_start_requested_editor() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    let pending_before = app.state.edit_mode.as_ref().unwrap().pending_count();
    app.pending_spreadsheet_edit_request = Some(PendingSpreadsheetEditRequest {
        target: app.edit_row_target_at(1).unwrap(),
        column_index: 2,
    });

    app.handle_spreadsheet_dirty_row_choice(crate::state::modal::DirtyRowChoice::Discard, 1, 700)
        .await;

    assert_eq!(
        app.state.edit_mode.as_ref().unwrap().pending_count(),
        pending_before
    );
    assert!(app.state.edit_mode.as_ref().unwrap().editor.is_none());
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert_eq!(app.tables_grid.selected_row, 0);
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("Spreadsheet dirty-row choice no longer matches the pending edit")
    );
}

#[tokio::test]
async fn spreadsheet_save_choice_consumes_pending_request_and_cancel_preserves_edits() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.pending_spreadsheet_edit_request = Some(PendingSpreadsheetEditRequest {
        target: app.edit_row_target_at(1).unwrap(),
        column_index: 2,
    });

    app.handle_spreadsheet_dirty_row_choice(crate::state::modal::DirtyRowChoice::Save, 1, 900)
        .await;

    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_lifecycle_plan_id().is_some());
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    let modal = app.state.modal.clone().unwrap();
    app.discard_guided_state_for_modal(&modal);
    assert!(app.pending_spreadsheet_edit_request.is_none());
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
}

#[test]
fn spreadsheet_parse_error_preserves_open_editor_and_canonical_edits() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 2;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("not-a-u64".to_string());

    app.commit_cell_edit();

    let edit_mode = app.state.edit_mode.as_ref().unwrap();
    assert_eq!(edit_mode.pending_count(), 0);
    assert_eq!(
        edit_mode
            .editor
            .as_ref()
            .map(|editor| editor.value.as_str()),
        Some("not-a-u64")
    );
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("Cannot parse score"));
}

#[tokio::test]
async fn spreadsheet_set_cell_generation_error_preserves_typed_editor_and_canonical_edits() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    let canonical_before = app
        .state
        .edit_mode
        .as_ref()
        .unwrap()
        .spreadsheet_edits()
        .pending_single_row_save()
        .expect("canonical dirty row before generation change");

    app.tables_grid.selected_col = 2;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("9".to_string());
    app.table_generation += 1;

    app.commit_cell_edit();

    let edit_mode = app.state.edit_mode.as_ref().unwrap();
    assert_eq!(
        edit_mode
            .editor
            .as_ref()
            .map(|editor| editor.value.as_str()),
        Some("9")
    );
    assert_eq!(edit_mode.pending_count(), 1);
    assert_eq!(
        edit_mode.spreadsheet_edits().pending_single_row_save(),
        Some(canonical_before)
    );
    assert!(app.write_plans.is_empty());
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert!(app.event_rx.try_recv().is_err());
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("Write plan was not sent: row generation changed before edit")
    );
}

#[tokio::test]
async fn spreadsheet_confirm_local_failure_clears_marker_removes_plan_and_preserves_edits() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();
    assert!(app.write_plans.contains_key(&plan_id));

    app.state.query_loading = true;
    app.dispatch_confirmed_write_plan(plan_id, "edit".to_string())
        .await;

    assert!(!app.write_plans.contains_key(&plan_id));
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("Data is loading; write plan was not sent")
    );
    assert!(
        app.event_rx.try_recv().is_err(),
        "no dispatch event should be emitted"
    );
}

#[test]
fn guided_write_sql_error_classifier_preserves_transport_categories_and_details() {
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!("SQL query HTTP 503: unavailable")),
        TransportFailure::Http5xx(503)
    );
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!("request cancelled by caller")),
        TransportFailure::Cancelled
    );
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!("operation timed out")),
        TransportFailure::Timeout
    );
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!(
            "error sending request: connection refused"
        )),
        TransportFailure::Disconnected
    );

    let protocol_error =
        App::classify_guided_write_sql_error(&anyhow::anyhow!("decode failed: invalid JSON frame"));
    assert_eq!(
        protocol_error,
        TransportFailure::Validation("decode failed: invalid JSON frame".into())
    );
    assert_ne!(protocol_error, TransportFailure::Disconnected);
}

#[test]
fn guided_write_sql_error_classifier_prefers_structured_http_and_preserves_protocol_details() {
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!(
            "SQL query HTTP 503: backend timed out"
        )),
        TransportFailure::Http5xx(503)
    );

    let protocol_error = App::classify_guided_write_sql_error(&anyhow::anyhow!(
        "error sending request: HTTP/2 protocol stream error"
    ));
    assert_eq!(
        protocol_error,
        TransportFailure::Validation("error sending request: HTTP/2 protocol stream error".into())
    );
    assert_ne!(protocol_error, TransportFailure::Disconnected);

    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!(
            "error sending request: connection reset by peer"
        )),
        TransportFailure::Disconnected
    );
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!("request canceled by caller")),
        TransportFailure::Cancelled
    );
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!("backend timeout elapsed")),
        TransportFailure::Timeout
    );
}

#[test]
fn guided_write_sql_error_classifier_parsed_http_4xx_wins_over_body_heuristics() {
    let cancelled_detail = "SQL query HTTP 409: operation cancelled";
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!(cancelled_detail)),
        TransportFailure::Validation(cancelled_detail.into())
    );

    let timeout_detail = "SQL query HTTP 422: validation timed out while checking unique key";
    assert_eq!(
        App::classify_guided_write_sql_error(&anyhow::anyhow!(timeout_detail)),
        TransportFailure::Validation(timeout_detail.into())
    );
}

#[tokio::test]
async fn spreadsheet_postcondition_planning_failure_clears_marker_consumes_plan_without_dispatch() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .and_then(|edit_mode| edit_mode.editor.as_mut())
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();

    app.state.tables[0].columns[1].col_name = "bad\0column".to_string();
    app.dispatch_confirmed_write_plan(plan_id, "spreadsheet save".to_string())
        .await;

    assert_eq!(
        app.state
            .edit_mode
            .as_ref()
            .map(|mode| mode.pending_count()),
        Some(1)
    );
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert!(!app.write_plans.contains_key(&plan_id));
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.event_rx.try_recv().is_err());
    assert!(!app.state.query_loading);
    assert!(app.state.query_result.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("Postcondition verification planning failed; nothing sent"));
}

#[tokio::test]
async fn spreadsheet_in_flight_freezes_edit_revert_exit_and_duplicate_save_until_result() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    app.state
        .edit_mode
        .as_mut()
        .unwrap()
        .editor
        .as_mut()
        .unwrap()
        .set("Adele".to_string());
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let plan_id = app.spreadsheet_lifecycle_plan_id().unwrap();
    app.dispatch_confirmed_write_plan(plan_id, "edit".to_string())
        .await;
    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::InFlight(plan_id))
    );
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);

    app.begin_cell_edit();
    assert!(app.state.edit_mode.as_ref().unwrap().editor.is_none());
    app.revert_active_cell();
    app.exit_edit_mode().await;
    app.save_pending_edits().await;

    assert_eq!(
        app.spreadsheet_save_lifecycle,
        Some(SpreadsheetSaveLifecycle::InFlight(plan_id))
    );
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
    assert!(app
        .guided_write_locks
        .targets_by_plan_id
        .contains_key(&plan_id));

    app.handle_app_event(guided_unknown_outcome_event(plan_id, "edit", "nope"))
        .await;
    assert!(app.spreadsheet_save_lifecycle.is_none());
    assert_eq!(app.state.edit_mode.as_ref().unwrap().pending_count(), 1);
}

fn users_table() -> QualifiedTable {
    QualifiedTable {
        database: "db".into(),
        schema: None,
        table: "users".into(),
    }
}

fn block_users_guided_writes(app: &mut App, generation: u64) {
    app.guided_write_blocks
        .insert(GuidedWriteBlockKey::from_table(&users_table()), generation);
}

fn users_browse_context(
    app: &App,
    origin: TableBrowseOrigin,
    table_generation: u64,
) -> TableBrowseRequestContext {
    TableBrowseRequestContext {
        database: "db".into(),
        schema: None,
        table: "users".into(),
        origin,
        server_context_generation: app.server_context_generation,
        table_generation,
        schema_generation: app.schema_generation,
        database_generation: app.database_generation,
    }
}

#[test]
fn blocked_enter_edit_mode_itself_refuses_and_explains_manual_refresh() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    block_users_guided_writes(&mut app, 0);

    app.enter_edit_mode();

    assert!(app.state.edit_mode.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));
}

#[test]
fn blocked_guided_update_form_refuses_and_explains_manual_refresh() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    block_users_guided_writes(&mut app, 0);

    app.open_update_form();

    assert!(app.state.modal.is_none());
    assert!(app.pending_update_draft.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));
}

#[test]
fn blocked_guided_delete_confirmation_refuses_and_explains_manual_refresh() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    block_users_guided_writes(&mut app, 0);

    app.open_delete_confirm();

    assert!(app.state.modal.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));
}

#[tokio::test]
async fn blocked_spreadsheet_save_preserves_existing_dirty_row() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    if let Some(editor) = app
        .state
        .edit_mode
        .as_mut()
        .and_then(|edit_mode| edit_mode.editor.as_mut())
    {
        editor.set("Adele".to_string());
    } else {
        panic!("expected open cell editor");
    }
    app.commit_cell_edit();
    block_users_guided_writes(&mut app, 0);

    app.save_pending_edits().await;

    assert_eq!(
        app.state
            .edit_mode
            .as_ref()
            .map(|mode| mode.pending_count()),
        Some(1)
    );
    assert!(app.write_plans.is_empty());
    assert!(app.event_rx.try_recv().is_err());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));
}

#[tokio::test]
async fn blocked_final_confirm_cleans_plan_without_event_lock_or_losing_spreadsheet_edits() {
    let mut app = spreadsheet_app();
    app.tables_grid.selected_col = 1;
    app.begin_cell_edit();
    if let Some(editor) = app
        .state
        .edit_mode
        .as_mut()
        .and_then(|edit_mode| edit_mode.editor.as_mut())
    {
        editor.set("Adele".to_string());
    } else {
        panic!("expected open cell editor");
    }
    app.commit_cell_edit();
    app.save_pending_edits().await;
    let Some(plan_id) = app.spreadsheet_lifecycle_plan_id() else {
        panic!("expected spreadsheet save plan");
    };
    block_users_guided_writes(&mut app, 0);

    app.dispatch_confirmed_write_plan(plan_id, "spreadsheet save".into())
        .await;

    assert!(!app.write_plans.contains_key(&plan_id));
    assert!(app.spreadsheet_lifecycle_plan_id().is_none());
    assert_eq!(
        app.state
            .edit_mode
            .as_ref()
            .map(|mode| mode.pending_count()),
        Some(1)
    );
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app.event_rx.try_recv().is_err());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));
}

#[tokio::test]
async fn exact_newer_manual_refresh_unblocks_but_same_generation_automatic_navigation_error_stale_unrelated_and_navigation_do_not(
) {
    let mut app = spreadsheet_app();
    block_users_guided_writes(&mut app, 0);
    let users = users_table();

    for (origin, generation) in [
        (TableBrowseOrigin::ManualRefresh, 0),
        (TableBrowseOrigin::Automatic, 1),
        (TableBrowseOrigin::Navigation, 1),
    ] {
        app.table_generation = generation;
        app.handle_app_event(AppEvent::TableBrowseResult {
            context: users_browse_context(&app, origin, generation),
            result: query_result("Ada"),
            total_rows: None,
        })
        .await;
        assert_eq!(app.guided_write_blocked_generation(&users), Some(0));
    }

    app.handle_app_event(AppEvent::TableBrowseError {
        context: users_browse_context(&app, TableBrowseOrigin::ManualRefresh, 1),
        error: "boom".into(),
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: TableBrowseRequestContext {
            table: "other".into(),
            ..users_browse_context(&app, TableBrowseOrigin::ManualRefresh, 1)
        },
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    app.state.selected_table_idx = None;
    app.state.selected_table_idx = Some(0);
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    app.table_generation = 1;
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: users_browse_context(&app, TableBrowseOrigin::ManualRefresh, 1),
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), None);
}

#[tokio::test]
async fn blocked_generic_write_success_does_not_auto_refresh_or_defer_current_table() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    block_users_guided_writes(&mut app, 0);
    let table_generation = app.table_generation;

    app.handle_app_event(AppEvent::WriteOpSuccess {
        op: "generic write".into(),
        response: serde_json::Value::Null,
    })
    .await;

    assert_eq!(app.table_generation, table_generation);
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.guided_write_blocked_generation(&users_table()), Some(0));
}

#[tokio::test]
async fn deferred_guided_refresh_flush_drops_target_that_became_blocked() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    queue_users_refresh(&mut app);
    assert!(app.deferred_guided_refresh.is_some());
    block_users_guided_writes(&mut app, 0);
    let table_generation = app.table_generation;

    app.flush_deferred_guided_refresh_if_unowned().await;

    assert_eq!(app.table_generation, table_generation);
    assert!(!app.state.query_loading);
    assert!(app.deferred_guided_refresh.is_none());
    assert_eq!(app.guided_write_blocked_generation(&users_table()), Some(0));
}

#[tokio::test]
async fn deferred_guided_refresh_drains_once_after_stale_guided_unknown_terminal() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    let table_a = QualifiedTable {
        database: "db".into(),
        schema: None,
        table: "posts".into(),
    };
    let plan_id = WritePlanId(700);
    let plan_a = make_write_plan(plan_id, "db", None, "posts", 1, true);
    assert!(app.guided_write_locks.acquire(&plan_a));
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "edit A".into(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::Unknown {
                reason: "ambiguous outcome".into(),
            },
        },
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(app.state.query_loading);
    assert_eq!(app.table_generation, generation_before + 1);
    assert_eq!(app.guided_write_blocked_generation(&table_a), Some(0));
    assert_eq!(app.guided_write_blocked_generation(&users_table()), None);
    assert_no_extra_table_refresh(&mut app);

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "edit A duplicate".into(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::Unknown {
                reason: "duplicate".into(),
            },
        },
    })
    .await;

    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn deferred_guided_refresh_drains_once_after_stale_guided_definitely_not_sent_terminal() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    let table_a = QualifiedTable {
        database: "db".into(),
        schema: None,
        table: "posts".into(),
    };
    let plan_id = WritePlanId(701);
    let plan_a = make_write_plan(plan_id, "db", None, "posts", 1, true);
    assert!(app.guided_write_locks.acquire(&plan_a));
    queue_users_refresh(&mut app);
    let generation_before = app.table_generation;

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "edit A".into(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::DefinitelyNotSent {
                reason: "local failure".into(),
            },
        },
    })
    .await;

    assert!(app.deferred_guided_refresh.is_none());
    assert!(app.state.query_loading);
    assert_eq!(app.table_generation, generation_before + 1);
    assert_eq!(app.guided_write_blocked_generation(&table_a), None);
    assert_eq!(app.guided_write_blocked_generation(&users_table()), None);
    assert_no_extra_table_refresh(&mut app);

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "edit A duplicate".into(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::DefinitelyNotSent {
                reason: "duplicate".into(),
            },
        },
    })
    .await;

    assert_eq!(app.table_generation, generation_before + 1);
}

#[tokio::test]
async fn guided_write_unsafe_final_blocks_exact_table_guards_and_newer_browse_unblocks() {
    let mut app = spreadsheet_app();
    app.state.edit_mode = None;
    app.tables_grid.selected_row = 0;
    app.tables_grid.selected_col = 1;
    app.open_update_form();
    let modal = app.state.modal.as_mut().expect("guided update form modal");
    let crate::state::modal::Modal::Form { fields, .. } = modal else {
        panic!("expected guided update form");
    };
    fields[1].input.set("Alicia".to_string());
    app.handle_modal_key(KeyEvent::from(KeyCode::Enter)).await;
    let plan_id = match app
        .state
        .modal
        .as_ref()
        .map(crate::state::modal::Modal::action)
    {
        Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) => *plan_id,
        other => panic!("expected confirmation modal, got {other:?}"),
    };
    app.state.modal = None;
    app.dispatch_confirmed_write_plan(plan_id, "guided update".to_string())
        .await;
    assert_eq!(app.guided_write_locks.targets_by_plan_id.len(), 1);
    queue_users_refresh(&mut app);

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "guided update".to_string(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::Unknown {
                reason: "verification failed".into(),
            },
        },
    })
    .await;

    let users = QualifiedTable {
        database: "db".into(),
        schema: None,
        table: "users".into(),
    };
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));
    assert!(app.deferred_guided_refresh.is_none());
    assert!(app.guided_write_locks.targets_by_plan_id.is_empty());
    assert!(app
        .state
        .log_buffer
        .iter()
        .any(|entry| entry.target.as_deref() == Some("guided-write-safety")));
    app.open_update_form();
    assert!(app.state.modal.is_none());
    assert!(app
        .state
        .error_message
        .as_deref()
        .unwrap_or_default()
        .contains("manual refresh"));

    app.handle_app_event(AppEvent::GuidedWriteVerification {
        plan_id,
        op: "duplicate".to_string(),
        report: WriteVerificationReport {
            outcome: MutationOutcome::SentAndConfirmed {
                affected_rows: None,
            },
        },
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    let stale_context = TableBrowseRequestContext {
        database: "db".into(),
        schema: None,
        table: "users".into(),
        origin: TableBrowseOrigin::Automatic,
        server_context_generation: app.server_context_generation,
        table_generation: 0,
        schema_generation: app.schema_generation,
        database_generation: app.database_generation,
    };
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: stale_context,
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    app.table_generation = 1;
    let newer_context = TableBrowseRequestContext {
        database: "db".into(),
        schema: None,
        table: "users".into(),
        origin: TableBrowseOrigin::Automatic,
        server_context_generation: app.server_context_generation,
        table_generation: 1,
        schema_generation: app.schema_generation,
        database_generation: app.database_generation,
    };
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: newer_context,
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), Some(0));

    let manual_newer_context = TableBrowseRequestContext {
        database: "db".into(),
        schema: None,
        table: "users".into(),
        origin: TableBrowseOrigin::ManualRefresh,
        server_context_generation: app.server_context_generation,
        table_generation: 1,
        schema_generation: app.schema_generation,
        database_generation: app.database_generation,
    };
    app.handle_app_event(AppEvent::TableBrowseResult {
        context: manual_newer_context,
        result: query_result("Ada"),
        total_rows: None,
    })
    .await;
    assert_eq!(app.guided_write_blocked_generation(&users), None);
}

#[tokio::test]
async fn app_ignores_matching_database_but_stale_schema_generation_schema_events() {
    let mut app = test_app();
    app.state.databases = vec!["db".to_string()];
    app.state.selected_database_idx = Some(0);
    app.schema_generation = 2;
    app.database_generation = 1;
    app.state.schema_loading = true;

    app.handle_app_event(AppEvent::SchemaLoaded {
        context: SchemaRequestContext {
            database: "db".to_string(),
            schema_generation: 1,
            database_generation: 1,
        },
        schema: schema_with_table("old"),
    })
    .await;
    assert!(app.state.schema_loading);
    assert!(app.state.tables.is_empty());

    app.state.set_error("newer schema error");
    app.handle_app_event(AppEvent::SchemaError {
        context: SchemaRequestContext {
            database: "db".to_string(),
            schema_generation: 1,
            database_generation: 1,
        },
        error: "stale".to_string(),
    })
    .await;
    assert!(app.state.schema_loading);
    assert!(!app.state.schema_load_failed);
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("newer schema error")
    );

    app.handle_app_event(AppEvent::SchemaLoaded {
        context: SchemaRequestContext {
            database: "db".to_string(),
            schema_generation: 2,
            database_generation: 1,
        },
        schema: schema_with_table("new"),
    })
    .await;
    assert!(!app.state.schema_loading);
    assert_eq!(app.state.tables[0].table_name, "new");

    app.state.schema_loading = true;
    app.handle_app_event(AppEvent::SchemaError {
        context: SchemaRequestContext {
            database: "db".to_string(),
            schema_generation: 2,
            database_generation: 1,
        },
        error: "matching".to_string(),
    })
    .await;
    assert!(!app.state.schema_loading);
    assert!(app.state.schema_load_failed);
    assert_eq!(app.state.error_message.as_deref(), Some("matching"));
}

#[tokio::test]
async fn app_ignores_stale_table_browse_events_including_schema_generation() {
    let mut app = test_app();
    app.state.databases = vec!["db".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = schema_with_table("users").tables;
    app.state.selected_table_idx = Some(0);
    app.schema_generation = 2;
    app.table_generation = 7;
    app.database_generation = 1;
    app.state.query_loading = true;

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: TableBrowseRequestContext {
            database: "db".to_string(),
            schema: None,
            table: "users".to_string(),
            origin: TableBrowseOrigin::Automatic,
            table_generation: 7,
            server_context_generation: 0,
            schema_generation: 1,
            database_generation: 1,
        },
        result: query_result("stale"),
        total_rows: None,
    })
    .await;
    assert!(
        app.state.query_loading,
        "stale result must preserve newer loading state"
    );
    assert!(app.state.table_browse_result.is_none());

    app.state.set_error("newer table error");
    app.handle_app_event(AppEvent::TableBrowseError {
        context: TableBrowseRequestContext {
            database: "db".to_string(),
            schema: None,
            table: "users".to_string(),
            origin: TableBrowseOrigin::Automatic,
            table_generation: 7,
            server_context_generation: 0,
            schema_generation: 1,
            database_generation: 1,
        },
        error: "stale".to_string(),
    })
    .await;
    assert!(
        app.state.query_loading,
        "stale error must preserve newer loading state"
    );
    assert!(app.state.table_browse_result.is_none());
    assert_eq!(
        app.state.error_message.as_deref(),
        Some("newer table error")
    );

    app.handle_app_event(AppEvent::TableBrowseResult {
        context: TableBrowseRequestContext {
            database: "db".to_string(),
            schema: None,
            table: "users".to_string(),
            origin: TableBrowseOrigin::Automatic,
            table_generation: 7,
            server_context_generation: 0,
            schema_generation: 2,
            database_generation: 1,
        },
        result: query_result("fresh"),
        total_rows: None,
    })
    .await;
    assert!(!app.state.query_loading);
    assert_eq!(
        app.state.table_browse_result.as_ref().unwrap().rows[0][1],
        serde_json::json!("fresh")
    );

    app.state.query_loading = true;
    app.handle_app_event(AppEvent::TableBrowseError {
        context: TableBrowseRequestContext {
            database: "db".to_string(),
            schema: None,
            table: "users".to_string(),
            origin: TableBrowseOrigin::Automatic,
            table_generation: 7,
            server_context_generation: 0,
            schema_generation: 2,
            database_generation: 1,
        },
        error: "matching".to_string(),
    })
    .await;
    assert!(!app.state.query_loading);
    assert_eq!(app.state.error_message.as_deref(), Some("matching"));
}

#[tokio::test]
async fn app_navigation_invalidates_browse_data_and_grid_state() {
    let mut app = test_app();
    app.state.databases = vec!["a".to_string(), "b".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = vec![
        make_table_with_pk("one", vec![typed_col_at(1, "id", "U64")], vec![1]),
        make_table_with_pk("two", vec![typed_col_at(1, "id", "U64")], vec![1]),
    ];
    app.state.selected_table_idx = Some(0);
    app.state.table_browse_result = Some(query_result("old"));
    app.tables_grid.selected_row = 1;
    app.state.focus = FocusPanel::Sidebar;
    app.state.sidebar_focus = SidebarFocus::Tables;
    app.nav_down().await;
    assert!(app.state.table_browse_result.is_none());
    assert_eq!(app.tables_grid.selected_row, 0);

    app.state.table_browse_result = Some(query_result("old"));
    app.tables_grid.selected_row = 1;
    app.state.sidebar_focus = SidebarFocus::Tables;
    app.state.selected_table_idx = Some(1);
    app.nav_down().await;
    assert_eq!(app.state.selected_database(), Some("b"));
    assert!(app.state.table_browse_result.is_none());
    assert_eq!(app.tables_grid.selected_row, 0);
}

#[tokio::test]
async fn shutdown_tasks_aborts_pending_tasks_and_joins_all_reports() {
    let mut app = test_app();

    // Spawn a task that would never finish on its own.
    app.task_registry.spawn("never finishes", async {
        tokio::time::sleep(Duration::from_secs(3600)).await
    });
    assert_eq!(app.task_registry.pending_count(), 1);

    app.shutdown_tasks().await;

    assert_eq!(app.task_registry.pending_count(), 0);
    assert!(app.task_registry.try_join_next().is_none());
}

#[test]
fn database_nav_up_emits_schema_reload_when_selection_changes() {
    let mut state = AppState::new("http://localhost:3000");
    state.databases = vec!["alpha".to_string(), "beta".to_string()];
    state.selected_database_idx = Some(1);

    assert!(database_nav_up_transition_requires_schema_reload(
        &mut state
    ));
    assert_eq!(state.selected_database(), Some("alpha"));
}

#[test]
fn table_browse_sql_quotes_identifiers_and_fetches_prefix() {
    assert_eq!(
        table_browse_sql("source_status", 0).unwrap(),
        r#"SELECT * FROM "source_status" LIMIT 200"#
    );
    assert_eq!(
        table_browse_sql(r#"we"ird"#, 400).unwrap(),
        r#"SELECT * FROM "we""ird" LIMIT 600"#
    );
}

#[test]
fn browse_page_bounds_slice_and_clamp() {
    // First page of a long table.
    assert_eq!(browse_page_bounds(500, 0), (0, 200, 0));
    // Third page.
    assert_eq!(browse_page_bounds(500, 400), (400, 500, 400));
    // Offset past the fetched rows clamps down to the last full page
    // boundary available.
    let (start, end, eff) = browse_page_bounds(50, 400);
    assert_eq!((start, end, eff), (50, 50, 50));
    // Empty table.
    assert_eq!(browse_page_bounds(0, 0), (0, 0, 0));
}

#[tokio::test]
async fn sidebar_j_from_database_walks_nested_tables() {
    let mut app = test_app();
    app.state.databases = vec!["sitdeck".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = vec![
        make_table_with_pk("annotation", vec![typed_col_at(0, "id", "String")], vec![0]),
        make_table_with_pk(
            "source_status",
            vec![typed_col_at(0, "key", "String")],
            vec![0],
        ),
    ];
    app.state.selected_table_idx = Some(0);
    app.state.focus = FocusPanel::Sidebar;
    app.state.sidebar_focus = SidebarFocus::Databases;

    app.nav_down().await;
    assert_eq!(app.state.sidebar_focus, SidebarFocus::Tables);
    assert_eq!(
        app.state.selected_table().map(|t| t.table_name.as_str()),
        Some("annotation")
    );

    app.nav_down().await;
    assert_eq!(
        app.state.selected_table().map(|t| t.table_name.as_str()),
        Some("source_status")
    );
}

#[tokio::test]
async fn key_release_does_not_advance_sidebar_selection() {
    let mut app = test_app();
    app.state.databases = vec!["sitdeck".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = vec![
        make_table_with_pk("annotation", vec![typed_col_at(0, "id", "String")], vec![0]),
        make_table_with_pk(
            "source_status",
            vec![typed_col_at(0, "key", "String")],
            vec![0],
        ),
        make_table_with_pk(
            "user_account",
            vec![typed_col_at(0, "id", "String")],
            vec![0],
        ),
    ];
    app.state.selected_table_idx = Some(0);
    app.state.focus = FocusPanel::Sidebar;
    app.state.sidebar_focus = SidebarFocus::Tables;

    let mut release = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    app.handle_key(release).await;
    assert_eq!(
        app.state.selected_table().map(|t| t.table_name.as_str()),
        Some("annotation"),
        "Windows key-release must not move the cursor"
    );

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE))
        .await;
    assert_eq!(
        app.state.selected_table().map(|t| t.table_name.as_str()),
        Some("source_status")
    );
}

#[test]
fn live_light_update_applies_without_bumping_table_generation() {
    let mut app = test_app();
    app.state.databases = vec!["sitdeck".to_string()];
    app.state.selected_database_idx = Some(0);
    app.state.tables = vec![make_table_with_pk(
        "source_status",
        vec![
            typed_col_at(0, "key", "String"),
            typed_col_at(1, "healthy", "Bool"),
        ],
        vec![0],
    )];
    app.state.selected_table_idx = Some(0);
    app.state.table_browse_result = Some(crate::api::types::QueryResult {
        schema: vec![
            crate::api::types::SchemaElement {
                name: "key".to_string(),
                algebraic_type: serde_json::json!("String"),
            },
            crate::api::types::SchemaElement {
                name: "healthy".to_string(),
                algebraic_type: serde_json::json!("Bool"),
            },
        ],
        rows: vec![vec![serde_json::json!("AIS"), serde_json::json!(true)]],
        total_duration_micros: 0,
    });
    let generation = app.table_generation;

    let message: crate::api::types::WsServerMessage = serde_json::from_str(
        r#"{
                "TransactionUpdateLight": {
                    "request_id": 0,
                    "update": {
                        "tables": [{
                            "table_id": 1,
                            "table_name": "source_status",
                            "num_rows": 2,
                            "updates": [{
                                "deletes": ["[\"AIS\", true]"],
                                "inserts": ["[\"AIS\", false]"]
                            }]
                        }]
                    }
                }
            }"#,
    )
    .unwrap();
    app.handle_ws_server_message(message);

    assert_eq!(app.table_generation, generation);
    let rows = &app.state.table_browse_result.as_ref().unwrap().rows;
    assert_eq!(
        rows,
        &vec![vec![serde_json::json!("AIS"), serde_json::json!(false)]]
    );
    assert_eq!(app.state.live_events.len(), 1);
    assert!(!app.state.live_events[0].snapshot);
}

#[test]
fn guided_write_locks_same_target_is_locked_regardless_of_plan_id_or_mutation() {
    let mut locks = GuidedWriteLocks::default();
    let first = make_write_plan(WritePlanId(1), "db", Some("public"), "users", 7, true);
    let same_target = make_write_plan(WritePlanId(2), "db", Some("public"), "users", 7, false);

    assert!(locks.acquire(&first));
    assert!(locks.is_locked(&same_target));
    assert!(!locks.acquire(&same_target));
}

#[test]
fn guided_write_locks_same_complete_key_blocks_across_table_generations() {
    let mut locks = GuidedWriteLocks::default();
    let mut first = make_write_plan(WritePlanId(1), "db", Some("public"), "users", 7, true);
    let mut same_row_newer_generation =
        make_write_plan(WritePlanId(2), "db", Some("public"), "users", 7, false);
    first.row_generation = 11;
    same_row_newer_generation.row_generation = 12;

    assert!(locks.acquire(&first));
    assert!(locks.is_locked(&same_row_newer_generation));
    assert!(!locks.acquire(&same_row_newer_generation));

    let released = locks.release(first.id).expect("release preserves metadata");
    assert_eq!(released.table_generation, 11);
}

#[test]
fn guided_write_locks_different_database_table_or_key_are_not_locked() {
    let mut locks = GuidedWriteLocks::default();
    let base = make_write_plan(WritePlanId(1), "db", Some("public"), "users", 7, true);
    let different_database =
        make_write_plan(WritePlanId(2), "other", Some("public"), "users", 7, true);
    let different_table = make_write_plan(WritePlanId(3), "db", Some("public"), "posts", 7, true);
    let different_key = make_write_plan(WritePlanId(4), "db", Some("public"), "users", 8, true);

    assert!(locks.acquire(&base));
    assert!(!locks.is_locked(&different_database));
    assert!(!locks.is_locked(&different_table));
    assert!(!locks.is_locked(&different_key));
}

#[test]
fn guided_write_locks_removing_by_plan_id_releases_target() {
    let mut locks = GuidedWriteLocks::default();
    let first = make_write_plan(WritePlanId(1), "db", Some("public"), "users", 7, true);
    let same_target = make_write_plan(WritePlanId(2), "db", Some("public"), "users", 7, false);

    assert!(locks.acquire(&first));
    let released = locks
        .release(WritePlanId(1))
        .expect("owned target released");
    assert_eq!(released, GuidedWriteTarget::from_plan(&first));
    assert!(locks.release(WritePlanId(1)).is_none());
    assert!(!locks.is_locked(&same_target));
    assert!(locks.acquire(&same_target));
}

#[test]
fn postcondition_fetch_failures_map_critical_multiplicity_and_unknown_transport() {
    let critical = verification_report_from_postcondition_fetch_error(
        PostconditionFetchError::critical("complete-key new tuple verification returned 2 rows"),
    );
    assert!(matches!(
        critical.outcome,
        MutationOutcome::CriticalSafetyError { ref reason }
            if reason.contains("complete-key new tuple")
    ));

    let unknown = verification_report_from_postcondition_fetch_error(
        PostconditionFetchError::unknown("postcondition verification timed out"),
    );
    assert!(matches!(
        unknown.outcome,
        MutationOutcome::Unknown { ref reason }
            if reason.contains("timed out")
    ));
}

fn make_write_plan(
    id: WritePlanId,
    database: &str,
    schema: Option<&str>,
    table: &str,
    pk: u64,
    update: bool,
) -> WritePlan {
    use crate::state::safety::{
        CompletePrimaryKey, GuidedMutation, PrimaryKeyPart, QualifiedTable, SqlValue,
    };

    let original_primary_key =
        CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart {
            column_id: 1,
            value: SqlValue::U64(pk),
        }])
        .expect("test primary key should be complete");
    let mutation = if update {
        GuidedMutation::Update { changes: vec![] }
    } else {
        GuidedMutation::Delete
    };

    WritePlan {
        id,
        table: QualifiedTable {
            database: database.to_string(),
            schema: schema.map(str::to_string),
            table: table.to_string(),
        },
        original_primary_key,
        schema_generation: 0,
        row_generation: 0,
        server_context_generation: 0,
        database_generation: 0,
        mutation,
    }
}

#[test]
fn extract_field_name_strips_type_suffix() {
    assert_eq!(extract_field_name("name (String)"), "name");
    assert_eq!(extract_field_name("user_id (U64 — auto)"), "user_id");
    assert_eq!(extract_field_name(""), "");
}

#[test]
fn coerce_field_to_json_numeric_types() {
    assert_eq!(coerce_field_to_json("42", "U64"), serde_json::json!(42));
    assert_eq!(coerce_field_to_json("-7", "I32"), serde_json::json!(-7));
    assert_eq!(coerce_field_to_json("1.5", "F64"), serde_json::json!(1.5));
}

#[test]
fn coerce_field_to_json_bool_and_string() {
    assert_eq!(
        coerce_field_to_json("true", "Bool"),
        serde_json::json!(true)
    );
    assert_eq!(
        coerce_field_to_json("hello", "String"),
        serde_json::json!("hello")
    );
}

#[test]
fn coerce_field_to_json_passes_through_json_arrays() {
    assert_eq!(
        coerce_field_to_json("[1,2,3]", "Array"),
        serde_json::json!([1, 2, 3])
    );
}

#[test]
fn coerce_field_to_json_empty_is_null() {
    assert_eq!(
        coerce_field_to_json("   ", "String"),
        serde_json::Value::Null
    );
}

#[test]
fn sql_literal_quotes_strings_and_escapes_quotes() {
    assert_eq!(sql_literal("alice", "String"), "'alice'");
    assert_eq!(sql_literal("O'Brien", "String"), "'O''Brien'");
}

#[test]
fn sql_literal_emits_numbers_bare() {
    assert_eq!(sql_literal("42", "U64"), "42");
    assert_eq!(sql_literal("-1.5", "F32"), "-1.5");
}

#[test]
fn sql_literal_bool_to_keyword() {
    assert_eq!(sql_literal("true", "Bool"), "TRUE");
    assert_eq!(sql_literal("false", "Bool"), "FALSE");
    assert_eq!(sql_literal("1", "Bool"), "TRUE");
    assert_eq!(sql_literal("nope", "Bool"), "FALSE");
}

#[test]
fn sql_literal_empty_is_null() {
    assert_eq!(sql_literal("  ", "String"), "NULL");
}

#[test]
fn sql_literal_identity_type_emits_hex() {
    // A user typing a 0x-prefixed hex value into an Identity
    // form field must not end up single-quoted.
    assert_eq!(sql_literal("0xdeadbeef", "Identity"), "0xdeadbeef");
    assert_eq!(sql_literal("0xfeed", "ConnectionId"), "0xfeed");
}

#[test]
fn sql_literal_identity_type_without_prefix_adds_0x() {
    // Same thing but without the `0x` the user omitted.
    assert_eq!(sql_literal("deadbeef", "Identity"), "0xdeadbeef");
}

#[test]
fn sql_literal_identity_type_non_hex_falls_through_to_quoted() {
    // Garbage input for an Identity column shouldn't silently
    // produce a bad hex literal — let the server reject a
    // quoted string so the user sees an error.
    let lit = sql_literal("not-hex!", "Identity");
    assert_eq!(lit, "'not-hex!'");
}
