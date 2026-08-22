//! Request-scoped context tracking: generation counters, staleness checks
//! for superseded schema/table-browse responses, and write-error explainers.

use super::*;

impl super::App {
    pub(crate) fn clear_table_browse_for_selection_change(&mut self) {
        self.state.table_browse_result = None;
        self.state.query_loading = false;
        // Pagination restarts from the first page on a selection change.
        self.state.browse_offset = 0;
        self.state.browse_total_rows = None;
        self.tables_grid = TableGridState::new();
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    pub(crate) fn current_schema_request_context(&self, database: String) -> SchemaRequestContext {
        SchemaRequestContext {
            database,
            schema_generation: self.schema_generation,
            database_generation: self.database_generation,
        }
    }

    /// Issue a fresh `RequestContext` for the given scope.
    ///
    /// Each call bumps both the global request id and the per-scope generation,
    /// then records the context as the latest for that scope. A later call with
    /// the same scope supersedes the previous one, so any response carrying the
    /// old context will be rejected by [`Self::apply_result_if_current`].
    pub(crate) fn next_request_context(
        &mut self,
        scope: crate::effects::request::RequestScope,
    ) -> crate::effects::request::RequestContext {
        self.next_request_id = self.next_request_id.saturating_add(1);
        let generation = self
            .latest_requests
            .get(&scope)
            .map(|current| current.generation.saturating_add(1))
            .unwrap_or(1);
        let context = crate::effects::request::RequestContext::new(
            crate::effects::request::RequestId::from_u64(self.next_request_id),
            scope,
            generation,
        );
        tracing::debug!(
            "request #{} scope {:?} db={:?} gen={}",
            context.id.get(),
            context.scope(),
            context.scope().database(),
            context.generation
        );
        self.latest_requests
            .insert(context.scope().clone(), context.clone());
        context
    }

    /// Return `true` if `delivered` is still the latest context for its scope.
    pub(crate) fn should_apply_result(
        latest: &HashMap<
            crate::effects::request::RequestScope,
            crate::effects::request::RequestContext,
        >,
        delivered: &crate::effects::request::RequestContext,
    ) -> bool {
        latest
            .get(delivered.scope())
            .is_some_and(|current| current.accepts(delivered))
    }

    /// Apply a delivered result only if it matches the latest issued context
    /// for its scope. Stale results are silently counted and dropped.
    ///
    /// Note: `SchemaLoaded`/`SchemaError` and `TableBrowseResult`/`TableBrowseError`
    /// still use their own `SchemaRequestContext`/`TableBrowseRequestContext` with
    /// generation-snapshot matching. Unifying them under this `RequestContext`
    /// system is tracked as tech debt.
    pub(crate) fn apply_result_if_current(
        &mut self,
        delivered: &crate::effects::request::RequestContext,
    ) -> bool {
        let should_apply = Self::should_apply_result(&self.latest_requests, delivered);
        if !should_apply {
            self.ignored_stale_results = self.ignored_stale_results.saturating_add(1);
        }
        should_apply
    }

    pub(crate) fn current_table_browse_request_context(
        &self,
        database: String,
        table: String,
        origin: TableBrowseOrigin,
    ) -> TableBrowseRequestContext {
        TableBrowseRequestContext {
            database,
            schema: None,
            table,
            origin,
            server_context_generation: self.server_context_generation,
            table_generation: self.table_generation,
            schema_generation: self.schema_generation,
            database_generation: self.database_generation,
        }
    }

    pub(crate) fn schema_context_matches_current(&self, context: &SchemaRequestContext) -> bool {
        self.state.selected_database() == Some(context.database.as_str())
            && self.schema_generation == context.schema_generation
            && self.database_generation == context.database_generation
    }

    pub(crate) fn table_context_matches_current(
        &self,
        context: &TableBrowseRequestContext,
    ) -> bool {
        self.state.selected_database() == Some(context.database.as_str())
            && self
                .state
                .selected_table()
                .is_some_and(|table| table.table_name == context.table)
            && self.table_generation == context.table_generation
            && self.schema_generation == context.schema_generation
            && self.database_generation == context.database_generation
            && self.server_context_generation == context.server_context_generation
    }

    pub(crate) fn bump_schema_generation(&mut self) {
        self.schema_generation = self.schema_generation.saturating_add(1);
        self.discard_pending_guided_writes();
    }

    pub(crate) fn bump_table_generation(&mut self) {
        self.table_generation = self.table_generation.saturating_add(1);
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    pub(crate) fn bump_server_context_generation(&mut self) {
        self.server_context_generation = self.server_context_generation.saturating_add(1);
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    pub(crate) fn bump_database_generation(&mut self) {
        self.database_generation = self.database_generation.saturating_add(1);
        // A database switch invalidates every block and staleness barrier:
        // they are keyed by (database, schema, table) and the old entries
        // would otherwise accumulate without bound across switches.
        self.guided_write_blocks.clear();
        self.guided_write_stale_data.clear();
        self.bump_schema_generation();
        self.bump_table_generation();
    }

    pub(crate) fn selected_database_for_modal(&mut self) -> Option<String> {
        match self.state.selected_database() {
            Some(database) => Some(database.to_string()),
            None => {
                self.state.set_error("No database selected".to_string());
                None
            }
        }
    }

    pub(crate) fn qualified_selected_table(&mut self, table: &TableInfo) -> Option<QualifiedTable> {
        let database = self.selected_database_for_modal()?;
        Some(QualifiedTable {
            database,
            schema: None,
            table: table.table_name.clone(),
        })
    }

    pub(crate) fn explain_primary_key_error(table: &TableInfo, error: PrimaryKeyError) -> String {
        let column_name = |column_id: u16| {
            table
                .columns
                .iter()
                .find(|column| column.col_id == u32::from(column_id))
                .map(|column| column.col_name.as_str())
        };

        match error {
            PrimaryKeyError::NoDeclaredPrimaryKey => "missing declared primary key".to_string(),
            PrimaryKeyError::DuplicateColumnId(column_id) => {
                format!("duplicate primary key column id {column_id}")
            }
            PrimaryKeyError::UnknownColumnId(column_id) => {
                format!("primary key column id {column_id} is not in the table schema")
            }
            PrimaryKeyError::ColumnIdOutOfRange { column_id } => {
                format!("primary key column id {column_id} is out of range")
            }
            PrimaryKeyError::MissingValue { column_id } => match column_name(column_id) {
                Some(name) => format!("primary key column '{name}' is missing from the row"),
                None => format!("primary key column id {column_id} is missing from the row"),
            },
            PrimaryKeyError::NullValue { column_id } => match column_name(column_id) {
                Some(name) => format!("primary key column '{name}' is null"),
                None => format!("primary key column id {column_id} is null"),
            },
            PrimaryKeyError::UnrepresentableValue { column_id } => match column_name(column_id) {
                Some(name) => format!("primary key column '{name}' cannot be represented safely"),
                None => format!("primary key column id {column_id} cannot be represented safely"),
            },
            PrimaryKeyError::TypeMismatch {
                column_id,
                expected,
            } => match column_name(column_id) {
                Some(name) => format!("primary key column '{name}' must be {expected}"),
                None => format!("primary key column id {column_id} must be {expected}"),
            },
        }
    }

    pub(crate) fn explain_write_plan_build_error(error: WritePlanBuildError) -> String {
        match error {
            WritePlanBuildError::PrimaryKey(error) => format!("primary key error: {error:?}"),
            WritePlanBuildError::NoChangedFields => "No changed fields".to_string(),
            WritePlanBuildError::TypeMismatch {
                column_id,
                expected,
            } => format!("column {column_id} must be {expected}"),
            WritePlanBuildError::Encoding(error) => format!("SQL encoding error: {error:?}"),
        }
    }
}
