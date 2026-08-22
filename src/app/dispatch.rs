//! Central async event dispatcher: maps background-task AppEvents to
//! state mutations across all tabs.

use super::*;

impl super::App {
    // ── Async event handler ───────────────────────────────────────────────

    pub(crate) async fn handle_app_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::PingResult(ok) => {
                if ok {
                    self.state.connection.status = ConnectionStatus::Connected;
                } else {
                    self.state.connection.status =
                        ConnectionStatus::Error("Server unreachable".to_string());
                }
            }

            AppEvent::DatabasesLoaded {
                context,
                databases: dbs,
            } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.connection.status = ConnectionStatus::Connected;
                // Preserve any pre-selected DB
                let existing = std::mem::take(&mut self.state.databases);
                self.state.databases = dbs;
                for db in existing {
                    if !self.state.databases.contains(&db) {
                        self.state.databases.insert(0, db);
                    }
                }
                // If a previous session left a "last database" hint
                // and we still have no selection, try to land on it.
                if self.state.selected_database_idx.is_none() {
                    if let Some(session) = self.pending_session.as_ref() {
                        if let Some(ref last_db) = session.last_database {
                            if let Some(idx) =
                                self.state.databases.iter().position(|d| d == last_db)
                            {
                                let last_tab = session.last_tab;
                                self.state.select_database(idx);
                                self.bump_database_generation();
                                if let Some(tab_idx) = last_tab {
                                    self.state.current_tab = index_to_tab(tab_idx);
                                }
                                self.load_schema().await;
                            }
                        }
                    }
                }
                if !self.state.databases.is_empty() && self.state.selected_database_idx.is_none() {
                    self.state.select_database(0);
                    self.bump_database_generation();
                    self.load_schema().await;
                }
            }

            AppEvent::SchemaLoaded { context, schema } => {
                if !self.schema_context_matches_current(&context) {
                    return;
                }
                let deferred_refresh_before_connect = self
                    .deferred_guided_refresh
                    .take()
                    .filter(|refresh| self.deferred_refresh_matches_current(refresh));
                self.state.schema_loading = false;
                self.state.schema_load_failed = false;
                self.state.tables = schema.tables.clone();
                // If we restored a session and the user was looking at
                // a specific table, jump to it instead of defaulting
                // to row 0.
                if !self.state.tables.is_empty() && self.state.selected_table_idx.is_none() {
                    let restored = self
                        .pending_session
                        .as_ref()
                        .and_then(|s| s.last_table.as_deref());
                    self.state.selected_table_idx = self.state.preferred_table_index(restored);
                }
                if self.state.selected_table_idx.is_some() {
                    self.state.sidebar_focus = SidebarFocus::Tables;
                }
                // Session restore is one-shot — don't keep firing it
                // every time the user navigates to a new database.
                self.pending_session = None;
                self.state.current_schema = Some(schema);
                let table_count = self.state.tables.len();
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("Schema loaded — {table_count} tables")),
                );
                // Establish WebSocket subscription for live data
                self.connect_ws().await;
                if self.state.selected_table_idx.is_some()
                    && self.state.table_browse_result.is_none()
                {
                    self.load_table_data(TableBrowseOrigin::Navigation).await;
                }
                if let Some(refresh) = deferred_refresh_before_connect {
                    if self.selected_table_matches(&refresh.table) {
                        self.deferred_guided_refresh =
                            Some(self.deferred_table_refresh_for(refresh.table));
                    }
                }
                self.flush_deferred_guided_refresh_if_unowned().await;
            }

            AppEvent::SchemaError { context, error } => {
                if !self.schema_context_matches_current(&context) {
                    return;
                }
                // Clear the in-flight flag so the sidebar drops its
                // "(loading…)" placeholder, then flip the terminal
                // "failed" flag so it can show an error hint instead.
                self.state.schema_loading = false;
                self.state.schema_load_failed = true;
                self.state.set_error(error);
                self.flush_deferred_guided_refresh_if_unowned().await;
            }

            AppEvent::QueryResult {
                context,
                result,
                duration,
                sql,
            } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.query_loading = false;
                let row_count = result.row_count();
                self.state.query_result = Some(result);
                // Reset grid scroll on new results
                self.tables_grid = TableGridState::new();
                self.sql_grid = TableGridState::new();

                // Push to history
                self.state.push_sql_history(SqlHistoryEntry {
                    sql,
                    executed_at: chrono::Utc::now(),
                    duration,
                    row_count: Some(row_count),
                    error: None,
                });
                self.state
                    .set_notification(format!("{row_count} rows returned"));
                self.flush_deferred_guided_refresh_if_unowned().await;
            }

            AppEvent::QueryError {
                context,
                sql,
                error,
            } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.query_loading = false;
                self.state.push_sql_history(SqlHistoryEntry {
                    sql,
                    executed_at: chrono::Utc::now(),
                    duration: Duration::ZERO,
                    row_count: None,
                    error: Some(error.clone()),
                });
                self.state.set_error(error);
                self.flush_deferred_guided_refresh_if_unowned().await;
            }

            AppEvent::TableBrowseResult {
                context,
                mut result,
                total_rows,
            } => {
                if !self.table_context_matches_current(&context) {
                    return;
                }
                self.state.query_loading = false;
                // Cut the visible page out of the fetched prefix. The
                // offset may clamp down when rows were deleted while
                // browsing past the new end of the table.
                let (start, end, effective_offset) =
                    browse_page_bounds(result.rows.len(), self.state.browse_offset);
                self.state.browse_offset = effective_offset;
                result.rows = result.rows[start..end].to_vec();
                let row_count = result.row_count();
                self.state.table_browse_result = Some(result);
                self.state.browse_total_rows = total_rows;
                self.unblock_guided_writes_after_accepted_browse(&context);
                // Reset the Tables grid scroll/selection on fresh data.
                self.tables_grid = TableGridState::new();
                self.state
                    .set_notification(format!("{row_count} rows loaded"));
                self.flush_deferred_guided_refresh_if_unowned().await;
                // Integer-PK tables wait for browse before opening a live
                // tail (`WHERE pk > max`). Timestamp tables already subscribed.
                self.ensure_live_subscription().await;
            }

            AppEvent::TableBrowseError { context, error } => {
                if !self.table_context_matches_current(&context) {
                    return;
                }
                self.state.query_loading = false;
                self.state.set_error(error);
                self.flush_deferred_guided_refresh_if_unowned().await;
            }

            AppEvent::LogsLoaded { context, logs } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.extend_logs(logs);
                self.state.set_notification("Logs refreshed".to_string());
            }

            AppEvent::MetricsLoaded { context, snapshot } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.update_metrics(snapshot);
            }

            AppEvent::LiveClientsLoaded { context, clients } => {
                if !self.apply_result_if_current(&context) {
                    return;
                }
                self.state.live_clients = clients;
            }

            AppEvent::WriteOpSuccess { op, response } => {
                let summary = if response.is_null() {
                    op.clone()
                } else {
                    let s = response.to_string();
                    let preview: String = s.chars().take(60).collect();
                    format!("{op} → {preview}")
                };
                self.state.set_notification(format!("✓ {summary}"));
                // Many writes invalidate the table-browse view, so a
                // gentle refresh is useful — but only when the user
                // is still looking at the Tables tab.
                if self.state.current_tab == Tab::Tables {
                    let selected_target = self.state.selected_database().map(str::to_string).zip(
                        self.state
                            .selected_table()
                            .map(|table| table.table_name.clone()),
                    );
                    if let Some((database, table)) = selected_target {
                        self.refresh_or_defer_guided_table(QualifiedTable {
                            database,
                            schema: None,
                            table,
                        })
                        .await;
                    }
                }
            }

            AppEvent::WriteOpError { op, error } => {
                self.state.set_error(format!("{op} failed: {error}"));
            }

            AppEvent::GuidedWriteVerification {
                plan_id,
                op,
                report,
            } => {
                let Some(target) = self.guided_write_locks.release(plan_id) else {
                    return;
                };
                match &report.outcome {
                    MutationOutcome::SentAndConfirmed { .. } => {
                        // A verified write makes our cached rows a pre-write
                        // snapshot. Invalidate any same-row owner that still
                        // believes otherwise, then raise the weak staleness
                        // barrier so nothing can be written from those rows
                        // until a reload lands. This is not the manual-refresh
                        // block: the refresh queued below clears it.
                        let invalidated_stale_owner =
                            self.discard_same_target_pending_guided_update(&target);
                        if invalidated_stale_owner {
                            self.mark_guided_write_data_stale(&target.table);
                        }
                        if self.spreadsheet_lifecycle_plan_id() == Some(plan_id) {
                            self.clear_spreadsheet_edit_state();
                        }
                        self.state.set_notification(format!("✓ {op} verified"));
                        if self.state.current_tab == Tab::Tables
                            && self
                                .state
                                .selected_table()
                                .is_some_and(|table| table.table_name == target.table.table)
                            && self.state.selected_database()
                                == Some(target.table.database.as_str())
                        {
                            self.refresh_or_defer_guided_table(target.table).await;
                        } else if invalidated_stale_owner {
                            // Off-tab (or off-table) successes still owe the
                            // user a reload; queue it so returning to the table
                            // clears the barrier without a manual refresh.
                            self.deferred_guided_refresh =
                                Some(self.deferred_table_refresh_for(target.table));
                        }
                        self.flush_deferred_guided_refresh_if_unowned().await;
                    }
                    MutationOutcome::Conflict { reason }
                    | MutationOutcome::Unknown { reason }
                    | MutationOutcome::CriticalSafetyError { reason } => {
                        let matching_lifecycle = self
                            .spreadsheet_save_lifecycle
                            .filter(|lifecycle| lifecycle.plan_id() == plan_id);
                        if let Some(lifecycle) = matching_lifecycle {
                            if lifecycle.is_invalidated() {
                                self.clear_spreadsheet_edit_state();
                            } else {
                                self.spreadsheet_save_lifecycle = None;
                            }
                        }
                        self.block_guided_writes_for_target(&target);
                        self.state
                            .set_error(format!("{op} requires manual refresh: {reason}"));
                        self.state.push_log(crate::api::types::LogEntry {
                            ts: Some(chrono::Utc::now()),
                            level: crate::api::types::LogLevel::Warn,
                            message: format!("guided write outcome for {op}: {reason}"),
                            target: Some("guided-write-safety".to_string()),
                            filename: None,
                            line_number: None,
                        });
                        self.flush_deferred_guided_refresh_if_unowned().await;
                    }
                    MutationOutcome::DefinitelyNotSent { reason } => {
                        if self.spreadsheet_lifecycle_plan_id() == Some(plan_id) {
                            self.spreadsheet_save_lifecycle = None;
                        }
                        self.state.set_error(format!("{op} was not sent: {reason}"));
                        self.flush_deferred_guided_refresh_if_unowned().await;
                    }
                }
            }

            AppEvent::LogLine(entry) => {
                self.state.push_log(entry);
            }

            AppEvent::Notification(msg) => {
                self.state.set_notification(msg);
            }

            AppEvent::Error(msg) => {
                self.state.set_error(msg);
            }
        }
    }
}
