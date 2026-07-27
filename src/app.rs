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
pub mod event;
pub mod navigation;
pub mod policy;
pub mod reducer;

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

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
        complete_primary_key_from_table_row, format_guided_delete_confirmation,
        format_guided_update_confirmation, guided_form_sql_value_from_user_input,
        guided_form_value_changed, guided_form_value_from_raw, normalize_lookup_result,
        plan_postcondition_queries, revalidate_before_dispatch, typed_sql_value_from_json,
        verify_authoritative_mutation_result, ConfirmationSnapshot, Generations, GuidedFormValue,
        LookupRows, MutationDispatchStage, PostconditionEvidence, PostconditionQueries,
        PostconditionUpdate, TransportFailure, WritePlanBuildError, WriteVerificationReport,
        WriteVerificationRequest,
    },
    state::{
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaRequestContext {
    database: String,
    schema_generation: u64,
    database_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TableBrowseOrigin {
    ManualRefresh,
    Navigation,
    Automatic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableBrowseRequestContext {
    database: String,
    schema: Option<String>,
    table: String,
    origin: TableBrowseOrigin,
    server_context_generation: u64,
    table_generation: u64,
    schema_generation: u64,
    database_generation: u64,
}

#[derive(Clone, Debug)]
struct PendingGuidedUpdateDraft {
    plan_id: WritePlanId,
    qualified_table: QualifiedTable,
    table_info: TableInfo,
    original_raw_row: Vec<serde_json::Value>,
    generations: Generations,
    original_form_values: Vec<GuidedFormValue>,
    requested_row: usize,
    requested_column: u32,
}

#[derive(Clone, Debug)]
struct PendingGuidedUpdateConfirmationForm {
    draft: PendingGuidedUpdateDraft,
    title: String,
    fields: Vec<crate::state::modal::FormField>,
    focus: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GuidedWriteTarget {
    table: QualifiedTable,
    primary_key: CompletePrimaryKey,
    table_generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeferredTableRefresh {
    table: QualifiedTable,
    server_context_generation: u64,
    database_generation: u64,
    schema_generation: u64,
    table_generation: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct GuidedWriteBlockKey {
    database: String,
    schema: Option<String>,
    table: String,
}

/// Why guided writes are currently refused for a table.
///
/// The two reasons are deliberately different products of the safety
/// contract. `RequiresManualRefresh` is the strong block demanded by
/// `Unknown`, `Conflict`, and `CriticalSafetyError`: the user must
/// personally reload before we accept another write. `StaleAfterVerifiedWrite`
/// is the weak barrier a *successful* write raises when it invalidated a
/// same-row owner: the cached rows are simply out of date, so any accepted
/// reload (including the automatic one the success itself queues) clears it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GuidedWriteUnavailable {
    RequiresManualRefresh,
    StaleAfterVerifiedWrite,
}

impl GuidedWriteBlockKey {
    fn from_table(table: &QualifiedTable) -> Self {
        Self {
            database: table.database.clone(),
            schema: table.schema.clone(),
            table: table.table.clone(),
        }
    }

    fn matches_browse_context(&self, context: &TableBrowseRequestContext) -> bool {
        self.database == context.database
            && self.schema == context.schema
            && self.table == context.table
    }
}

impl GuidedWriteTarget {
    fn from_plan(plan: &WritePlan) -> Self {
        Self {
            table: plan.table.clone(),
            primary_key: plan.original_primary_key.clone(),
            table_generation: plan.row_generation,
        }
    }

    fn same_row_identity(&self, other: &Self) -> bool {
        self.table == other.table && self.primary_key == other.primary_key
    }
}

#[derive(Debug, Default)]
struct GuidedWriteLocks {
    targets_by_plan_id: HashMap<WritePlanId, GuidedWriteTarget>,
}

#[derive(Clone, Debug)]
struct PendingSpreadsheetEditRequest {
    target: EditRowTarget,
    column_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpreadsheetSaveLifecycle {
    AwaitingConfirmation(WritePlanId),
    InFlight(WritePlanId),
    InFlightInvalidated(WritePlanId),
}

impl SpreadsheetSaveLifecycle {
    fn plan_id(self) -> WritePlanId {
        match self {
            Self::AwaitingConfirmation(plan_id)
            | Self::InFlight(plan_id)
            | Self::InFlightInvalidated(plan_id) => plan_id,
        }
    }

    fn is_in_flight(self) -> bool {
        matches!(self, Self::InFlight(_) | Self::InFlightInvalidated(_))
    }

    fn is_invalidated(self) -> bool {
        matches!(self, Self::InFlightInvalidated(_))
    }

    fn invalidate_if_in_flight(self) -> Self {
        match self {
            Self::InFlight(plan_id) | Self::InFlightInvalidated(plan_id) => {
                Self::InFlightInvalidated(plan_id)
            }
            Self::AwaitingConfirmation(plan_id) => Self::AwaitingConfirmation(plan_id),
        }
    }
}

impl GuidedWriteLocks {
    fn acquire(&mut self, plan: &WritePlan) -> bool {
        let target = GuidedWriteTarget::from_plan(plan);
        if self
            .targets_by_plan_id
            .values()
            .any(|locked| locked.same_row_identity(&target))
        {
            return false;
        }
        self.targets_by_plan_id.insert(plan.id, target);
        true
    }

    fn is_locked(&self, plan: &WritePlan) -> bool {
        let target = GuidedWriteTarget::from_plan(plan);
        self.targets_by_plan_id
            .values()
            .any(|locked| locked.same_row_identity(&target))
    }

    fn has_lock_for_table(&self, table: &QualifiedTable) -> bool {
        self.targets_by_plan_id
            .values()
            .any(|locked| locked.table == *table)
    }

    fn release(&mut self, plan_id: WritePlanId) -> Option<GuidedWriteTarget> {
        self.targets_by_plan_id.remove(&plan_id)
    }
}

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

    /// Raise the strong "user must reload before we write again" block.
    ///
    /// The epoch is deliberately the generation observed *at the moment the
    /// outcome is applied*, not the generation the plan captured. A refresh
    /// that was already in flight when the outcome landed cannot prove
    /// anything about the post-outcome state, so it must not unblock.
    fn block_guided_writes_for_target(&mut self, target: &GuidedWriteTarget) {
        let key = GuidedWriteBlockKey::from_table(&target.table);
        let generation = self.table_generation;
        self.guided_write_blocks
            .entry(key)
            .and_modify(|blocked_generation| {
                *blocked_generation = (*blocked_generation).max(generation);
            })
            .or_insert(generation);
        self.deferred_guided_refresh = self
            .deferred_guided_refresh
            .take()
            .filter(|refresh| refresh.table != target.table);
    }

    /// Raise the weak "cached rows are behind a verified write" barrier.
    ///
    /// Unlike [`Self::block_guided_writes_for_target`] this is not part of the
    /// manual-refresh contract: it only records that the rows currently on
    /// screen predate a confirmed mutation. It is retired when those rows are
    /// replaced or dropped, and while it is up the app keeps driving the
    /// reload that replaces them, so the user is never left stranded.
    fn mark_guided_write_data_stale(&mut self, table: &QualifiedTable) {
        self.guided_write_stale_data
            .insert(GuidedWriteBlockKey::from_table(table));
    }

    fn guided_write_data_is_stale(&self, table: &QualifiedTable) -> bool {
        self.guided_write_stale_data
            .contains(&GuidedWriteBlockKey::from_table(table))
    }

    fn guided_write_blocked_generation(&self, table: &QualifiedTable) -> Option<u64> {
        self.guided_write_blocks
            .get(&GuidedWriteBlockKey::from_table(table))
            .copied()
    }

    fn guided_write_unavailable_reason(
        &self,
        table: &QualifiedTable,
    ) -> Option<GuidedWriteUnavailable> {
        if self.guided_write_blocked_generation(table).is_some() {
            return Some(GuidedWriteUnavailable::RequiresManualRefresh);
        }
        self.guided_write_data_is_stale(table)
            .then_some(GuidedWriteUnavailable::StaleAfterVerifiedWrite)
    }

    fn set_guided_write_unavailable_error(
        &mut self,
        table: &QualifiedTable,
        reason: GuidedWriteUnavailable,
    ) {
        let message = match reason {
            GuidedWriteUnavailable::RequiresManualRefresh => format!(
                "Guided writes for {} are blocked until manual refresh loads newer table data",
                table.table
            ),
            GuidedWriteUnavailable::StaleAfterVerifiedWrite => format!(
                "Guided writes for {} are paused until the reload after the last verified write lands",
                table.table
            ),
        };
        self.state.set_error(message);
    }

    /// Refuse a guided write when the target is blocked or stale, explaining
    /// which of the two it is. Returns `true` when the caller must stop.
    fn refuse_guided_write_if_unavailable(&mut self, table: &QualifiedTable) -> bool {
        match self.guided_write_unavailable_reason(table) {
            Some(reason) => {
                self.set_guided_write_unavailable_error(table, reason);
                true
            }
            None => false,
        }
    }

    fn unblock_guided_writes_after_accepted_browse(&mut self, context: &TableBrowseRequestContext) {
        // An accepted load replaces the rows the barrier was raised for.
        self.guided_write_stale_data
            .retain(|key| !key.matches_browse_context(context));
        if context.origin != TableBrowseOrigin::ManualRefresh {
            return;
        }
        self.guided_write_blocks.retain(|key, blocked_generation| {
            !(key.matches_browse_context(context) && context.table_generation > *blocked_generation)
        });
    }

    fn next_write_plan_id(&mut self) -> WritePlanId {
        let id = self.next_write_plan_id;
        self.next_write_plan_id = WritePlanId(self.next_write_plan_id.0.saturating_add(1));
        id
    }

    fn current_generations(&self) -> Generations {
        Generations {
            schema: self.schema_generation,
            row: self.table_generation,
            server_context: self.server_context_generation,
            database: self.database_generation,
        }
    }

    fn discard_pending_guided_writes(&mut self) {
        self.clear_spreadsheet_owned_modal();
        self.clear_discarded_guided_owned_modal();
        self.write_plans.clear();
        self.pending_update_draft = None;
        self.pending_update_confirmation_form = None;
        self.pending_spreadsheet_edit_request = None;
        if let Some(lifecycle) = self.spreadsheet_save_lifecycle {
            self.spreadsheet_save_lifecycle = if lifecycle.is_in_flight() {
                Some(lifecycle.invalidate_if_in_flight())
            } else {
                None
            };
        }
    }

    fn clear_discarded_guided_owned_modal(&mut self) {
        let should_clear = self
            .state
            .modal
            .as_ref()
            .is_some_and(|modal| match modal.action() {
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::DirtyRowChoice {
                        requested_row,
                        requested_column,
                    },
                ) => self.pending_update_draft.as_ref().is_some_and(|draft| {
                    draft.requested_row == *requested_row
                        && draft.requested_column == *requested_column
                }),
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                ) => {
                    self.write_plans.contains_key(plan_id)
                        || self
                            .pending_update_confirmation_form
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.draft.plan_id == *plan_id)
                }
                _ => false,
            });
        if should_clear {
            self.state.modal = None;
        }
    }

    fn clear_spreadsheet_edit_state(&mut self) {
        self.clear_spreadsheet_owned_modal();
        self.state.edit_mode = None;
        self.pending_spreadsheet_edit_request = None;
        self.spreadsheet_save_lifecycle = None;
    }

    fn clear_spreadsheet_owned_modal(&mut self) {
        let should_clear = self
            .state
            .modal
            .as_ref()
            .is_some_and(|modal| match modal.action() {
                crate::state::modal::ModalAction::SpreadsheetDirtyRowChoice { .. }
                | crate::state::modal::ModalAction::DiscardPendingEdits => true,
                crate::state::modal::ModalAction::Safety(
                    crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                ) => self.spreadsheet_lifecycle_plan_id() == Some(*plan_id),
                _ => false,
            });
        if should_clear {
            self.state.modal = None;
        }
    }

    fn spreadsheet_lifecycle_plan_id(&self) -> Option<WritePlanId> {
        self.spreadsheet_save_lifecycle
            .map(SpreadsheetSaveLifecycle::plan_id)
    }

    fn spreadsheet_edits_are_frozen(&self) -> bool {
        self.spreadsheet_save_lifecycle
            .is_some_and(SpreadsheetSaveLifecycle::is_in_flight)
    }

    fn selected_table_matches(&self, table: &QualifiedTable) -> bool {
        self.state.selected_database() == Some(table.database.as_str())
            && self
                .state
                .selected_table()
                .is_some_and(|selected| selected.table_name == table.table)
    }

    fn target_for_guided_update_draft(
        draft: &PendingGuidedUpdateDraft,
    ) -> Option<GuidedWriteTarget> {
        let primary_key =
            complete_primary_key_from_table_row(&draft.table_info, &draft.original_raw_row).ok()?;
        Some(GuidedWriteTarget {
            table: draft.qualified_table.clone(),
            primary_key,
            table_generation: draft.generations.row,
        })
    }

    fn discard_same_target_pending_guided_update(&mut self, target: &GuidedWriteTarget) -> bool {
        let discard_draft = self
            .pending_update_draft
            .as_ref()
            .and_then(Self::target_for_guided_update_draft)
            .is_some_and(|draft_target| draft_target.same_row_identity(target));
        if discard_draft {
            self.pending_update_draft = None;
        }

        let discard_confirmation_snapshot = self
            .pending_update_confirmation_form
            .as_ref()
            .and_then(|snapshot| Self::target_for_guided_update_draft(&snapshot.draft))
            .is_some_and(|snapshot_target| snapshot_target.same_row_identity(target));
        if discard_confirmation_snapshot {
            self.pending_update_confirmation_form = None;
        }

        let removed_plan_ids: Vec<WritePlanId> = self
            .write_plans
            .iter()
            .filter_map(|(plan_id, plan)| {
                let plan_target = GuidedWriteTarget::from_plan(plan);
                plan_target.same_row_identity(target).then_some(*plan_id)
            })
            .collect();
        for plan_id in &removed_plan_ids {
            self.write_plans.remove(plan_id);
        }

        let should_clear_modal =
            self.state
                .modal
                .as_ref()
                .is_some_and(|modal| match modal.action() {
                    crate::state::modal::ModalAction::Safety(
                        crate::state::modal::SafetyModalAction::DirtyRowChoice { .. },
                    ) => discard_draft,
                    crate::state::modal::ModalAction::Safety(
                        crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
                    ) => removed_plan_ids.contains(plan_id),
                    _ => false,
                });
        if should_clear_modal {
            self.state.modal = None;
        }
        // A spreadsheet save that was still awaiting confirmation loses its
        // plan and modal here; leaving the marker behind would freeze every
        // later spreadsheet interaction on a plan that no longer exists.
        if self.spreadsheet_save_lifecycle.is_some_and(|lifecycle| {
            !lifecycle.is_in_flight() && removed_plan_ids.contains(&lifecycle.plan_id())
        }) {
            self.spreadsheet_save_lifecycle = None;
        }
        discard_draft || discard_confirmation_snapshot || !removed_plan_ids.is_empty()
    }

    fn restore_guided_update_plan_after_local_rejection(&mut self, plan: WritePlan) {
        if self
            .pending_update_confirmation_form
            .as_ref()
            .is_some_and(|snapshot| snapshot.draft.plan_id == plan.id)
        {
            let plan_id = plan.id;
            self.write_plans.insert(plan_id, plan);
            self.restore_pending_update_confirmation_form(plan_id);
        }
    }

    fn guided_current_table_interaction_is_active(&self) -> bool {
        if let (Some(database), Some(table)) = (
            self.state.selected_database(),
            self.state
                .selected_table()
                .map(|table| table.table_name.as_str()),
        ) {
            let selected_table = QualifiedTable {
                database: database.to_string(),
                schema: None,
                table: table.to_string(),
            };
            if self.guided_write_locks.has_lock_for_table(&selected_table) {
                return true;
            }
        }
        if self
            .pending_update_draft
            .as_ref()
            .is_some_and(|draft| self.selected_table_matches(&draft.qualified_table))
        {
            return true;
        }
        if self.pending_spreadsheet_edit_request.is_some()
            || self.spreadsheet_save_lifecycle.is_some()
            || self.state.edit_mode.is_some()
        {
            return true;
        }
        if let Some(crate::state::modal::ModalAction::Safety(
            crate::state::modal::SafetyModalAction::ConfirmWritePlan { plan_id },
        )) = self
            .state
            .modal
            .as_ref()
            .map(crate::state::modal::Modal::action)
        {
            return self
                .write_plans
                .get(plan_id)
                .is_some_and(|plan| self.selected_table_matches(&plan.table));
        }
        false
    }

    fn read_owner_is_active(&self) -> bool {
        self.state.query_loading || self.state.schema_loading
    }

    fn deferred_table_refresh_for(&self, table: QualifiedTable) -> DeferredTableRefresh {
        DeferredTableRefresh {
            table,
            server_context_generation: self.server_context_generation,
            database_generation: self.database_generation,
            schema_generation: self.schema_generation,
            table_generation: self.table_generation,
        }
    }

    fn deferred_refresh_matches_current(&self, refresh: &DeferredTableRefresh) -> bool {
        self.selected_table_matches(&refresh.table)
            && self.server_context_generation == refresh.server_context_generation
            && self.database_generation == refresh.database_generation
            && self.schema_generation == refresh.schema_generation
            && self.table_generation == refresh.table_generation
    }

    async fn refresh_or_defer_guided_table(&mut self, table: QualifiedTable) {
        // Only the manual-refresh block suppresses an automatic reload; the
        // weak staleness barrier exists precisely so this reload can clear it.
        if self.guided_write_blocked_generation(&table).is_some() {
            self.deferred_guided_refresh = self
                .deferred_guided_refresh
                .take()
                .filter(|refresh| refresh.table != table);
            return;
        }
        if self.guided_current_table_interaction_is_active() || self.read_owner_is_active() {
            self.deferred_guided_refresh = Some(self.deferred_table_refresh_for(table));
            return;
        }
        if self.selected_table_matches(&table) {
            self.deferred_guided_refresh = None;
            self.load_table_data(TableBrowseOrigin::Automatic).await;
        }
    }

    /// The table whose rows are behind a verified write and which we are still
    /// responsible for reloading.
    ///
    /// The weak staleness barrier refuses guided writes, so something must
    /// always be driving the load that clears it. A queued deferred refresh can
    /// be dropped by an unrelated invalidation, so the barrier on the selected
    /// table is itself a standing reload request and outlives that queue entry.
    fn selected_table_awaits_stale_reload(&self) -> bool {
        let Some(database) = self.state.selected_database() else {
            return false;
        };
        let Some(table) = self.state.selected_table() else {
            return false;
        };
        let selected = QualifiedTable {
            database: database.to_string(),
            schema: None,
            table: table.table_name.clone(),
        };
        self.guided_write_data_is_stale(&selected)
            && self.guided_write_blocked_generation(&selected).is_none()
    }

    async fn flush_deferred_guided_refresh_if_unowned(&mut self) {
        if self
            .deferred_guided_refresh
            .as_ref()
            .is_some_and(|refresh| !self.deferred_refresh_matches_current(refresh))
        {
            self.deferred_guided_refresh = None;
        }
        if self.guided_current_table_interaction_is_active() || self.read_owner_is_active() {
            return;
        }
        let refresh = self.deferred_guided_refresh.take();
        if let Some(refresh) = refresh {
            if self
                .guided_write_blocked_generation(&refresh.table)
                .is_some()
            {
                return;
            }
            if self.deferred_refresh_matches_current(&refresh) {
                self.load_table_data(TableBrowseOrigin::Automatic).await;
                return;
            }
        }
        // The queued reload was dropped or never matched, but the barrier is
        // still refusing writes on the table in front of the user, so drive the
        // reload that retires it.
        if self.selected_table_awaits_stale_reload() {
            self.load_table_data(TableBrowseOrigin::Automatic).await;
        }
    }

    fn block_if_spreadsheet_edits_frozen(&mut self, action: &str) -> bool {
        if self.spreadsheet_edits_are_frozen() {
            self.state.set_notification(format!(
                "Spreadsheet save is in flight; {action} after it finishes"
            ));
            true
        } else {
            false
        }
    }

    fn clear_table_browse_for_selection_change(&mut self) {
        self.state.table_browse_result = None;
        self.state.query_loading = false;
        self.tables_grid = TableGridState::new();
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    fn current_schema_request_context(&self, database: String) -> SchemaRequestContext {
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
    fn next_request_context(
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
    fn should_apply_result(
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
    /// system is tracked as Phase 2 tech debt.
    fn apply_result_if_current(
        &mut self,
        delivered: &crate::effects::request::RequestContext,
    ) -> bool {
        let should_apply = Self::should_apply_result(&self.latest_requests, delivered);
        if !should_apply {
            self.ignored_stale_results = self.ignored_stale_results.saturating_add(1);
        }
        should_apply
    }

    fn current_table_browse_request_context(
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

    fn schema_context_matches_current(&self, context: &SchemaRequestContext) -> bool {
        self.state.selected_database() == Some(context.database.as_str())
            && self.schema_generation == context.schema_generation
            && self.database_generation == context.database_generation
    }

    fn table_context_matches_current(&self, context: &TableBrowseRequestContext) -> bool {
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

    fn bump_schema_generation(&mut self) {
        self.schema_generation = self.schema_generation.saturating_add(1);
        self.discard_pending_guided_writes();
    }

    fn bump_table_generation(&mut self) {
        self.table_generation = self.table_generation.saturating_add(1);
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    fn bump_server_context_generation(&mut self) {
        self.server_context_generation = self.server_context_generation.saturating_add(1);
        self.discard_pending_guided_writes();
        if !self.spreadsheet_edits_are_frozen() {
            self.clear_spreadsheet_edit_state();
        }
    }

    fn bump_database_generation(&mut self) {
        self.database_generation = self.database_generation.saturating_add(1);
        // A database switch invalidates every block and staleness barrier:
        // they are keyed by (database, schema, table) and the old entries
        // would otherwise accumulate without bound across switches.
        self.guided_write_blocks.clear();
        self.guided_write_stale_data.clear();
        self.bump_schema_generation();
        self.bump_table_generation();
    }

    fn selected_database_for_modal(&mut self) -> Option<String> {
        match self.state.selected_database() {
            Some(database) => Some(database.to_string()),
            None => {
                self.state.set_error("No database selected".to_string());
                None
            }
        }
    }

    fn qualified_selected_table(&mut self, table: &TableInfo) -> Option<QualifiedTable> {
        let database = self.selected_database_for_modal()?;
        Some(QualifiedTable {
            database,
            schema: None,
            table: table.table_name.clone(),
        })
    }

    fn explain_primary_key_error(table: &TableInfo, error: PrimaryKeyError) -> String {
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

    fn explain_write_plan_build_error(error: WritePlanBuildError) -> String {
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

    // ── Key dispatch ──────────────────────────────────────────────────────

    /// Dispatch a keyboard event to the appropriate handler.
    ///
    /// Uses explicit `return` statements to make early-exit control flow clear.
    #[allow(clippy::needless_return)]
    async fn handle_key(&mut self, key: KeyEvent) {
        // ── Command palette intercept ─────────────────────────────────────
        // The palette owns every key while it's open. Ctrl+C still quits.
        if self.state.palette.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.state.should_quit = true;
                return;
            }
            self.handle_palette_key(key).await;
            return;
        }

        // ── Modal dialog intercept ────────────────────────────────────────
        // When a confirm prompt or form is open, the modal owns every
        // key. Ctrl+C still quits as a panic-button escape hatch.
        if self.state.modal.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.state.should_quit = true;
                return;
            }
            self.handle_modal_key(key).await;
            return;
        }

        // ── Spreadsheet edit-mode intercept ───────────────────────────────
        // When edit mode is active on the Tables tab, the whole key map
        // changes (cell cursor / inline editor / save / revert). We still
        // honour Ctrl+C as an escape hatch and Ctrl+E to toggle off.
        if self.state.edit_mode.is_some()
            && self.state.focus == FocusPanel::Main
            && self.state.current_tab == Tab::Tables
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('c') => {
                        self.state.should_quit = true;
                        return;
                    }
                    KeyCode::Char('e') => {
                        self.exit_edit_mode().await;
                        return;
                    }
                    _ => {}
                }
            }
            self.handle_edit_mode_key(key).await;
            return;
        }

        // ── Global always-active bindings ─────────────────────────────────
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.state.should_quit = true;
                    return;
                }
                KeyCode::Char('r') => {
                    // Force a fresh WebSocket connection (e.g. after a server bounce).
                    self.connect_ws().await;
                    self.state
                        .set_notification("Reconnecting WebSocket…".to_string());
                    return;
                }
                KeyCode::Char('e')
                    if self.state.focus == FocusPanel::Main
                        && self.state.current_tab == Tab::Tables =>
                {
                    // Ctrl+E on the Tables tab enters spreadsheet edit
                    // mode. Only fires when we're *not* already in it —
                    // the intercept above catches the toggle-off case.
                    self.enter_edit_mode();
                    return;
                }
                KeyCode::Char('p') => {
                    // Open the command palette.
                    self.state.palette = Some(crate::state::palette::CommandPalette::new());
                    return;
                }
                KeyCode::Char('a') | KeyCode::Home if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.home();
                    return;
                }
                KeyCode::Char('e') | KeyCode::End if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.end();
                    return;
                }
                KeyCode::Char('k') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.kill_to_end();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('u') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.kill_to_start();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('l') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.clear();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('f')
                    if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                        && self.state.focus == FocusPanel::Main =>
                {
                    // Ctrl+F opens the grid search prompt.
                    self.state.grid_search = Some(String::new());
                    self.state.grid_search_editing = true;
                    return;
                }
                KeyCode::Char('w') if self.state.focus == FocusPanel::SqlInput => {
                    // Delete the previous word (Ctrl+W, classic Unix convention).
                    let before = &self.sql_input.value[..self.sql_input.cursor];
                    let trimmed_end = before.trim_end_matches(|c: char| c.is_whitespace());
                    let word_start = trimmed_end
                        .rfind(|c: char| c.is_whitespace() || !(c.is_alphanumeric() || c == '_'))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let range = word_start..self.sql_input.cursor;
                    if !range.is_empty() {
                        self.sql_input.replace_range(range, "");
                        self.state.history_cursor = None;
                    }
                    return;
                }
                _ => {}
            }
        }

        // ── Help overlay ──────────────────────────────────────────────────
        if self.state.show_help {
            // Clamp the scroll offset to the actual number of lines so a
            // user mashing `↓` doesn't push the value into the millions
            // (and then have to bash `↑` for ages to recover).
            let max_scroll =
                crate::ui::components::help::HelpOverlay::total_lines().saturating_sub(1);
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.state.show_help = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.state.help_scroll =
                        self.state.help_scroll.saturating_add(1).min(max_scroll);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(1);
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    self.state.help_scroll = 0;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    self.state.help_scroll = max_scroll;
                }
                _ => {}
            }
            return;
        }

        // ── Error popup — only Esc / Enter dismiss it so accidental keys
        // don't silently swallow the message before the user has read it.
        if self.state.error_message.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                self.state.clear_error();
            }
            return;
        }

        // ── Grid search prompt mode ───────────────────────────────────────
        // When Ctrl+F is active on a data-grid tab, we intercept every key
        // for the search buffer instead of running the regular bindings.
        if self.state.grid_search_editing {
            match key.code {
                KeyCode::Esc => {
                    // Cancel search entirely — clears the highlight.
                    self.state.grid_search = None;
                    self.state.grid_search_editing = false;
                }
                KeyCode::Enter => {
                    // Commit the query; highlights stay and n/N navigate.
                    self.state.grid_search_editing = false;
                    self.jump_to_next_match(true);
                }
                KeyCode::Backspace => {
                    if let Some(q) = self.state.grid_search.as_mut() {
                        q.pop();
                    }
                }
                KeyCode::Char(ch) => {
                    if let Some(q) = self.state.grid_search.as_mut() {
                        q.push(ch);
                    }
                }
                _ => {}
            }
            return;
        }

        // ── SQL input mode ────────────────────────────────────────────────
        if self.state.focus == FocusPanel::SqlInput {
            match key.code {
                KeyCode::Esc => {
                    self.state.focus = FocusPanel::Main;
                }
                KeyCode::Enter => {
                    self.execute_sql().await;
                }
                KeyCode::Tab => {
                    self.complete_sql_input();
                }
                KeyCode::Up => {
                    if self.state.history_prev() {
                        if let Some(sql) = self.state.current_history_sql() {
                            self.sql_input.set(sql.to_string());
                        }
                    }
                }
                KeyCode::Down => match self.state.history_next() {
                    HistoryAdvance::Moved => {
                        if let Some(sql) = self.state.current_history_sql() {
                            self.sql_input.set(sql.to_string());
                        }
                    }
                    HistoryAdvance::Cleared => {
                        self.sql_input.clear();
                    }
                    HistoryAdvance::Unchanged => {}
                },
                KeyCode::Left => self.sql_input.move_left(),
                KeyCode::Right => self.sql_input.move_right(),
                KeyCode::Home => self.sql_input.home(),
                KeyCode::End => self.sql_input.end(),
                KeyCode::Backspace => {
                    self.sql_input.backspace();
                    self.state.history_cursor = None;
                }
                KeyCode::Delete => {
                    self.sql_input.delete();
                    self.state.history_cursor = None;
                }
                KeyCode::Char(ch) => {
                    self.sql_input.insert(ch);
                    // Any edit drops the user out of "browsing history"
                    // mode so ↓ no longer snaps back to the old entry.
                    self.state.history_cursor = None;
                }
                _ => {}
            }
            return;
        }

        // ── Global bindings (not in SQL input mode) ───────────────────────
        match key.code {
            // Quit
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.state.should_quit = true;
                return;
            }

            // Help overlay
            KeyCode::Char('?') => {
                self.state.show_help = !self.state.show_help;
                self.state.help_scroll = 0;
                return;
            }

            // Tab cycling
            KeyCode::Tab => {
                self.state.current_tab = self.state.current_tab.next();
                self.state.focus = FocusPanel::Main;
                return;
            }
            KeyCode::BackTab => {
                self.state.current_tab = self.state.current_tab.prev();
                self.state.focus = FocusPanel::Main;
                return;
            }

            // Direct tab jump
            KeyCode::Char('1') => {
                self.state.current_tab = Tab::Tables;
                return;
            }
            KeyCode::Char('2') => {
                self.state.current_tab = Tab::Sql;
                return;
            }
            KeyCode::Char('3') => {
                self.state.current_tab = Tab::Logs;
                return;
            }
            KeyCode::Char('4') => {
                self.state.current_tab = Tab::Metrics;
                return;
            }
            KeyCode::Char('5') => {
                self.state.current_tab = Tab::Module;
                return;
            }
            KeyCode::Char('6') => {
                self.state.current_tab = Tab::Live;
                return;
            }

            // Sidebar focus: h/← steps up Tables → Databases; l/→ moves
            // focus over into the main pane.
            KeyCode::Left | KeyCode::Char('h') if self.state.focus == FocusPanel::Sidebar => {
                if self.state.sidebar_focus == SidebarFocus::Tables {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                }
                return;
            }
            KeyCode::Right | KeyCode::Char('l') if self.state.focus == FocusPanel::Sidebar => {
                self.state.focus = FocusPanel::Main;
                return;
            }

            // Main focus: h/← and l/→ move the cell cursor inside a data
            // grid (Tables or SQL tabs). Use Esc to drop back to sidebar.
            KeyCode::Left | KeyCode::Char('h')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.prev_col();
                return;
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let col_count = if self.state.current_tab == Tab::Tables {
                    self.state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0)
                } else {
                    self.state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0)
                };
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.next_col(col_count);
                return;
            }

            // Enter SQL mode
            KeyCode::Char(':') => {
                self.state.current_tab = Tab::Sql;
                self.state.focus = FocusPanel::SqlInput;
                return;
            }

            // Search / filter (sidebar)
            KeyCode::Char('/') => {
                // Toggle search mode — simple: enter a char into search_query
                if self.state.search_query.is_empty() {
                    self.state.focus = FocusPanel::Sidebar;
                } else {
                    self.state.search_query.clear();
                }
                return;
            }

            // Refresh current view
            KeyCode::Char('r') => {
                self.refresh_current_view().await;
                return;
            }

            // Navigation — delegate to focus owner
            KeyCode::Char('j') | KeyCode::Down => {
                self.nav_down().await;
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.nav_up().await;
                return;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.nav_home();
                return;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.nav_end();
                return;
            }

            // Enter / select
            KeyCode::Enter => {
                self.nav_enter().await;
                return;
            }

            // Escape — multi-level "go back":
            //   1. clear an active search-as-you-type query, else
            //   2. if sidebar focus is on Tables, step back up to Databases, else
            //   3. snap keyboard focus from the main pane back to the sidebar.
            KeyCode::Esc => {
                if !self.state.search_query.is_empty() {
                    self.state.search_query.clear();
                } else if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Tables
                {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                } else {
                    self.state.focus = FocusPanel::Sidebar;
                }
                return;
            }

            // Clipboard — `y` yanks the currently selected cell, `Y`
            // yanks the whole row (TSV-joined). Works on the data-grid
            // tabs (Tables / SQL) when focus is in the main pane.
            KeyCode::Char('y')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.copy_selected_cell();
                return;
            }
            KeyCode::Char('Y')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.copy_selected_row();
                return;
            }

            // Insert row (Tables tab only). Opens a form prefilled
            // with the current schema's columns. Submit issues an
            // INSERT INTO ... VALUES (...) SQL statement.
            KeyCode::Char('i')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_insert_form();
                return;
            }

            // Delete row (Tables tab only). Opens a y/n confirm dialog
            // that issues a DELETE FROM ... WHERE pk = ... statement.
            // PK is heuristically the first column of the table.
            KeyCode::Char('d')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_delete_confirm();
                return;
            }

            // Update row (Tables tab only). Opens an edit form
            // prefilled with the current row's values. The first
            // field is the PK display; submit issues an UPDATE.
            KeyCode::Char('U')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_update_form();
                return;
            }

            // ── Destructive admin ops (typed-confirm forms) ───────
            // Shift+D on the Tables tab → truncate the selected
            // table (DELETE FROM <table>). Same key on the sidebar
            // when focused on the Databases panel → delete the
            // entire database via DELETE /v1/database/<name>.
            KeyCode::Char('D')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_truncate_table_form();
                return;
            }
            KeyCode::Char('D')
                if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Databases =>
            {
                self.open_delete_db_form();
                return;
            }

            // `a` on the Databases sidebar panel opens a form to
            // attach a new alias (human name) to the selected DB.
            // Non-destructive, no typed-confirm required.
            KeyCode::Char('a')
                if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Databases =>
            {
                self.open_add_alias_form();
                return;
            }

            // Sort — `s` cycles the sort state (off → asc → desc → off)
            // on the currently-selected column.
            KeyCode::Char('s')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let col_count = self
                    .active_grid()
                    .map(|(qr, _)| qr.column_count())
                    .unwrap_or(0);
                if col_count == 0 {
                    return;
                }
                // Snapshot the underlying data row the cursor points at
                // *before* mutating the sort state — we'll translate it
                // back into a display row after the permutation changes
                // so the cursor appears to stay on the "same" record
                // instead of jumping to a random position.
                let anchor_data_row = self.active_data_row_index();
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.cycle_sort(grid.selected_col);

                // Re-map the anchor through the new permutation. If
                // anything goes sideways (empty rows, missing anchor)
                // the cursor stays where it was — no worse than the
                // old behaviour.
                if let Some(data_idx) = anchor_data_row {
                    if let Some(new_display) = self.display_row_for_data_idx(data_idx) {
                        let grid = if self.state.current_tab == Tab::Tables {
                            &mut self.tables_grid
                        } else {
                            &mut self.sql_grid
                        };
                        grid.selected_row = new_display;
                    }
                }
                let col_name = self
                    .active_grid()
                    .and_then(|(qr, g)| {
                        qr.column_names().get(g.selected_col).map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                let dir = match (
                    self.active_grid().map(|(_, g)| g.sort_col),
                    self.active_grid().map(|(_, g)| g.sort_desc),
                ) {
                    (Some(Some(_)), Some(false)) => "asc",
                    (Some(Some(_)), Some(true)) => "desc",
                    _ => "off",
                };
                self.state
                    .set_notification(format!("Sort {col_name} {dir}"));
                return;
            }

            // Export — `e` writes a CSV, `E` writes a JSON file under
            // `./exports/` for the currently visible query result.
            KeyCode::Char('e')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.export_current_result(crate::ui::export::ExportFormat::Csv);
                return;
            }
            KeyCode::Char('E')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.export_current_result(crate::ui::export::ExportFormat::Json);
                return;
            }

            // Log-specific
            KeyCode::Char(' ') if self.state.current_tab == Tab::Logs => {
                self.state.log_follow = !self.state.log_follow;
                return;
            }
            KeyCode::Char('c') if self.state.current_tab == Tab::Logs => {
                self.state.log_buffer.clear();
                self.state.log_scroll = 0;
                return;
            }
            KeyCode::Char('f') if self.state.current_tab == Tab::Logs => {
                self.state.log_filter_level = self.state.log_filter_level.clone().next_filter();
                self.state
                    .set_notification(format!("Log filter: {}", self.state.log_filter_level));
                return;
            }

            // `n` / `N` on data-grid tabs: jump to next / previous
            // search match when a search query is active, otherwise
            // fall through to page-scroll on the Tables tab.
            KeyCode::Char('n')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                    && self.state.grid_search.is_some() =>
            {
                self.jump_to_next_match(true);
                return;
            }
            KeyCode::Char('N')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                    && self.state.grid_search.is_some() =>
            {
                self.jump_to_next_match(false);
                return;
            }

            // Page navigation in tables (no active search)
            KeyCode::Char('n') if self.state.current_tab == Tab::Tables => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_add(20);
                return;
            }
            KeyCode::Char('p') if self.state.current_tab == Tab::Tables => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_sub(20);
                return;
            }

            // Horizontal scroll in table/SQL results (< / > or H / L)
            KeyCode::Char('<') | KeyCode::Char('H')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.scroll_left();
                return;
            }
            KeyCode::Char('>') | KeyCode::Char('L')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let (col_count, grid) = if self.state.current_tab == Tab::Tables {
                    let cc = self
                        .state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0);
                    (cc, &mut self.tables_grid)
                } else {
                    let cc = self
                        .state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0);
                    (cc, &mut self.sql_grid)
                };
                grid.scroll_right(col_count);
                return;
            }

            // Search input (when in sidebar search mode) — also acts as
            // "step up" when there's no search text and the user is on the
            // Tables sub-panel.
            KeyCode::Backspace if self.state.focus == FocusPanel::Sidebar => {
                if !self.state.search_query.is_empty() {
                    self.state.search_query.pop();
                } else if self.state.sidebar_focus == SidebarFocus::Tables {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                }
                return;
            }
            KeyCode::Char(ch)
                if self.state.focus == FocusPanel::Sidebar && !ch.is_ascii_control() =>
            {
                // In sidebar, typing filters the list
                self.state.search_query.push(ch);
                return;
            }

            _ => {}
        }
    }

    // ── Navigation helpers ────────────────────────────────────────────────

    async fn nav_down(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => match self.state.sidebar_focus {
                SidebarFocus::Databases => {
                    let old = self.state.selected_database_idx;
                    self.state.database_next();
                    if self.state.selected_database_idx != old {
                        self.bump_database_generation();
                        self.load_schema().await;
                    }
                }
                SidebarFocus::Tables => {
                    let old = self.state.selected_table_idx;
                    self.state.table_next();
                    if self.state.selected_table_idx != old {
                        self.bump_table_generation();
                        self.clear_table_browse_for_selection_change();
                    }
                }
            },
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    let row_count = self
                        .state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.row_count())
                        .unwrap_or(0);
                    self.tables_grid.next_row(row_count);
                }
                Tab::Sql => {
                    let row_count = self
                        .state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.row_count())
                        .unwrap_or(0);
                    self.sql_grid.next_row(row_count);
                }
                Tab::Logs => {
                    if !self.state.log_follow {
                        self.state.log_scroll = self
                            .state
                            .log_scroll
                            .saturating_add(1)
                            .min(self.state.log_buffer.len().saturating_sub(1));
                    }
                }
                Tab::Module => {
                    let count = self
                        .state
                        .current_schema
                        .as_ref()
                        .map(|s| s.reducers.len())
                        .unwrap_or(0);
                    if count > 0 {
                        self.state.module_selected_reducer =
                            (self.state.module_selected_reducer + 1).min(count - 1);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    async fn nav_up(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => match self.state.sidebar_focus {
                SidebarFocus::Databases => {
                    if database_nav_up_transition_requires_schema_reload(&mut self.state) {
                        self.bump_database_generation();
                        self.load_schema().await;
                    }
                }
                SidebarFocus::Tables => {
                    let old = self.state.selected_table_idx;
                    self.state.table_prev();
                    if self.state.selected_table_idx != old {
                        self.bump_table_generation();
                        self.clear_table_browse_for_selection_change();
                    }
                }
            },
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    self.tables_grid.prev_row();
                }
                Tab::Sql => {
                    self.sql_grid.prev_row();
                }
                Tab::Logs => {
                    if !self.state.log_follow {
                        self.state.log_scroll = self.state.log_scroll.saturating_sub(1);
                    }
                }
                Tab::Module => {
                    self.state.module_selected_reducer =
                        self.state.module_selected_reducer.saturating_sub(1);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn nav_home(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                if let SidebarFocus::Tables = self.state.sidebar_focus {
                    let old = self.state.selected_table_idx;
                    self.state.selected_table_idx = if self.state.tables.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
                    if self.state.selected_table_idx != old {
                        self.bump_table_generation();
                        self.clear_table_browse_for_selection_change();
                    }
                }
            }
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    self.tables_grid.selected_row = 0;
                    self.tables_grid.scroll_row = 0;
                }
                Tab::Sql => {
                    self.sql_grid.selected_row = 0;
                    self.sql_grid.scroll_row = 0;
                }
                Tab::Logs => {
                    self.state.log_scroll = 0;
                    self.state.log_follow = false;
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn nav_end(&mut self) {
        if self.state.focus == FocusPanel::Main {
            match self.state.current_tab {
                Tab::Tables => {
                    if let Some(ref qr) = self.state.table_browse_result {
                        let count = qr.row_count();
                        self.tables_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Sql => {
                    if let Some(ref qr) = self.state.query_result {
                        let count = qr.row_count();
                        self.sql_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Logs => {
                    self.state.log_follow = true;
                }
                _ => {}
            }
        }
    }

    async fn nav_enter(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                match self.state.sidebar_focus {
                    SidebarFocus::Databases => {
                        // Move focus to tables
                        self.state.sidebar_focus = SidebarFocus::Tables;
                        if !self.state.tables.is_empty() && self.state.selected_table_idx.is_none()
                        {
                            self.state.selected_table_idx = Some(0);
                        }
                    }
                    SidebarFocus::Tables => {
                        // Load the selected table's data
                        self.load_table_data(TableBrowseOrigin::Navigation).await;
                        self.state.focus = FocusPanel::Main;
                        self.state.current_tab = Tab::Tables;
                        self.tables_grid = TableGridState::new();
                    }
                }
            }
            FocusPanel::Main => {
                if self.state.current_tab == Tab::Sql {
                    self.state.focus = FocusPanel::SqlInput;
                } else if self.state.current_tab == Tab::Module {
                    // Enter on a reducer in the module inspector opens
                    // a call form (or a no-arg confirm, when the
                    // reducer has no parameters).
                    self.open_reducer_form();
                }
            }
            _ => {}
        }
    }

    // ── Data loading ──────────────────────────────────────────────────────

    async fn load_schema(&mut self) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        self.bump_schema_generation();
        self.state.tables.clear();
        self.state.selected_table_idx = None;
        self.state.current_schema = None;
        self.clear_table_browse_for_selection_change();
        // Track the in-flight schema fetch so the sidebar can show
        // a real loading spinner and clear it on both success and
        // failure (fixes the "stuck on (loading…)" bug after HTTP
        // 500s).
        self.state.schema_loading = true;
        self.state.schema_load_failed = false;

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.current_schema_request_context(db.clone());
        self.task_registry.spawn("load schema", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_schema(&db)).await {
                Ok(Ok(schema)) => send_event(
                    &tx,
                    AppEvent::SchemaLoaded {
                        context: context.clone(),
                        schema,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::SchemaError {
                        context: context.clone(),
                        error: format!("Schema load failed: {e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::SchemaError {
                        context: context.clone(),
                        error: "Schema load timed out".to_string(),
                    },
                ),
            }
        });
    }

    async fn load_table_data(&mut self, origin: TableBrowseOrigin) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        let table = match self.state.selected_table() {
            Some(t) => t.table_name.clone(),
            None => return,
        };

        self.bump_table_generation();
        self.state.query_loading = true;
        self.state.table_browse_result = None;

        let sql = format!("SELECT * FROM {table} LIMIT 200");
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.current_table_browse_request_context(db.clone(), table.clone(), origin);

        self.task_registry.spawn("browse table", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql)).await {
                Ok(Ok(result)) => send_event(
                    &tx,
                    AppEvent::TableBrowseResult {
                        context: context.clone(),
                        result,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        context: context.clone(),
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        context: context.clone(),
                        error: "table load timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Return a reference to the `QueryResult` / `TableGridState` pair
    /// that backs the currently focused data-grid tab, together with
    /// the table-name hint (if any) used for notifications.
    fn active_grid(&self) -> Option<(&crate::api::types::QueryResult, &TableGridState)> {
        match self.state.current_tab {
            Tab::Tables => self
                .state
                .table_browse_result
                .as_ref()
                .map(|qr| (qr, &self.tables_grid)),
            Tab::Sql => self
                .state
                .query_result
                .as_ref()
                .map(|qr| (qr, &self.sql_grid)),
            _ => None,
        }
    }

    /// Translate a grid's `selected_row` (which is in display order
    /// when a sort is active) back to the underlying `QueryResult.rows`
    /// index, so clipboard / export operations read the cells the user
    /// is actually looking at.
    fn active_data_row_index(&self) -> Option<usize> {
        let (qr, grid) = self.active_grid()?;
        // Re-project the rows into the same `Vec<Vec<String>>` that the
        // renderer sorts, then ask `sorted_data_index` for the mapping.
        let string_rows: Vec<Vec<String>> = qr
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(crate::ui::tabs::tables::value_to_display)
                    .collect()
            })
            .collect();
        crate::ui::components::table_grid::sorted_data_index(
            &string_rows,
            grid.sort_col,
            grid.sort_desc,
            grid.selected_row,
        )
    }

    /// Reverse of [`active_data_row_index`]: given an underlying
    /// `data_idx` (stable across sort permutations), return the
    /// display row index it currently lives at under the active
    /// grid's sort state. Used by the `s` key binding to keep the
    /// cell cursor anchored to the same record when the permutation
    /// changes.
    fn display_row_for_data_idx(&self, data_idx: usize) -> Option<usize> {
        let (qr, grid) = self.active_grid()?;
        if qr.rows.is_empty() || data_idx >= qr.rows.len() {
            return None;
        }
        let Some(sort_col) = grid.sort_col else {
            return Some(data_idx);
        };
        let string_rows: Vec<Vec<String>> = qr
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(crate::ui::tabs::tables::value_to_display)
                    .collect()
            })
            .collect();
        // Rebuild the permutation the renderer uses and scan for
        // the display index whose mapped data index matches.
        // O(n²) in the worst case but data grids cap at a few
        // hundred rows — acceptable for a one-shot key event.
        (0..string_rows.len()).find(|&display_idx| {
            crate::ui::components::table_grid::sorted_data_index(
                &string_rows,
                Some(sort_col),
                grid.sort_desc,
                display_idx,
            ) == Some(data_idx)
        })
    }

    /// Copy the currently-highlighted cell to the terminal clipboard.
    fn copy_selected_cell(&mut self) {
        let cell_text = {
            let data_idx = match self.active_data_row_index() {
                Some(i) => i,
                None => return,
            };
            let Some((qr, grid)) = self.active_grid() else {
                return;
            };
            let row = match qr.rows.get(data_idx) {
                Some(r) => r,
                None => return,
            };
            let value = match row.get(grid.selected_col) {
                Some(v) => v,
                None => return,
            };
            crate::ui::tabs::tables::value_to_display(value)
        };

        match crate::ui::clipboard::copy_to_clipboard(&cell_text) {
            Ok(n) => {
                let preview: String = cell_text.chars().take(40).collect();
                self.state
                    .set_notification(format!("Copied {n}B: {preview}"));
            }
            Err(e) => {
                tracing::warn!("clipboard copy failed: {e}");
                self.state.set_error(format!("Clipboard copy failed: {e}"));
            }
        }
    }

    /// Copy the currently-selected row to the terminal clipboard as a
    /// TSV (tab-separated values) line.
    fn copy_selected_row(&mut self) {
        let (row_text, col_count) = {
            let data_idx = match self.active_data_row_index() {
                Some(i) => i,
                None => return,
            };
            let Some((qr, _grid)) = self.active_grid() else {
                return;
            };
            let row = match qr.rows.get(data_idx) {
                Some(r) => r,
                None => return,
            };
            let tsv = row
                .iter()
                .map(crate::ui::tabs::tables::value_to_display)
                .collect::<Vec<_>>()
                .join("\t");
            (tsv, row.len())
        };

        match crate::ui::clipboard::copy_to_clipboard(&row_text) {
            Ok(n) => {
                self.state
                    .set_notification(format!("Copied row ({col_count} cells, {n}B)"));
            }
            Err(e) => {
                tracing::warn!("clipboard copy failed: {e}");
                self.state.set_error(format!("Clipboard copy failed: {e}"));
            }
        }
    }

    /// Move the cell cursor to the next (or previous, if `forward` is
    /// `false`) row that contains a match for the current grid search
    /// query. Wraps around the end of the result set.
    ///
    /// A "match" is any cell whose string representation contains the
    /// query as a case-insensitive substring. Used by Enter on the
    /// search prompt and by `n` / `N` afterwards.
    fn jump_to_next_match(&mut self, forward: bool) {
        let query = match self.state.grid_search.as_ref() {
            Some(q) if !q.is_empty() => q.to_ascii_lowercase(),
            _ => return,
        };

        // Snapshot the rows we're searching so we can release the
        // immutable borrow on `state` before mutating the grid.
        let rows: Vec<Vec<String>> = {
            let qr = match self.state.current_tab {
                Tab::Tables => self.state.table_browse_result.as_ref(),
                Tab::Sql => self.state.query_result.as_ref(),
                _ => return,
            };
            let Some(qr) = qr else {
                return;
            };
            qr.rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(crate::ui::tabs::tables::value_to_display)
                        .collect()
                })
                .collect()
        };

        if rows.is_empty() {
            self.state.set_notification("No rows to search".to_string());
            return;
        }

        let grid = if self.state.current_tab == Tab::Tables {
            &mut self.tables_grid
        } else {
            &mut self.sql_grid
        };

        // Walk display order (which is `rows` when unsorted, or the
        // sort permutation when a sort is active) so `n` / `N`
        // visually steps by one row on screen each time.
        let order: Vec<usize> = match grid.sort_col {
            Some(col) => {
                let mut idxs: Vec<usize> = (0..rows.len()).collect();
                idxs.sort_by(|&a, &b| {
                    let av = rows[a].get(col).map(String::as_str).unwrap_or("");
                    let bv = rows[b].get(col).map(String::as_str).unwrap_or("");
                    // Replicate `compare_cells` locally so we don't have
                    // to expose it outside `table_grid`.
                    match (av.parse::<f64>(), bv.parse::<f64>()) {
                        (Ok(na), Ok(nb)) => {
                            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
                        }
                        _ => av.to_ascii_lowercase().cmp(&bv.to_ascii_lowercase()),
                    }
                });
                if grid.sort_desc {
                    idxs.reverse();
                }
                idxs
            }
            None => (0..rows.len()).collect(),
        };

        let n = order.len();
        let start = grid.selected_row.min(n - 1);
        for step in 1..=n {
            let display_idx = if forward {
                (start + step) % n
            } else {
                (start + n - step) % n
            };
            let data_idx = order[display_idx];
            if rows[data_idx]
                .iter()
                .any(|cell| cell.to_ascii_lowercase().contains(&query))
            {
                grid.selected_row = display_idx;
                return;
            }
        }
        self.state
            .set_notification(format!("No match for \"{query}\""));
    }

    /// Serialise the currently visible query result to CSV or JSON and
    /// write it under `./exports/`. Shows the resulting path in the
    /// status bar notification so the user can `cat` / open it.
    fn export_current_result(&mut self, format: crate::ui::export::ExportFormat) {
        let (qr, label) = match self.state.current_tab {
            Tab::Tables => {
                let qr = match self.state.table_browse_result.as_ref() {
                    Some(qr) => qr,
                    None => {
                        self.state.set_notification("Nothing to export".to_string());
                        return;
                    }
                };
                let label = self
                    .state
                    .selected_table()
                    .map(|t| t.table_name.clone())
                    .unwrap_or_else(|| "table".to_string());
                (qr.clone(), label)
            }
            Tab::Sql => {
                let qr = match self.state.query_result.as_ref() {
                    Some(qr) => qr,
                    None => {
                        self.state.set_notification("Nothing to export".to_string());
                        return;
                    }
                };
                (qr.clone(), "query".to_string())
            }
            _ => return,
        };

        match crate::ui::export::write_export(&qr, format, &label) {
            Ok(path) => {
                self.state
                    .set_notification(format!("Exported to {}", path.display()));
            }
            Err(e) => {
                tracing::warn!("export failed: {e:#}");
                self.state.set_error(format!("Export failed: {e:#}"));
            }
        }
    }

    // ── Spreadsheet edit mode (Faz 10) ───────────────────────────────────

    /// Open spreadsheet edit mode on the Tables tab. No-op if the
    /// user hasn't loaded any table data yet (nothing to edit).
    fn enter_edit_mode(&mut self) {
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

    fn active_edit_row_target(&self) -> Result<EditRowTarget, String> {
        let data_row = self
            .active_data_row_index()
            .ok_or_else(|| "No row selected".to_string())?;
        self.edit_row_target_at(data_row)
    }

    fn edit_row_target_at(&self, data_row: usize) -> Result<EditRowTarget, String> {
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
    async fn exit_edit_mode(&mut self) {
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
    async fn handle_edit_mode_key(&mut self, key: KeyEvent) {
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
    fn handle_cell_editor_key(&mut self, key: KeyEvent) {
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
    fn begin_cell_edit(&mut self) {
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
    fn commit_cell_edit(&mut self) {
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
    fn revert_active_cell(&mut self) {
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
    async fn save_pending_edits(&mut self) {
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

    async fn open_spreadsheet_guided_save(&mut self) {
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

    fn record_spreadsheet_definitely_not_sent(&mut self, reason: String) {
        self.spreadsheet_save_lifecycle = None;
        self.state.set_error(format_mutation_outcome_not_sent(
            MutationOutcome::DefinitelyNotSent { reason },
        ));
    }

    fn format_spreadsheet_edit_error(error: SpreadsheetEditError) -> String {
        match error {
            SpreadsheetEditError::MultiRowSaveAllDisabled => {
                "Saving all spreadsheet rows is disabled".to_string()
            }
            SpreadsheetEditError::DefinitelyNotSent { reason } => {
                format_mutation_outcome_not_sent(MutationOutcome::DefinitelyNotSent { reason })
            }
        }
    }
    // ── Command palette (Faz 6.3) ────────────────────────────────────────

    /// Route a key event into the active command palette overlay.
    /// Mirrors `handle_modal_key`'s "take, mutate, put back" pattern
    /// so we never hold two borrows on `state` at once.
    async fn handle_palette_key(&mut self, key: KeyEvent) {
        let Some(mut palette) = self.state.palette.take() else {
            return;
        };

        match key.code {
            KeyCode::Esc => {
                // Cancel — drop the palette entirely.
                return;
            }
            KeyCode::Enter => {
                if let Some(cmd) = palette.current() {
                    self.dispatch_command(cmd).await;
                }
                return;
            }
            KeyCode::Down | KeyCode::Tab => {
                let len = palette.filter().len();
                palette.next(len);
            }
            KeyCode::Up | KeyCode::BackTab => {
                palette.prev();
            }
            KeyCode::Backspace => {
                palette.query.backspace();
                palette.selected = 0;
            }
            KeyCode::Char(ch) => {
                palette.query.insert(ch);
                palette.selected = 0;
            }
            _ => {}
        }

        self.state.palette = Some(palette);
    }

    /// Run the action behind a [`Command`].
    async fn dispatch_command(&mut self, cmd: crate::state::palette::Command) {
        use crate::state::palette::Command as C;
        match cmd {
            C::GotoTables => self.state.current_tab = Tab::Tables,
            C::GotoSql => self.state.current_tab = Tab::Sql,
            C::GotoLogs => self.state.current_tab = Tab::Logs,
            C::GotoMetrics => self.state.current_tab = Tab::Metrics,
            C::GotoModule => self.state.current_tab = Tab::Module,
            C::GotoLive => self.state.current_tab = Tab::Live,
            C::RefreshCurrentView => self.refresh_current_view().await,
            C::ReconnectWebSocket => {
                self.connect_ws().await;
                self.state
                    .set_notification("Reconnecting WebSocket…".to_string());
            }
            C::ToggleHelp => {
                self.state.show_help = !self.state.show_help;
                self.state.help_scroll = 0;
            }
            C::ExportCsv => {
                self.export_current_result(crate::ui::export::ExportFormat::Csv);
            }
            C::ExportJson => {
                self.export_current_result(crate::ui::export::ExportFormat::Json);
            }
            C::CopyCell => self.copy_selected_cell(),
            C::CopyRow => self.copy_selected_row(),
            C::Quit => self.state.should_quit = true,
        }
    }

    // ── Modal dialogs (Faz 5: write operations) ──────────────────────────

    /// Drop every piece of state that belongs to one guided update
    /// confirmation: the draft, the restore snapshot, and the correlated write
    /// plan. Cancellation must clear all three together, otherwise a later
    /// same-row success can act on the leftovers.
    fn discard_guided_update_confirmation_state(&mut self, plan_id: WritePlanId) {
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

    fn discard_guided_state_for_modal(&mut self, modal: &crate::state::modal::Modal) {
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
    async fn handle_modal_key(&mut self, key: KeyEvent) {
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
                KeyCode::Tab | KeyCode::Down => {
                    if !fields.is_empty() {
                        *focus = (*focus + 1) % fields.len();
                    }
                }
                KeyCode::BackTab | KeyCode::Up => {
                    if !fields.is_empty() {
                        *focus = if *focus == 0 {
                            fields.len() - 1
                        } else {
                            *focus - 1
                        };
                    }
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

    async fn handle_spreadsheet_dirty_row_choice(
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
    fn open_update_form(&mut self) {
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
    fn open_add_alias_form(&mut self) {
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
    fn open_typed_confirm_form(
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
    fn open_delete_db_form(&mut self) {
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
    fn open_truncate_table_form(&mut self) {
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
    fn open_delete_confirm(&mut self) {
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
    fn open_insert_form(&mut self) {
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
    fn open_reducer_form(&mut self) {
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

    /// Dispatch a finished modal action — runs the underlying API
    /// call on a background task and surfaces the result via
    /// `AppEvent::WriteOpSuccess` / `WriteOpError`. The modal is
    /// dropped (the caller already moved it out of `state.modal`).
    async fn dispatch_modal_action(&mut self, modal: crate::state::modal::Modal) {
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

    fn submit_guided_update_form(
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

    fn restore_guided_update_form(
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

    fn restore_pending_update_confirmation_form(&mut self, plan_id: WritePlanId) {
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

    async fn dispatch_confirmed_write_plan(&mut self, plan_id: WritePlanId, op_label: String) {
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
    fn spawn_add_alias(&mut self, database: String, alias: String, op_label: String) {
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
    fn spawn_delete_database(&mut self, database: String, op_label: String) {
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
    fn spawn_call_reducer(
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
    fn spawn_write_sql(&mut self, db: String, sql: String, op_label: String) {
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
    fn spawn_guided_write_sql(
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

    fn classify_guided_write_sql_error(error: &anyhow::Error) -> TransportFailure {
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

    fn parse_sql_query_http_status(detail: &str) -> Option<u16> {
        let marker = "SQL query HTTP ";
        let start = detail.find(marker)? + marker.len();
        let digits: String = detail[start..]
            .chars()
            .take_while(|ch| ch.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }

    /// Tab-complete the SQL input against the current schema.
    ///
    /// Extracts the identifier token immediately to the left of the cursor,
    /// builds a candidate list from SQL keywords plus every table/column
    /// name in the active schema, and then either (a) commits the unique
    /// completion, (b) extends the token to the longest common prefix
    /// shared by multiple matches and surfaces the candidate list as a
    /// notification, or (c) shows a "no match" notification.
    fn complete_sql_input(&mut self) {
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

    async fn execute_sql(&mut self) {
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

    async fn refresh_current_view(&mut self) {
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
                // Phase 1: Live subscriptions are disabled. Manual refresh
                // forces an immediate metadata poll of `st_client`.
                self.last_live_clients_fetch = None;
                self.maybe_refresh_live_clients();
            }
        }
    }

    async fn load_logs(&mut self) {
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
    async fn connect_ws(&mut self) {
        self.bump_server_context_generation();
        // Close existing connection if any
        if let Some(ref handle) = self.ws_handle {
            handle.close().await;
        }
        self.ws_handle = None;
        self.state.ws_connected = false;
        self.state.live_table_data.clear();

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
    async fn drain_ws_events(&mut self) {
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
    async fn shutdown_tasks(&mut self) {
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

    /// Drain terminal reports from the background-task registry.
    ///
    /// Panicked tasks are surfaced into the Activity log and as an error
    /// notification so a background panic is never silently lost. Completed
    /// and cancelled tasks are recorded at debug level only.
    fn drain_task_reports(&mut self) {
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
    async fn handle_ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Connected => {
                tracing::info!("WebSocket connected");
                self.state.ws_connected = true;
                self.state.ws_reconnect_deadline = None;
                self.state.ws_reconnect_attempt = 0;
                // Phase 1: no automatic all-table subscription. The Live
                // tab shows a disabled notice; bounded scoped Live will
                // return later.
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
            }
            WsEvent::RawText(text) => {
                // Raw frames we can't decode as structured messages — log for diagnostics
                tracing::debug!("WebSocket raw text frame ({} bytes)", text.len());
            }
        }
    }

    /// Apply a decoded WebSocket server message to the application state.
    fn handle_ws_server_message(&mut self, msg: crate::api::types::WsServerMessage) {
        use crate::api::types::WsServerMessage;
        match msg {
            WsServerMessage::InitialSubscription(payload) => {
                self.bump_table_generation();
                // Initial snapshot — replace any existing live data for each table.
                let mut total_rows = 0usize;
                for table_update in payload.database_update.tables {
                    total_rows += table_update.inserts.len();
                    self.state
                        .live_table_data
                        .insert(table_update.table_name, table_update.inserts);
                }
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("Live subscription active — {total_rows} rows")),
                );
            }
            WsServerMessage::TransactionUpdate(payload) => {
                self.bump_table_generation();
                // Incremental update — apply inserts/deletes to the cached
                // live data. Deletes are matched by exact JSON value equality
                // (the server's row identity model isn't exposed in the JSON
                // protocol, so this is a best-effort match).
                let mut total_changes = 0usize;
                for table_update in payload.database_update.tables {
                    let inserts_n = table_update.inserts.len();
                    let deletes_n = table_update.deletes.len();
                    total_changes += inserts_n + deletes_n;
                    let entry = self
                        .state
                        .live_table_data
                        .entry(table_update.table_name)
                        .or_default();
                    if !table_update.deletes.is_empty() {
                        entry.retain(|row| !table_update.deletes.contains(row));
                    }
                    entry.extend(table_update.inserts);
                }
                if total_changes > 0 {
                    tracing::debug!("Transaction update: {total_changes} row changes");
                }
            }
            WsServerMessage::IdentityToken(payload) => {
                tracing::info!("WebSocket identity confirmed: {:?}", payload.identity);
            }
        }
    }

    // ── Async event handler ───────────────────────────────────────────────

    async fn handle_app_event(&mut self, ev: AppEvent) {
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
                let existing: Vec<_> = self.state.databases.drain(..).collect();
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
                        .and_then(|s| s.last_table.as_deref())
                        .and_then(|name| {
                            self.state.tables.iter().position(|t| t.table_name == name)
                        });
                    self.state.selected_table_idx = Some(restored.unwrap_or(0));
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

            AppEvent::TableBrowseResult { context, result } => {
                if !self.table_context_matches_current(&context) {
                    return;
                }
                self.state.query_loading = false;
                let row_count = result.row_count();
                self.state.table_browse_result = Some(result);
                self.unblock_guided_writes_after_accepted_browse(&context);
                // Reset the Tables grid scroll/selection on fresh data.
                self.tables_grid = TableGridState::new();
                self.state
                    .set_notification(format!("{row_count} rows loaded"));
                self.flush_deferred_guided_refresh_if_unowned().await;
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

async fn build_postcondition_evidence(
    client: &SpacetimeClient,
    db: &str,
    table: &TableInfo,
    postcondition_queries: PostconditionQueries,
) -> Result<PostconditionEvidence, PostconditionFetchError> {
    match postcondition_queries {
        PostconditionQueries::Delete {
            original_key_lookup,
        } => {
            let rows = run_lookup(client, db, table, &original_key_lookup).await?;
            match rows {
                LookupRows::Zero => Ok(PostconditionEvidence::Delete {
                    original_tuple_present: false,
                }),
                LookupRows::One(_) => Ok(PostconditionEvidence::Delete {
                    original_tuple_present: true,
                }),
                LookupRows::Multiple(count) => Err(PostconditionFetchError::critical(format!(
                    "complete-key delete verification returned {count} rows; expected at most 1"
                ))),
            }
        }
        PostconditionQueries::Update {
            original_key_lookup,
            new_key_lookup: None,
        } => {
            let rows = run_lookup(client, db, table, &original_key_lookup).await?;
            let row_by_original_key = match rows {
                LookupRows::Zero => None,
                LookupRows::One(row) => Some(row),
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key update verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            Ok(PostconditionEvidence::Update(Box::new(
                PostconditionUpdate {
                    table: table.clone(),
                    row_by_original_key,
                    row_by_new_key: None,
                    old_key_still_present: None,
                },
            )))
        }
        PostconditionQueries::Update {
            original_key_lookup,
            new_key_lookup: Some(new_key_lookup),
        } => {
            let old_rows = run_lookup(client, db, table, &original_key_lookup).await?;
            let old_key_still_present = match old_rows {
                LookupRows::Zero => false,
                LookupRows::One(row) => {
                    return Ok(PostconditionEvidence::Update(Box::new(
                        PostconditionUpdate {
                            table: table.clone(),
                            row_by_original_key: Some(row),
                            row_by_new_key: None,
                            old_key_still_present: Some(true),
                        },
                    )));
                }
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key old tuple verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            let new_rows = run_lookup(client, db, table, &new_key_lookup).await?;
            let row_by_new_key = match new_rows {
                LookupRows::Zero => None,
                LookupRows::One(row) => Some(row),
                LookupRows::Multiple(count) => {
                    return Err(PostconditionFetchError::critical(format!(
                        "complete-key new tuple verification returned {count} rows; expected at most 1"
                    )));
                }
            };
            Ok(PostconditionEvidence::Update(Box::new(
                PostconditionUpdate {
                    table: table.clone(),
                    row_by_original_key: None,
                    row_by_new_key,
                    old_key_still_present: Some(old_key_still_present),
                },
            )))
        }
    }
}

async fn run_lookup(
    client: &SpacetimeClient,
    db: &str,
    table: &TableInfo,
    sql: &str,
) -> Result<LookupRows, PostconditionFetchError> {
    let result = tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(db, sql))
        .await
        .map_err(|_| PostconditionFetchError::unknown("postcondition verification timed out"))?
        .map_err(|error| {
            PostconditionFetchError::unknown(format!(
                "postcondition verification failed: {error:#}"
            ))
        })?;
    normalize_lookup_result(table, &result)
        .map_err(|error| PostconditionFetchError::unknown(format!("{error:?}")))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PostconditionFetchError {
    Critical(String),
    Unknown(String),
}

impl PostconditionFetchError {
    fn critical(reason: impl Into<String>) -> Self {
        Self::Critical(reason.into())
    }

    fn unknown(reason: impl Into<String>) -> Self {
        Self::Unknown(reason.into())
    }
}

fn verification_report_from_postcondition_fetch_error(
    error: PostconditionFetchError,
) -> WriteVerificationReport {
    let outcome = match error {
        PostconditionFetchError::Critical(reason) => {
            MutationOutcome::CriticalSafetyError { reason }
        }
        PostconditionFetchError::Unknown(reason) => MutationOutcome::Unknown { reason },
    };
    WriteVerificationReport { outcome }
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

// ── Tests for modal helpers ──────────────────────────────────────────────────
//
// Kept inline because `draw_frame` follows this block and logically closes out
// the app module.
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod modal_helper_tests {
    use super::*;

    use crate::api::types::{ColumnInfo, TableInfo};

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
        app.handle_spreadsheet_dirty_row_choice(
            crate::state::modal::DirtyRowChoice::Discard,
            1,
            900,
        )
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
    async fn spreadsheet_stale_generation_creates_no_plan_and_selection_invalidation_clears_state()
    {
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
    async fn spreadsheet_stale_guided_success_does_not_refresh_or_mutate_active_spreadsheet_state()
    {
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
    async fn guided_success_invalidates_same_row_update_form_before_unlock_and_blocks_stale_dispatch(
    ) {
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
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
            app.state.table_browse_result.is_none()
                || app.guided_write_data_is_stale(&users_table()),
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
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
            app.state.table_browse_result.is_none()
                || app.guided_write_data_is_stale(&users_table()),
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
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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

        let context =
            app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
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
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
        let mut plan_a =
            make_write_plan(WritePlanId(plan_id.0 + 100), "db", None, "users", 1, true);
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
    async fn guided_update_empty_update_keeps_exact_form_draft_and_esc_drains_deferred_refresh_once(
    ) {
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
        assert_guided_update_form_preserved(
            &app,
            draft_plan,
            draft_generation,
            1,
            &["1", "Ada", "5"],
        );
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
        lock_plan.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
        plan_a.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
        plan.original_primary_key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            crate::state::safety::PrimaryKeyPart {
                column_id: 511,
                value: crate::state::safety::SqlValue::U64(1),
            },
        ])
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
        })
        .await;
        assert_eq!(
            app.guided_write_blocked_generation(&users_table()),
            None,
            "a manual refresh started after the outcome must unblock"
        );
    }

    #[tokio::test]
    async fn guided_owned_form_and_confirm_modals_clear_when_generation_discards_their_private_state(
    ) {
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
        let context =
            app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
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
        let context =
            app.next_request_context(crate::effects::request::RequestScope::SqlWorkspace {
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
    async fn deferred_guided_refresh_schema_loaded_rebases_matching_intent_and_accepts_error_terminal(
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
    async fn deferred_guided_refresh_generic_write_success_defers_for_active_read_and_drains_once()
    {
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
    async fn deferred_guided_refresh_waits_for_generic_current_table_guided_lock_then_drains_once()
    {
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
    async fn deferred_guided_refresh_is_dropped_after_away_and_back_navigation_to_same_named_table()
    {
        let mut app = test_app();
        app.state.databases = vec!["db".to_string(), "other".to_string()];
        app.state.selected_database_idx = Some(0);
        app.state.tables = schema_with_table("users").tables;
        app.state.selected_table_idx = Some(0);
        app.state.current_tab = Tab::Tables;
        queue_users_refresh(&mut app);

        app.state.sidebar_focus = SidebarFocus::Databases;
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

        app.handle_spreadsheet_dirty_row_choice(
            crate::state::modal::DirtyRowChoice::Discard,
            1,
            700,
        )
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
            App::classify_guided_write_sql_error(&anyhow::anyhow!(
                "SQL query HTTP 503: unavailable"
            )),
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

        let protocol_error = App::classify_guided_write_sql_error(&anyhow::anyhow!(
            "decode failed: invalid JSON frame"
        ));
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
            TransportFailure::Validation(
                "error sending request: HTTP/2 protocol stream error".into()
            )
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
    async fn spreadsheet_postcondition_planning_failure_clears_marker_consumes_plan_without_dispatch(
    ) {
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
        app.state.sidebar_focus = SidebarFocus::Databases;
        app.nav_down().await;
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
        let different_table =
            make_write_plan(WritePlanId(3), "db", Some("public"), "posts", 7, true);
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
        let critical =
            verification_report_from_postcondition_fetch_error(PostconditionFetchError::critical(
                "complete-key new tuple verification returned 2 rows",
            ));
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
