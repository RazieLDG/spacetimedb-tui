//! Data-source plumbing: SQL execution/completion, view refresh, log
//! loading, WebSocket connection, and background task shutdown.

use super::*;

impl super::App {
    /// Tab-complete the SQL input against the current schema.
    ///
    /// Extracts the identifier token immediately to the left of the cursor,
    /// builds a candidate list from SQL keywords plus every table/column
    /// name in the active schema, and then either (a) commits the unique
    /// completion, (b) extends the token to the longest common prefix
    /// shared by multiple matches and surfaces the candidate list as a
    /// notification, or (c) shows a "no match" notification.
    pub(crate) fn complete_sql_input(&mut self) {
        use crate::ui::components::completion::{build_candidates, complete, CompletionResult};

        let (range, word) = self.sql_input.current_word();
        if word.is_empty() {
            return;
        }
        let word = word.to_string();

        let candidates = build_candidates(self.state.tables.iter());
        let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();

        match complete(&word, &refs) {
            CompletionResult::NoMatch => {
                self.state
                    .set_notification(format!("No match for \"{word}\""));
            }
            CompletionResult::Unique(hit) => {
                self.sql_input.replace_range(range, &hit);
            }
            CompletionResult::Multiple {
                common_prefix,
                candidates,
            } => {
                // Extend the input to the longest common prefix (if any),
                // then show the user what's still ambiguous.
                if common_prefix.len() > word.len() {
                    self.sql_input.replace_range(range, &common_prefix);
                }
                let preview: Vec<String> = candidates.into_iter().take(6).collect();
                let more = if preview.len() == 6 { "…" } else { "" };
                self.state.set_notification(format!(
                    "{} matches: {}{more}",
                    preview.len(),
                    preview.join(", ")
                ));
            }
        }
    }

    pub(crate) async fn execute_sql(&mut self) {
        let sql = self.sql_input.as_str().trim().to_string();
        if sql.is_empty() {
            return;
        }

        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => {
                self.state
                    .set_error("No database selected — pick one from the sidebar".to_string());
                return;
            }
        };

        self.state.query_loading = true;
        self.state.query_result = None;
        self.state.history_cursor = None;

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let start = Instant::now();
        let sql_clone = sql.clone();
        let context =
            self.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
                database: db.clone(),
                workspace: "main".to_string(),
            });

        self.task_registry.spawn("execute sql", async move {
            let outcome =
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql_clone)).await;
            match outcome {
                Ok(Ok(result)) => send_event(
                    &tx,
                    AppEvent::QueryResult {
                        context,
                        result,
                        duration: start.elapsed(),
                        sql: sql_clone,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::QueryError {
                        context,
                        sql: sql_clone,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::QueryError {
                        context,
                        sql: sql_clone,
                        error: "SQL query timed out".to_string(),
                    },
                ),
            }
        });
    }

    pub(crate) async fn refresh_current_view(&mut self) {
        match self.state.current_tab {
            Tab::Tables => {
                // If the previous schema fetch failed, `r` should
                // retry it instead of running a no-op table load
                // against a schema we don't have.
                if self.state.schema_load_failed || self.state.current_schema.is_none() {
                    self.load_schema().await;
                } else {
                    self.load_table_data(TableBrowseOrigin::ManualRefresh).await;
                }
            }
            Tab::Sql => {
                // Re-execute last SQL if any
                if let Some(entry) = self.state.sql_history.back() {
                    let sql = entry.sql.clone();
                    self.sql_input.set(sql);
                    self.execute_sql().await;
                }
            }
            Tab::Logs => {
                self.load_logs().await;
            }
            Tab::Metrics => {
                let client = self.client.clone();
                let tx = self.event_tx.clone();
                let context =
                    self.next_request_context(crate::effects::request::RequestScope::Metrics);
                self.task_registry
                    .spawn("refresh view metrics", async move {
                        if let Ok(Ok(text)) =
                            tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_metrics()).await
                        {
                            let snapshot = parse_prometheus_metrics(&text);
                            send_event(&tx, AppEvent::MetricsLoaded { context, snapshot });
                        }
                        let ok = matches!(
                            tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.ping()).await,
                            Ok(true)
                        );
                        send_event(&tx, AppEvent::PingResult(ok));
                    });
            }
            Tab::Module => {
                self.load_schema().await;
            }
            Tab::Live => {
                // Live subscriptions are disabled. Manual refresh
                // forces an immediate metadata poll of `st_client`.
                self.last_live_clients_fetch = None;
                self.maybe_refresh_live_clients();
            }
        }
    }

    pub(crate) async fn load_logs(&mut self) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.next_request_context(crate::effects::request::RequestScope::Logs {
            database: db.clone(),
        });
        self.task_registry.spawn("load logs", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_logs(&db, 500, false)).await
            {
                Ok(Ok(logs)) => send_event(&tx, AppEvent::LogsLoaded { context, logs }),
                Ok(Err(e)) => send_event(&tx, AppEvent::Error(format!("Logs fetch failed: {e:#}"))),
                Err(_) => send_event(&tx, AppEvent::Error("Logs fetch timed out".to_string())),
            }
        });
    }

    // ── WebSocket integration ─────────────────────────────────────────────

    /// Connect a WebSocket subscription for the currently selected database.
    ///
    /// Closes any existing WebSocket connection before opening a new one and
    /// clears any stale live-data cache from a previous database.
    pub(crate) async fn connect_ws(&mut self) {
        self.bump_server_context_generation();
        // Close existing connection if any
        if let Some(ref handle) = self.ws_handle {
            handle.close().await;
        }
        self.ws_handle = None;
        self.state.ws_connected = false;
        self.state.live_table_data.clear();
        self.live_subscribed = None;
        self.live_query_ids.clear();

        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };

        let config = WsConfig {
            base_url: self.ws_url.clone(),
            database: db,
            auth_token: self.auth_token.clone(),
            channel_capacity: 256,
        };

        match crate::api::ws::spawn_subscription(config) {
            Ok(handle) => {
                self.ws_handle = Some(handle);
                tracing::info!("WebSocket subscription task spawned");
            }
            Err(e) => {
                tracing::warn!("Failed to spawn WebSocket subscription: {e}");
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("WebSocket unavailable: {e}")),
                );
            }
        }
    }

    /// Drain all pending WebSocket events without blocking.
    pub(crate) async fn drain_ws_events(&mut self) {
        // Collect events first to avoid borrow issues
        let mut events: Vec<WsEvent> = Vec::new();
        if let Some(ref mut handle) = self.ws_handle {
            while let Ok(ev) = handle.event_rx.try_recv() {
                events.push(ev);
            }
        }
        for ev in events {
            self.handle_ws_event(ev).await;
        }
    }

    /// Abort all pending background tasks and drain every remaining report.
    /// Called once at shutdown so panics are logged and no task is silently
    /// detached.
    pub(crate) async fn shutdown_tasks(&mut self) {
        let pending = self.task_registry.pending_count();
        if pending > 0 {
            tracing::debug!("aborting {pending} pending background task(s) at shutdown");
        }
        self.task_registry.abort_all();
        while let Some(report) = self.task_registry.join_next().await {
            if let crate::effects::task_registry::TaskOutcome::Panicked { ref message } =
                report.outcome
            {
                tracing::error!(
                    "background task '{}' panicked during shutdown: {message}",
                    report.name
                );
            }
        }
        tracing::debug!("all background tasks joined at shutdown");
    }
}
