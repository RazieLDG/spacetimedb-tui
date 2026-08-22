/// Application orchestrator.
///
/// [`App`] owns both the [`AppState`] (all UI state) and the
/// [`SpacetimeClient`] (async HTTP API).  The main event loop:
///
/// 1. Draws the current frame via [`draw_frame`].
/// 2. Waits for either a crossterm keyboard/resize event **or** an async API
///    event arriving on the mpsc channel.
/// 3. Dispatches the event to the appropriate handler.
/// 4. Loops until `app_state.should_quit` is set.
pub mod command;
mod data_access;
mod dispatch;
pub mod event;
mod keys;
mod live_sync;
mod modals;
mod nav_actions;
pub mod navigation;
mod palette;
pub mod policy;
pub mod reducer;
pub mod request_context;
mod sources;
mod spreadsheet;
#[cfg(test)]
mod tests;
mod write_pipeline;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use guided_write::{
    build_postcondition_evidence, verification_report_from_postcondition_fetch_error,
    DeferredTableRefresh, GuidedWriteBlockKey, GuidedWriteLocks, GuidedWriteUnavailable,
    PendingGuidedUpdateConfirmationForm, PendingGuidedUpdateDraft, PendingSpreadsheetEditRequest,
    SchemaRequestContext, SpreadsheetSaveLifecycle, TableBrowseOrigin, TableBrowseRequestContext,
};

use ratatui::widgets::Widget;

use crate::{
    api::{
        types::TableInfo,
        ws::{WsConfig, WsEvent, WsHandle},
        SpacetimeClient,
    },
    config::Config,
    effects::write_ops::{
        build_delete_sql, build_guided_delete_plan, build_guided_update_plan, build_update_sql,
        complete_primary_key_from_table_row, encode_identifier, format_guided_delete_confirmation,
        format_guided_update_confirmation, guided_form_sql_value_from_user_input,
        guided_form_value_changed, guided_form_value_from_raw, normalize_lookup_result,
        plan_postcondition_queries, revalidate_before_dispatch, typed_sql_value_from_json,
        verify_authoritative_mutation_result, ConfirmationSnapshot, Generations, GuidedFormValue,
        LookupRows, MutationDispatchStage, PostconditionEvidence, PostconditionQueries,
        PostconditionUpdate, SqlEncodingError, TransportFailure, WritePlanBuildError,
        WriteVerificationReport, WriteVerificationRequest,
    },
    state::{
        app_state::BROWSE_PAGE_SIZE,
        edit_mode::{
            BeginEditResult, EditCellProjection, EditCellValues, EditRowTarget,
            SpreadsheetEditError,
        },
        safety::{
            ColumnChange, CompletePrimaryKey, GuidedMutation, MutationOutcome, PrimaryKeyError,
            QualifiedTable, WritePlan, WritePlanId,
        },
        AppState, ConnectionStatus, FocusPanel, HistoryAdvance, SidebarFocus, SqlHistoryEntry, Tab,
    },
    ui::components::input::InputState,
    ui::components::table_grid::TableGridState,
    ui::sidebar::{current_nav_index, nav_items, SidebarNavItem},
};

// ── Tick rate ─────────────────────────────────────────────────────────────────

/// How often we redraw even when there is no event.
const TICK_RATE: Duration = Duration::from_millis(200);

// ── Async API events ──────────────────────────────────────────────────────────

/// Events produced by background async tasks and delivered to the event loop.
#[derive(Debug)]
pub enum AppEvent {
    /// Databases list fetched.
    DatabasesLoaded {
        context: crate::effects::request::RequestContext,
        databases: Vec<String>,
    },
    /// Tables / schema fetched for the selected database.
    SchemaLoaded {
        context: SchemaRequestContext,
        schema: crate::api::types::SchemaResponse,
    },
    /// Schema fetch failed — carries a pre-formatted error message.
    /// Separate from the generic `Error` variant so the handler can
    /// clear `schema_loading` and flip `schema_load_failed` atomically.
    SchemaError {
        context: SchemaRequestContext,
        error: String,
    },
    /// SQL query result arrived (user-typed SQL in the SQL console tab).
    QueryResult {
        context: crate::effects::request::RequestContext,
        result: crate::api::types::QueryResult,
        duration: Duration,
        sql: String,
    },
    /// Table-browse result arrived (triggered by selecting a table from the
    /// sidebar). Kept separate from `QueryResult` so the Tables tab and the
    /// SQL tab do not share state.
    TableBrowseResult {
        context: TableBrowseRequestContext,
        result: crate::api::types::QueryResult,
        /// Total row count of the table from a parallel `COUNT(*)`,
        /// when that side query succeeded.
        total_rows: Option<u64>,
    },
    /// Table-browse load failed.
    TableBrowseError {
        context: TableBrowseRequestContext,
        error: String,
    },
    /// SQL query failed.
    QueryError {
        context: crate::effects::request::RequestContext,
        sql: String,
        error: String,
    },
    /// Log lines fetched.
    LogsLoaded {
        context: crate::effects::request::RequestContext,
        logs: Vec<crate::api::types::LogEntry>,
    },
    /// Metrics fetched.
    MetricsLoaded {
        context: crate::effects::request::RequestContext,
        snapshot: crate::state::MetricsSnapshot,
    },
    /// Live tab's periodic `st_client` poll returned.
    LiveClientsLoaded {
        context: crate::effects::request::RequestContext,
        clients: Vec<crate::state::app_state::LiveClientEntry>,
    },
    /// A reducer call (or write-SQL exec) finished successfully.
    /// `op` is a short human label like `call insert_user` or
    /// `delete row from users` so we can surface it in the status bar
    /// and the Live tab without re-deriving the description here.
    WriteOpSuccess {
        op: String,
        response: serde_json::Value,
    },
    /// A reducer call (or write-SQL exec) failed.
    WriteOpError { op: String, error: String },
    GuidedWriteVerification {
        plan_id: WritePlanId,
        op: String,
        report: WriteVerificationReport,
    },
    /// A live log line from WebSocket.
    LogLine(crate::api::types::LogEntry),
    /// Ping result.
    PingResult(bool),
    /// Generic notification.
    Notification(String),
    /// Generic error.
    Error(String),
}

pub mod guided_write;

#[cfg(test)]
fn database_nav_up_transition_requires_schema_reload(state: &mut AppState) -> bool {
    let old = state.selected_database_idx;
    state.database_prev();
    state.selected_database_idx != old
}

// ── App struct ────────────────────────────────────────────────────────────────

/// Top-level application struct.
pub struct App {
    pub state: AppState,
    pub client: SpacetimeClient,
    /// Sender half — cloned into background tasks.
    pub event_tx: mpsc::UnboundedSender<AppEvent>,
    /// Receiver half — consumed by the event loop.
    event_rx: mpsc::UnboundedReceiver<AppEvent>,
    /// Persistent user preferences from `~/.config/spacetimedb-tui/`.
    /// Read once at startup; the only thing we mutate at runtime is
    /// `SessionState`, which is written back on quit.
    user_config: crate::user_config::UserConfig,
    /// In-memory copy of the last-known session state, applied to the
    /// UI once the database list arrives in `bootstrap`.
    pending_session: Option<crate::user_config::SessionState>,
    /// SQL input state — single source of truth for the SQL editor buffer.
    pub sql_input: InputState,
    /// Table grid state for the tables tab.
    pub tables_grid: TableGridState,
    /// Table grid state for the SQL results.
    pub sql_grid: TableGridState,
    /// Active WebSocket subscription handle (set after database selection).
    ws_handle: Option<WsHandle>,
    /// WebSocket base URL (e.g. `ws://localhost:3000`).
    ws_url: String,
    /// Auth token for WebSocket connections.
    auth_token: Option<String>,
    /// Last time the metrics tab pulled fresh data — used to throttle the
    /// background refresh task to one fetch every `METRICS_REFRESH_INTERVAL`.
    last_metrics_fetch: Option<Instant>,
    /// Last time the Live tab polled `st_client` for the connected-client
    /// list. Throttled the same way metrics are.
    last_live_clients_fetch: Option<Instant>,
    /// `request_id` of the most recent scoped `Subscribe` message. Used to
    /// ignore snapshots that arrive after the user has switched tables.
    live_request_id: u32,
    /// Active `SubscribeSingle` query ids for the current table.
    live_query_ids: Vec<u32>,
    next_live_query_id: u32,
    /// Timestamp window (microseconds) used to bound Live snapshots.
    live_time_window_us: i64,
    /// Table + window currently subscribed, to avoid duplicate SubscribeSingle.
    live_subscribed: Option<(String, i64)>,
    next_write_plan_id: WritePlanId,
    write_plans: HashMap<WritePlanId, WritePlan>,
    pending_update_draft: Option<PendingGuidedUpdateDraft>,
    pending_update_confirmation_form: Option<PendingGuidedUpdateConfirmationForm>,
    pending_spreadsheet_edit_request: Option<PendingSpreadsheetEditRequest>,
    spreadsheet_save_lifecycle: Option<SpreadsheetSaveLifecycle>,
    deferred_guided_refresh: Option<DeferredTableRefresh>,
    guided_write_locks: GuidedWriteLocks,
    guided_write_blocks: HashMap<GuidedWriteBlockKey, u64>,
    guided_write_stale_data: HashSet<GuidedWriteBlockKey>,
    schema_generation: u64,
    table_generation: u64,
    server_context_generation: u64,
    database_generation: u64,
    /// Monotonically increasing request identifier for async read results.
    next_request_id: u64,
    /// The most recent `RequestContext` issued for each scope. A delivered
    /// result is applied only if it matches the latest context for its scope.
    latest_requests:
        HashMap<crate::effects::request::RequestScope, crate::effects::request::RequestContext>,
    /// Count of stale results that were silently dropped. Useful for debugging
    /// and observability.
    ignored_stale_results: u64,
    /// Owns all background tasks so their completion, panic, and cancellation
    /// are joined and reported instead of silently detached. Panics are
    /// surfaced into the Activity log by the event loop.
    task_registry: crate::effects::task_registry::TaskRegistry,
}

/// How often the Metrics tab automatically refreshes server-side metrics.
const METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
/// How often the Live tab re-polls `st_client` for connected clients.
const LIVE_CLIENTS_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// Maximum time we wait for any single HTTP-backed background request before
/// surfacing a timeout error to the user.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Send an [`AppEvent`] from a background task, logging a warning if the
/// receiver has been dropped (which only happens during shutdown).
fn send_event(tx: &mpsc::UnboundedSender<AppEvent>, event: AppEvent) {
    if tx.send(event).is_err() {
        tracing::warn!("AppEvent channel closed; dropping event");
    }
}

/// SQL used to browse a table. Identifiers are quoted so reserved names
/// and mixed-case tables round-trip.
/// Build the browse query for the page starting at `offset`.
///
/// SpacetimeDB SQL supports neither `OFFSET` nor `ORDER BY`, so paging
/// works by fetching the row *prefix* up to the end of the wanted page
/// (`LIMIT offset + page_size`) and slicing client-side. Page depth N
/// therefore transfers N×[`BROWSE_PAGE_SIZE`] rows — acceptable for an
/// admin browser, and every page turn doubles as a data refresh.
fn table_browse_sql(table: &str, offset: u64) -> Result<String, SqlEncodingError> {
    Ok(format!(
        "SELECT * FROM {} LIMIT {}",
        encode_identifier(table)?,
        offset + crate::state::app_state::BROWSE_PAGE_SIZE
    ))
}

/// Clamp a requested page `offset` against the number of rows actually
/// fetched and return `(start, end, effective_offset)` bounds for the
/// visible slice. `end` is exclusive; `effective_offset` may be lower
/// than `offset` when the table shrank between page turns.
pub(crate) fn browse_page_bounds(fetched_rows: usize, offset: u64) -> (usize, usize, u64) {
    let fetched = fetched_rows as u64;
    let start = offset.min(fetched);
    let end = fetched.min(start + crate::state::app_state::BROWSE_PAGE_SIZE);
    (start as usize, end as usize, start)
}

const LIVE_BROWSE_ROW_LIMIT: usize = 200;
const LIVE_TABLE_ROW_LIMIT: usize = 2_000;

/// Compare a cached live row with a delete payload.
///
/// Snapshots arrive as named JSON objects; incremental deletes often
/// arrive as positional arrays. Treat those as equal when the object
/// values match the array in order.
fn live_rows_match(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    if left == right {
        return true;
    }
    match (left, right) {
        (serde_json::Value::Object(object), serde_json::Value::Array(array))
        | (serde_json::Value::Array(array), serde_json::Value::Object(object)) => {
            object.values().eq(array.iter())
        }
        _ => false,
    }
}

impl App {
    /// Create a new [`App`] from config and a pre-built client.
    pub fn new(config: &Config, client: SpacetimeClient) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = AppState::new(config.server_url.clone());
        state.theme = config.theme.clone();
        Self {
            state,
            client,
            event_tx: tx,
            event_rx: rx,
            sql_input: InputState::new(),
            tables_grid: TableGridState::new(),
            sql_grid: TableGridState::new(),
            ws_handle: None,
            ws_url: config.ws_url.clone(),
            auth_token: config.auth_token.clone(),
            last_metrics_fetch: None,
            last_live_clients_fetch: None,
            live_request_id: 0,
            live_query_ids: Vec::new(),
            next_live_query_id: 0,
            live_time_window_us: crate::api::live_subscribe::DEFAULT_LIVE_WINDOW_US,
            live_subscribed: None,
            next_write_plan_id: WritePlanId(1),
            write_plans: HashMap::new(),
            pending_update_draft: None,
            pending_update_confirmation_form: None,
            pending_spreadsheet_edit_request: None,
            spreadsheet_save_lifecycle: None,
            deferred_guided_refresh: None,
            guided_write_locks: GuidedWriteLocks::default(),
            guided_write_blocks: HashMap::new(),
            guided_write_stale_data: HashSet::new(),
            schema_generation: 0,
            table_generation: 0,
            server_context_generation: 0,
            database_generation: 0,
            next_request_id: 0,
            latest_requests: HashMap::new(),
            ignored_stale_results: 0,
            task_registry: crate::effects::task_registry::TaskRegistry::new(),
            user_config: config.user_config.clone(),
            pending_session: if config.user_config.restore_session {
                Some(crate::user_config::SessionState::load())
            } else {
                None
            },
        }
    }

    // ── Bootstrap ─────────────────────────────────────────────────────────

    /// Perform the initial connection check and database listing.
    pub async fn bootstrap(&mut self) {
        self.state.connection.status = ConnectionStatus::Connecting;
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context =
            self.next_request_context(crate::effects::request::RequestScope::DatabaseCatalog);

        self.task_registry.spawn("bootstrap ping", async move {
            let ping_ok = matches!(
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.ping()).await,
                Ok(true)
            );
            send_event(&tx, AppEvent::PingResult(ping_ok));
            if !ping_ok {
                return;
            }
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.list_databases()).await {
                Ok(Ok(dbs)) => send_event(
                    &tx,
                    AppEvent::DatabasesLoaded {
                        context,
                        databases: dbs,
                    },
                ),
                Ok(Err(e)) => send_event(&tx, AppEvent::Error(format!("list_databases: {e:#}"))),
                Err(_) => send_event(
                    &tx,
                    AppEvent::Error("list_databases: request timed out".to_string()),
                ),
            }
        });
    }

    // ── Main event loop ───────────────────────────────────────────────────

    /// Run the application until the user quits.
    pub async fn run<B: ratatui::backend::Backend>(
        &mut self,
        terminal: &mut ratatui::Terminal<B>,
    ) -> Result<()> {
        self.bootstrap().await;

        loop {
            // Draw
            terminal
                .draw(|frame| {
                    draw_frame(
                        frame,
                        &mut self.state,
                        &self.sql_input,
                        &mut self.tables_grid,
                        &mut self.sql_grid,
                    )
                })
                .context("Terminal draw failed")?;

            // Poll for crossterm events (non-blocking, timeout = TICK_RATE)
            if ct_event::poll(TICK_RATE).context("ct_event::poll failed")? {
                match ct_event::read().context("ct_event::read failed")? {
                    Event::Key(key) => {
                        self.handle_key(key).await;
                    }
                    Event::Resize(_, _) => {
                        // Crossterm always redraws the next frame after a
                        // resize since we're in non-blocking poll mode, so
                        // we just consume the event and let the tick rate
                        // handle the redraw.
                    }
                    Event::Mouse(_) => {}
                    _ => {}
                }
            }

            // Drain async API events (non-blocking)
            while let Ok(ev) = self.event_rx.try_recv() {
                self.handle_app_event(ev).await;
            }

            // Drain WebSocket events (non-blocking)
            self.drain_ws_events().await;

            // Drain background-task reports (non-blocking). Panics are
            // surfaced into the Activity log instead of being silently lost.
            self.drain_task_reports();

            // Throttled background refresh of server metrics while the
            // Metrics tab is visible.
            self.maybe_refresh_metrics();

            // Throttled poll of st_client for the Live tab.
            self.maybe_refresh_live_clients();

            // Expire notifications
            self.state.tick_notifications(Duration::from_secs(5));

            if self.state.should_quit {
                break;
            }
        }

        // Abort all pending background tasks and drain their reports so
        // panics are logged before shutdown completes.
        self.shutdown_tasks().await;

        // Persist last-known UI state for the next launch.
        if self.user_config.restore_session {
            let snapshot = crate::user_config::SessionState {
                last_database: self.state.selected_database().map(str::to_string),
                last_table: self.state.selected_table().map(|t| t.table_name.clone()),
                last_tab: Some(tab_to_index(self.state.current_tab)),
            };
            snapshot.save();
        }

        Ok(())
    }

    /// If the Live tab is visible and we haven't polled `st_client`
    /// recently, spawn a background SQL query that fills
    /// `state.live_clients`.
    fn maybe_refresh_live_clients(&mut self) {
        if self.state.current_tab != Tab::Live {
            return;
        }
        let due = match self.last_live_clients_fetch {
            None => true,
            Some(t) => t.elapsed() >= LIVE_CLIENTS_REFRESH_INTERVAL,
        };
        if !due {
            return;
        }
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        self.last_live_clients_fetch = Some(Instant::now());

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context =
            self.next_request_context(crate::effects::request::RequestScope::LiveClients {
                database: db.clone(),
            });
        self.task_registry.spawn("live clients poll", async move {
            // `st_client` is a system table; we cap the result so a
            // huge production deployment doesn't hang the UI.
            let sql = "SELECT * FROM st_client LIMIT 200";
            let fetch = tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, sql));
            let Ok(Ok(result)) = fetch.await else {
                // Silent — `st_client` may not be exposed on some
                // deployments; we don't want to spam the error popup.
                return;
            };
            let clients: Vec<crate::state::app_state::LiveClientEntry> = result
                .rows
                .iter()
                .map(|row| {
                    // Best-effort: pick the first string-ish cell as
                    // the identity. We don't have a reliable schema
                    // for st_client across server versions.
                    let identity = row
                        .iter()
                        .find_map(|v| match v {
                            serde_json::Value::String(s) => Some(s.clone()),
                            serde_json::Value::Number(n) => Some(n.to_string()),
                            _ => None,
                        })
                        .unwrap_or_else(|| "(unknown)".to_string());
                    crate::state::app_state::LiveClientEntry {
                        identity,
                        connected_at: None,
                    }
                })
                .collect();
            send_event(&tx, AppEvent::LiveClientsLoaded { context, clients });
        });
    }

    /// If the user is on the Metrics tab and we haven't fetched metrics
    /// recently, spawn a background fetch. Throttled by
    /// [`METRICS_REFRESH_INTERVAL`] to keep network traffic minimal.
    fn maybe_refresh_metrics(&mut self) {
        if self.state.current_tab != Tab::Metrics {
            return;
        }
        let due = match self.last_metrics_fetch {
            None => true,
            Some(t) => t.elapsed() >= METRICS_REFRESH_INTERVAL,
        };
        if !due {
            return;
        }
        self.last_metrics_fetch = Some(Instant::now());

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.next_request_context(crate::effects::request::RequestScope::Metrics);
        self.task_registry.spawn("metrics refresh", async move {
            if let Ok(Ok(text)) =
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_metrics()).await
            {
                let snapshot = parse_prometheus_metrics(&text);
                send_event(&tx, AppEvent::MetricsLoaded { context, snapshot });
            }
        });
    }
}

// ── Session restore helpers (Faz 6) ──────────────────────────────────────────

/// Encode a [`Tab`] as the same `0..6` index used in
/// `Tab::ALL`. Used when persisting / restoring `SessionState`.
fn tab_to_index(tab: Tab) -> u8 {
    Tab::ALL.iter().position(|t| *t == tab).unwrap_or(0) as u8
}

/// Inverse of [`tab_to_index`].
fn index_to_tab(idx: u8) -> Tab {
    Tab::ALL.get(idx as usize).copied().unwrap_or(Tab::Tables)
}

// ── Modal helpers (Faz 5) ────────────────────────────────────────────────────

/// Extract the SpacetimeDB type "tag" from an algebraic-type JSON
/// value. The schema encodes types as either `"String"` or
/// `{"String": []}`-style objects, so we tolerate both.
fn type_tag(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) if o.len() == 1 => {
            let Some((name, value)) = o.iter().next() else {
                return "Unknown".to_string();
            };
            if name == "AlgebraicType" {
                type_tag(value)
            } else {
                name.clone()
            }
        }
        _ => "Unknown".to_string(),
    }
}

fn build_guided_update_changes_from_form_fields(
    table: &TableInfo,
    original_raw_row: &[serde_json::Value],
    original_form_values: &[GuidedFormValue],
    fields: &[crate::state::modal::FormField],
) -> Result<Vec<ColumnChange>, String> {
    let mut changes = Vec::new();
    for (column_index, column) in table.columns.iter().enumerate() {
        let field = fields
            .get(column_index)
            .ok_or_else(|| format!("Missing form field for column {}", column.col_name))?;
        let original_raw = original_raw_row
            .get(column_index)
            .ok_or_else(|| format!("Missing original value for column {}", column.col_name))?;
        let original_form = original_form_values
            .get(column_index)
            .ok_or_else(|| format!("Missing original form value for column {}", column.col_name))?;
        if matches!(
            original_form.typed_value,
            crate::state::safety::SqlValue::Unrepresentable(_)
        ) {
            if field.input.value == original_form.input {
                continue;
            }
            return Err(format!(
                "Cannot edit {}: original value is unsupported or unrepresentable for guided updates",
                column.col_name
            ));
        }
        if !guided_form_value_changed(column, original_form, &field.input.value).map_err(
            |error| {
                format!(
                    "Cannot parse {} as {}: {error:?}",
                    column.col_name,
                    type_tag(&column.col_type)
                )
            },
        )? {
            continue;
        }

        let old_value = typed_sql_value_from_json(column, original_raw).map_err(|error| {
            format!(
                "Cannot read original value for {} as {}: {error:?}",
                column.col_name,
                type_tag(&column.col_type)
            )
        })?;
        let new_value = guided_form_sql_value_from_user_input(column, &field.input.value)
            .map_err(|error| format!("Cannot parse {}: {error:?}", column.col_name))?;
        changes.push(ColumnChange {
            column_id: column.col_id,
            old_value: Some(old_value),
            new_value,
        });
    }
    Ok(changes)
}

fn format_mutation_outcome_not_sent(outcome: MutationOutcome) -> String {
    match outcome {
        MutationOutcome::CriticalSafetyError { reason } => {
            format!("Critical guided-write safety error: {reason}")
        }
        MutationOutcome::DefinitelyNotSent { reason } => {
            format!("Write plan was not sent: {reason}")
        }
        MutationOutcome::Conflict { reason } => format!("Write conflict before send: {reason}"),
        MutationOutcome::Unknown { reason } => {
            format!("Write status unknown before send: {reason}")
        }
        MutationOutcome::SentAndConfirmed { .. } => "Write plan was already sent".to_string(),
    }
}

/// Suggest a placeholder string for a form field based on its type
/// tag. Just guidance — the user can type whatever they want.
fn default_placeholder_for_type(t: &str) -> String {
    match t {
        "String" => "text".to_string(),
        "Bool" => "true / false".to_string(),
        s if s.starts_with('U') || s.starts_with('I') => "0".to_string(),
        s if s.starts_with('F') => "0.0".to_string(),
        _ => "".to_string(),
    }
}

/// Extract the bare column / parameter name from a form field label
/// like `"name (String)"` → `"name"`.
fn extract_field_name(label: &str) -> String {
    label.split_whitespace().next().unwrap_or("").to_string()
}

/// Coerce a raw input string into a JSON value suitable for the
/// SpacetimeDB reducer-call wire format. We try to be helpful but
/// not magical: numerics parse, booleans parse, everything else stays
/// a string. JSON-shaped input (`[1,2]`, `{"k":"v"}`) is preserved
/// verbatim by attempting a `serde_json::from_str` first.
fn coerce_field_to_json(raw: &str, type_tag: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return serde_json::Value::Null;
    }
    // Numeric / bool inference based on the declared type tag.
    if type_tag.starts_with('U') || type_tag.starts_with('I') {
        if let Ok(n) = trimmed.parse::<i64>() {
            return serde_json::json!(n);
        }
    }
    if type_tag.starts_with('F') {
        if let Ok(f) = trimmed.parse::<f64>() {
            return serde_json::json!(f);
        }
    }
    if type_tag == "Bool" {
        if let Ok(b) = trimmed.parse::<bool>() {
            return serde_json::json!(b);
        }
    }
    // If the input *looks* like JSON, accept it as-is so users can
    // pass arrays / objects to complex param types.
    if matches!(trimmed.chars().next(), Some('[' | '{' | '"')) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return v;
        }
    }
    serde_json::Value::String(trimmed.to_string())
}

/// Build a SQL literal from a raw input string and a type tag.
/// Numerics are emitted bare, booleans become `TRUE`/`FALSE`, and
/// everything else is single-quoted with embedded quotes doubled.
///
/// `Identity` / `ConnectionId` / `Address` columns get a hex-literal
/// path: if the user typed `"0xabc…"` or a bare `"abc…"` the emitted
/// literal is `0xabc…` (valid SpacetimeDB hex syntax) rather than a
/// single-quoted string — which the server would reject.
fn sql_literal(raw: &str, type_tag: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "NULL".to_string();
    }
    // Identity / ConnectionId / Address want a hex literal, not a
    // quoted string. Accept both "0xabc…" and bare "abc…" and always
    // emit the `0x`-prefixed form. Checked *before* the numeric
    // prefix branch below because `"Identity"` starts with `I` and
    // would otherwise get mis-classified as an integer type.
    if type_tag == "Identity" || type_tag == "ConnectionId" || type_tag.contains("Address") {
        let body = trimmed.strip_prefix("0x").unwrap_or(trimmed);
        if !body.is_empty() && body.chars().all(|c| c.is_ascii_hexdigit()) {
            return format!("0x{body}");
        }
        // Fall through to quoted-string form if the input doesn't
        // look like hex — surfaces the problem as a SQL error
        // rather than sending a silently-wrong literal.
        let escaped = trimmed.replace('\'', "''");
        return format!("'{escaped}'");
    }
    if type_tag.starts_with('U') || type_tag.starts_with('I') || type_tag.starts_with('F') {
        return trimmed.to_string();
    }
    if type_tag == "Bool" {
        return match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => "TRUE".to_string(),
            _ => "FALSE".to_string(),
        };
    }
    let escaped = trimmed.replace('\'', "''");
    format!("'{escaped}'")
}

// ── Metrics Parser ────────────────────────────────────────────────────────────

fn parse_prometheus_metrics(text: &str) -> crate::state::MetricsSnapshot {
    let mut snapshot = crate::state::MetricsSnapshot {
        sampled_at: Some(chrono::Utc::now()),
        ..Default::default()
    };

    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let key = parts[0];
        let val: f64 = parts[1].parse().unwrap_or(0.0);

        if key.contains("connected_clients") {
            snapshot.connected_clients = val as u64;
        } else if key.contains("reducer_calls_total") {
            snapshot.total_reducer_calls = val as u64;
        } else if key.contains("energy_used_total") {
            snapshot.total_energy_used = val as u64;
        } else if key.contains("memory_bytes") {
            snapshot.memory_bytes = val as u64;
        } else {
            snapshot
                .extra
                .insert(key.to_string(), serde_json::json!(val));
        }
    }
    snapshot
}

// ── Frame renderer ────────────────────────────────────────────────────────────

/// Draw the complete UI frame.
pub fn draw_frame(
    frame: &mut ratatui::Frame,
    state: &mut AppState,
    sql_input: &InputState,
    tables_grid: &mut TableGridState,
    sql_grid: &mut TableGridState,
) {
    use crate::ui::{
        components::{
            help::HelpOverlay, modal::render_modal, palette::render_palette, status_bar::StatusBar,
        },
        layout::render_layout,
        sidebar::render_sidebar,
        tabs::{
            live::render_live, logs::render_logs, metrics::render_metrics, module::render_module,
            sql::render_sql, tables::render_tables,
        },
    };
    use ratatui::layout::{Constraint, Direction, Layout};

    let area = frame.area();

    // ── Outer layout: content + status bar ───────────────────────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let main_area = outer[0];
    let status_area = outer[1];

    // ── Render chrome (title bar, tab bar, sidebar border) ────────────────
    let content_areas = render_layout(main_area, frame.buffer_mut(), state);

    // ── Sidebar ───────────────────────────────────────────────────────────
    render_sidebar(content_areas.sidebar, frame.buffer_mut(), state);

    // ── Tab content ───────────────────────────────────────────────────────
    match state.current_tab {
        crate::state::Tab::Tables => {
            render_tables(
                content_areas.content,
                frame.buffer_mut(),
                state,
                tables_grid,
            );
        }
        crate::state::Tab::Sql => {
            render_sql(
                content_areas.content,
                frame.buffer_mut(),
                state,
                sql_input,
                sql_grid,
            );
        }
        crate::state::Tab::Logs => {
            render_logs(content_areas.content, frame.buffer_mut(), state);
        }
        crate::state::Tab::Metrics => {
            render_metrics(content_areas.content, frame.buffer_mut(), state);
        }
        crate::state::Tab::Module => {
            let selected = state.module_selected_reducer;
            render_module(content_areas.content, frame.buffer_mut(), state, selected);
        }
        crate::state::Tab::Live => {
            render_live(content_areas.content, frame.buffer_mut(), state);
        }
    }

    // ── Status bar ────────────────────────────────────────────────────────
    StatusBar::new(state).render(status_area, frame.buffer_mut());

    // ── Help overlay (drawn on top of everything) ─────────────────────────
    if state.show_help {
        HelpOverlay::new(state.help_scroll).render(area, frame.buffer_mut());
    }

    // ── Modal dialog (drawn last so it's always on top) ──────────────────
    if let Some(ref modal) = state.modal {
        render_modal(area, frame.buffer_mut(), modal);
    }

    // ── Command palette (always on top, even above modals) ──────────────
    if let Some(ref palette) = state.palette {
        render_palette(area, frame.buffer_mut(), palette);
    }
}
