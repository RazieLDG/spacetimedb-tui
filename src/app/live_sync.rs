//! Live WebSocket sync: task report draining, WS event handling,
//! scoped subscription management, and browse-result merging.

use super::*;

impl super::App {
    /// Drain terminal reports from the background-task registry.
    ///
    /// Panicked tasks are surfaced into the Activity log and as an error
    /// notification so a background panic is never silently lost. Completed
    /// and cancelled tasks are recorded at debug level only.
    pub(crate) fn drain_task_reports(&mut self) {
        while let Some(report) = self.task_registry.try_join_next() {
            match report.outcome {
                crate::effects::task_registry::TaskOutcome::Panicked { ref message } => {
                    tracing::error!("background task '{}' panicked: {message}", report.name);
                    self.state.push_log(crate::api::types::LogEntry {
                        ts: Some(chrono::Utc::now()),
                        level: crate::api::types::LogLevel::Panic,
                        message: format!("background task '{}' panicked: {message}", report.name),
                        target: Some("spacetimedb-tui::task_registry".to_string()),
                        filename: None,
                        line_number: None,
                    });
                    send_event(
                        &self.event_tx,
                        AppEvent::Error(format!(
                            "Background task '{}' crashed: {message}",
                            report.name
                        )),
                    );
                }
                crate::effects::task_registry::TaskOutcome::Cancelled => {
                    tracing::debug!("background task '{}' cancelled", report.name);
                }
                crate::effects::task_registry::TaskOutcome::Completed => {
                    tracing::debug!("background task '{}' completed", report.name);
                }
            }
        }
    }

    /// Handle a single WebSocket event.
    pub(crate) async fn handle_ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Connected => {
                tracing::info!("WebSocket connected");
                self.state.ws_connected = true;
                self.state.ws_reconnect_deadline = None;
                self.state.ws_reconnect_attempt = 0;
                self.ensure_live_subscription().await;
            }
            WsEvent::ServerMessage(msg) => {
                self.handle_ws_server_message(msg);
            }
            WsEvent::LogLine(entry) => {
                send_event(&self.event_tx, AppEvent::LogLine(entry));
            }
            WsEvent::Disconnected { reason } => {
                tracing::warn!("WebSocket disconnected: {reason}");
                self.state.ws_connected = false;
                // If the disconnect was flagged as permanent
                // ("(retries disabled)" marker from subscription_task),
                // clear the countdown so the status bar doesn't keep
                // showing a stale "reconnect in Ns" pill forever.
                if reason.contains("(retries disabled)") {
                    self.state.ws_reconnect_deadline = None;
                    self.state.ws_reconnect_attempt = 0;
                }
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("WebSocket disconnected: {reason}")),
                );
            }
            WsEvent::Reconnecting { attempt, delay_ms } => {
                tracing::info!("WebSocket reconnect attempt {attempt} in {delay_ms}ms");
                self.state.ws_reconnect_attempt = attempt;
                self.state.ws_reconnect_deadline =
                    Some(Instant::now() + Duration::from_millis(delay_ms));
                // No notification here — the status bar renders a live
                // countdown from `ws_reconnect_deadline` so a persistent
                // toast would just duplicate the information.
            }
            WsEvent::Error(e) => {
                tracing::warn!("WebSocket error: {e}");
                if e.contains("smaller window") || e.contains("size limit") {
                    let previous = self.live_time_window_us;
                    self.live_time_window_us = (self.live_time_window_us / 4).max(0);
                    self.live_subscribed = None;
                    if previous == 0 {
                        self.state.set_notification(
                            "Live snapshot is too large even for new rows; live paused for this table"
                                .to_string(),
                        );
                    } else {
                        self.state.set_notification(format!(
                            "Live window shrunk to {}ms to stay under the WebSocket size limit",
                            self.live_time_window_us / 1000
                        ));
                        self.ensure_live_subscription().await;
                    }
                }
            }
            WsEvent::RawText(text) => {
                // Raw frames we can't decode as structured messages — log for diagnostics
                tracing::debug!("WebSocket raw text frame ({} bytes)", text.len());
            }
        }
    }

    /// Apply a decoded WebSocket server message to the application state.
    pub(crate) fn handle_ws_server_message(&mut self, msg: crate::api::types::WsServerMessage) {
        use crate::api::types::WsServerMessage;
        match msg {
            WsServerMessage::InitialSubscription(payload) => {
                self.apply_live_table_updates(payload.database_update.tables, true);
            }
            WsServerMessage::SubscribeMultiApplied(payload) => {
                self.apply_live_table_updates(payload.update.tables, true);
            }
            WsServerMessage::SubscribeApplied(payload) => {
                if let Some(rows) = payload.rows.and_then(|r| r.table_rows) {
                    self.apply_live_table_updates(vec![rows], true);
                }
            }
            WsServerMessage::TransactionUpdate(payload) => {
                self.apply_live_table_updates(payload.table_updates(), false);
            }
            WsServerMessage::TransactionUpdateLight(payload) => {
                self.apply_live_table_updates(payload.update.tables, false);
            }
            WsServerMessage::SubscriptionError(payload) => {
                let error = if payload.error.is_empty() {
                    "Live subscription error".to_string()
                } else {
                    payload.error
                };
                tracing::warn!("Live subscription error: {error}");
                self.state
                    .set_notification(format!("Live subscription error: {error}"));
            }
            WsServerMessage::IdentityToken(payload) => {
                tracing::info!("WebSocket identity confirmed: {:?}", payload.identity);
            }
        }
    }

    pub(crate) async fn toggle_live(&mut self) {
        self.state.live_enabled = !self.state.live_enabled;
        if self.state.live_enabled {
            self.live_time_window_us = crate::api::live_subscribe::DEFAULT_LIVE_WINDOW_US;
            self.state
                .set_notification("Live updates on — scoped to the selected table".to_string());
            self.ensure_live_subscription().await;
        } else {
            self.state.live_table_data.clear();
            self.live_subscribed = None;
            self.state
                .set_notification("Live updates off — use r to refresh".to_string());
            if let Some(handle) = self.ws_handle.as_ref() {
                for query_id in self.live_query_ids.drain(..) {
                    self.live_request_id = self.live_request_id.wrapping_add(1);
                    let _ = handle.unsubscribe(self.live_request_id, query_id).await;
                }
            }
        }
    }

    pub(crate) async fn ensure_live_subscription(&mut self) {
        if !self.state.live_enabled || !self.state.ws_connected {
            return;
        }
        let Some(table) = self.state.selected_table().cloned() else {
            return;
        };
        let table_name = table.table_name.clone();
        let window = self.live_time_window_us;
        if self.live_subscribed.as_ref() == Some(&(table_name.clone(), window)) {
            return;
        }

        let now_us = chrono::Utc::now().timestamp_micros();
        let browse = self.state.table_browse_result.clone();
        let plan = crate::api::live_subscribe::plan_live_subscribe(
            &table,
            now_us,
            window,
            browse.as_ref(),
        );
        let queries = match plan {
            crate::api::live_subscribe::LiveSubscribePlan::Queries(queries)
                if !queries.is_empty() =>
            {
                queries
            }
            crate::api::live_subscribe::LiveSubscribePlan::Queries(_)
            | crate::api::live_subscribe::LiveSubscribePlan::NotReady { .. } => {
                // Wait for browse (integer PK tail) or skip unbounded tables.
                return;
            }
        };

        let Some(handle) = self.ws_handle.as_ref() else {
            return;
        };
        for query_id in self.live_query_ids.drain(..) {
            self.live_request_id = self.live_request_id.wrapping_add(1);
            if let Err(error) = handle.unsubscribe(self.live_request_id, query_id).await {
                tracing::warn!("Live unsubscribe failed: {error}");
            }
        }
        self.live_request_id = self.live_request_id.wrapping_add(1);
        let _ = handle.subscribe(Vec::new(), self.live_request_id).await;
        for query in queries {
            self.next_live_query_id = self.next_live_query_id.wrapping_add(1);
            self.live_request_id = self.live_request_id.wrapping_add(1);
            let query_id = self.next_live_query_id;
            if let Err(error) = handle
                .subscribe_single(query, self.live_request_id, query_id)
                .await
            {
                tracing::warn!("Live subscribe failed: {error}");
                self.state
                    .set_notification(format!("Live subscribe failed: {error}"));
                return;
            }
            self.live_query_ids.push(query_id);
        }
        self.live_subscribed = Some((table_name.clone(), window));
        self.state.set_notification(format!(
            "Live watching recent rows in {} chunk(s) — not the full table",
            self.live_query_ids.len()
        ));
    }

    pub(crate) fn apply_live_table_updates(
        &mut self,
        tables: Vec<crate::api::types::TableUpdate>,
        snapshot: bool,
    ) {
        if !self.state.live_enabled || tables.is_empty() {
            return;
        }
        let selected = self
            .state
            .selected_table()
            .map(|table| table.table_name.clone());
        let mut total_inserts = 0usize;
        let mut total_deletes = 0usize;

        for table_update in tables {
            let inserts = table_update.insert_rows();
            let deletes = table_update.delete_rows();
            total_inserts += inserts.len();
            total_deletes += deletes.len();
            let name = table_update.table_name;

            self.state
                .push_live_event(crate::state::app_state::LiveEvent {
                    at: chrono::Utc::now(),
                    table_name: name.clone(),
                    inserts: inserts.len(),
                    deletes: deletes.len(),
                    snapshot,
                });

            let entry = self.state.live_table_data.entry(name.clone()).or_default();
            if snapshot {
                *entry = inserts.clone();
            } else {
                if !deletes.is_empty() {
                    entry
                        .retain(|row| !deletes.iter().any(|deleted| live_rows_match(row, deleted)));
                }
                entry.extend(inserts.iter().cloned());
            }
            if entry.len() > LIVE_TABLE_ROW_LIMIT {
                let drop_n = entry.len() - LIVE_TABLE_ROW_LIMIT;
                entry.drain(0..drop_n);
            }

            if selected.as_deref() == Some(name.as_str()) {
                self.merge_live_into_browse(&name, &inserts, &deletes, snapshot);
            }
        }

        if snapshot {
            tracing::debug!("Live snapshot chunk: {total_inserts} rows");
        } else if total_inserts + total_deletes > 0 {
            tracing::debug!("Live update: +{total_inserts} -{total_deletes} row changes");
        }
    }

    pub(crate) fn merge_live_into_browse(
        &mut self,
        table_name: &str,
        inserts: &[serde_json::Value],
        deletes: &[serde_json::Value],
        snapshot: bool,
    ) {
        if self.state.edit_mode.is_some() {
            return;
        }
        let columns: Vec<String> = if let Some(result) = self.state.table_browse_result.as_ref() {
            result
                .column_names()
                .into_iter()
                .map(str::to_string)
                .collect()
        } else if let Some(table) = self
            .state
            .tables
            .iter()
            .find(|table| table.table_name == table_name)
        {
            table
                .columns
                .iter()
                .map(|col| col.col_name.clone())
                .collect()
        } else {
            Vec::new()
        };
        if columns.is_empty() && self.state.table_browse_result.is_none() {
            return;
        }

        let delete_cells: Vec<Vec<serde_json::Value>> = deletes
            .iter()
            .map(|row| crate::api::types::ws_row_to_cells(row, &columns))
            .collect();
        let insert_cells: Vec<Vec<serde_json::Value>> = inserts
            .iter()
            .map(|row| crate::api::types::ws_row_to_cells(row, &columns))
            .collect();

        if let Some(result) = self.state.table_browse_result.as_mut() {
            if !snapshot {
                if !delete_cells.is_empty() {
                    result
                        .rows
                        .retain(|row| !delete_cells.iter().any(|deleted| deleted == row));
                }
                result.rows.extend(insert_cells);
                if result.rows.len() > LIVE_BROWSE_ROW_LIMIT {
                    let drop_n = result.rows.len() - LIVE_BROWSE_ROW_LIMIT;
                    result.rows.drain(0..drop_n);
                }
            }
            return;
        }

        if snapshot && !self.state.query_loading {
            let schema: Vec<crate::api::types::SchemaElement> = self
                .state
                .tables
                .iter()
                .find(|table| table.table_name == table_name)
                .map(|table| {
                    table
                        .columns
                        .iter()
                        .map(|col| crate::api::types::SchemaElement {
                            name: col.col_name.clone(),
                            algebraic_type: col.col_type.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            if schema.is_empty() {
                return;
            }
            let mut rows = insert_cells;
            rows.truncate(LIVE_BROWSE_ROW_LIMIT);
            self.state.table_browse_result = Some(crate::api::types::QueryResult {
                schema,
                rows,
                total_duration_micros: 0,
            });
        }
    }
}
