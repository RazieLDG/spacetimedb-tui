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
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers,
};
use tokio::sync::mpsc;

use ratatui::widgets::Widget;

use crate::{
    api::{
        ws::{WsConfig, WsEvent, WsHandle},
        SpacetimeClient,
    },
    config::Config,
    state::{
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
    DatabasesLoaded(Vec<String>),
    /// Tables / schema fetched for the selected database.
    SchemaLoaded(crate::api::types::SchemaResponse),
    /// SQL query result arrived (user-typed SQL in the SQL console tab).
    QueryResult {
        result: crate::api::types::QueryResult,
        duration: Duration,
        sql: String,
    },
    /// Table-browse result arrived (triggered by selecting a table from the
    /// sidebar). Kept separate from `QueryResult` so the Tables tab and the
    /// SQL tab do not share state.
    TableBrowseResult {
        result: crate::api::types::QueryResult,
    },
    /// Table-browse load failed.
    TableBrowseError { error: String },
    /// SQL query failed.
    QueryError { sql: String, error: String },
    /// Log lines fetched.
    LogsLoaded(Vec<crate::api::types::LogEntry>),
    /// Metrics fetched.
    MetricsLoaded(crate::state::MetricsSnapshot),
    /// A live log line from WebSocket.
    LogLine(crate::api::types::LogEntry),
    /// Ping result.
    PingResult(bool),
    /// Generic notification.
    Notification(String),
    /// Generic error.
    Error(String),
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
}

/// How often the Metrics tab automatically refreshes server-side metrics.
const METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(10);

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
        }
    }

    // ── Bootstrap ─────────────────────────────────────────────────────────

    /// Perform the initial connection check and database listing.
    pub async fn bootstrap(&mut self) {
        self.state.connection.status = ConnectionStatus::Connecting;
        let client = self.client.clone();
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            let ping_ok = matches!(
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.ping()).await,
                Ok(true)
            );
            send_event(&tx, AppEvent::PingResult(ping_ok));
            if !ping_ok {
                return;
            }
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.list_databases()).await {
                Ok(Ok(dbs)) => send_event(&tx, AppEvent::DatabasesLoaded(dbs)),
                Ok(Err(e)) => {
                    send_event(&tx, AppEvent::Error(format!("list_databases: {e:#}")))
                }
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
                .draw(|frame| draw_frame(frame, &mut self.state, &self.sql_input, &mut self.tables_grid, &mut self.sql_grid))
                .context("Terminal draw failed")?;

            // Poll for crossterm events (non-blocking, timeout = TICK_RATE)
            if event::poll(TICK_RATE).context("event::poll failed")? {
                match event::read().context("event::read failed")? {
                    Event::Key(key) => {
                        self.handle_key(key).await;
                    }
                    Event::Resize(_, _) => {
                        self.state.needs_redraw = true;
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

            // Throttled background refresh of server metrics while the
            // Metrics tab is visible.
            self.maybe_refresh_metrics();

            // Expire notifications
            self.state.tick_notifications(Duration::from_secs(5));

            if self.state.should_quit {
                break;
            }
        }

        Ok(())
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
        tokio::spawn(async move {
            if let Ok(Ok(text)) =
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_metrics()).await
            {
                let snapshot = parse_prometheus_metrics(&text);
                send_event(&tx, AppEvent::MetricsLoaded(snapshot));
            }
        });
    }

    // ── Key dispatch ──────────────────────────────────────────────────────

    /// Dispatch a keyboard event to the appropriate handler.
    ///
    /// Uses explicit `return` statements to make early-exit control flow clear.
    #[allow(clippy::needless_return)]
    async fn handle_key(&mut self, key: KeyEvent) {
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
                    self.state.set_notification("Reconnecting WebSocket…".to_string());
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
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.state.show_help = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.state.help_scroll = self.state.help_scroll.saturating_add(1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(1);
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
                self.nav_up();
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
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.cycle_sort(grid.selected_col);
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
                self.state.set_notification(format!(
                    "Log filter: {}",
                    self.state.log_filter_level
                ));
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
            KeyCode::Char('<') | KeyCode::Char('H') if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) => {
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.scroll_left();
                return;
            }
            KeyCode::Char('>') | KeyCode::Char('L') if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) => {
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
            KeyCode::Char(ch) if self.state.focus == FocusPanel::Sidebar && !ch.is_ascii_control() => {
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
            FocusPanel::Sidebar => {
                match self.state.sidebar_focus {
                    SidebarFocus::Databases => {
                        let old = self.state.selected_database_idx;
                        self.state.database_next();
                        if self.state.selected_database_idx != old {
                            self.load_schema().await;
                        }
                    }
                    SidebarFocus::Tables => {
                        self.state.table_next();
                    }
                }
            }
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

    fn nav_up(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => match self.state.sidebar_focus {
                SidebarFocus::Databases => {
                    self.state.database_prev();
                }
                SidebarFocus::Tables => {
                    self.state.table_prev();
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
                    self.state.selected_table_idx = if self.state.tables.is_empty() {
                        None
                    } else {
                        Some(0)
                    };
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
                        if !self.state.tables.is_empty() && self.state.selected_table_idx.is_none() {
                            self.state.selected_table_idx = Some(0);
                        }
                    }
                    SidebarFocus::Tables => {
                        // Load the selected table's data
                        self.load_table_data().await;
                        self.state.focus = FocusPanel::Main;
                        self.state.current_tab = Tab::Tables;
                        self.tables_grid = TableGridState::new();
                    }
                }
            }
            FocusPanel::Main => {
                if self.state.current_tab == Tab::Sql {
                    self.state.focus = FocusPanel::SqlInput;
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
        self.state.tables.clear();
        self.state.selected_table_idx = None;
        self.state.current_schema = None;

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_schema(&db)).await {
                Ok(Ok(schema)) => send_event(&tx, AppEvent::SchemaLoaded(schema)),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::Error(format!("Schema load failed: {e:#}")),
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::Error("Schema load timed out".to_string()),
                ),
            }
        });
    }

    async fn load_table_data(&mut self) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        let table = match self.state.selected_table() {
            Some(t) => t.table_name.clone(),
            None => return,
        };

        self.state.query_loading = true;
        self.state.table_browse_result = None;

        let sql = format!("SELECT * FROM {table} LIMIT 200");
        let client = self.client.clone();
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql)).await {
                Ok(Ok(result)) => send_event(&tx, AppEvent::TableBrowseResult { result }),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        error: "table load timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Return a reference to the `QueryResult` / `TableGridState` pair
    /// that backs the currently focused data-grid tab, together with
    /// the table-name hint (if any) used for notifications.
    fn active_grid(
        &self,
    ) -> Option<(&crate::api::types::QueryResult, &TableGridState)> {
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
                self.state
                    .set_error(format!("Clipboard copy failed: {e}"));
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
                self.state
                    .set_error(format!("Clipboard copy failed: {e}"));
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
                        _ => av
                            .to_ascii_lowercase()
                            .cmp(&bv.to_ascii_lowercase()),
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

    /// Tab-complete the SQL input against the current schema.
    ///
    /// Extracts the identifier token immediately to the left of the cursor,
    /// builds a candidate list from SQL keywords plus every table/column
    /// name in the active schema, and then either (a) commits the unique
    /// completion, (b) extends the token to the longest common prefix
    /// shared by multiple matches and surfaces the candidate list as a
    /// notification, or (c) shows a "no match" notification.
    fn complete_sql_input(&mut self) {
        use crate::ui::components::completion::{complete, build_candidates, CompletionResult};

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
                self.state.set_error("No database selected — pick one from the sidebar".to_string());
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

        tokio::spawn(async move {
            let outcome =
                tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql_clone))
                    .await;
            match outcome {
                Ok(Ok(result)) => send_event(
                    &tx,
                    AppEvent::QueryResult {
                        result,
                        duration: start.elapsed(),
                        sql: sql_clone,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::QueryError {
                        sql: sql_clone,
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::QueryError {
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
                self.load_table_data().await;
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
                tokio::spawn(async move {
                    if let Ok(Ok(text)) =
                        tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_metrics()).await
                    {
                        let snapshot = parse_prometheus_metrics(&text);
                        send_event(&tx, AppEvent::MetricsLoaded(snapshot));
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
        }
    }

    async fn load_logs(&mut self) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_logs(&db, 500, false))
                .await
            {
                Ok(Ok(logs)) => send_event(&tx, AppEvent::LogsLoaded(logs)),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::Error(format!("Logs fetch failed: {e:#}")),
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::Error("Logs fetch timed out".to_string()),
                ),
            }
        });
    }

    // ── WebSocket integration ─────────────────────────────────────────────

    /// Connect a WebSocket subscription for the currently selected database.
    ///
    /// Closes any existing WebSocket connection before opening a new one and
    /// clears any stale live-data cache from a previous database.
    async fn connect_ws(&mut self) {
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

    /// Handle a single WebSocket event.
    async fn handle_ws_event(&mut self, event: WsEvent) {
        match event {
            WsEvent::Connected => {
                tracing::info!("WebSocket connected");
                self.state.ws_connected = true;
                self.state.ws_reconnect_deadline = None;
                self.state.ws_reconnect_attempt = 0;
                // Subscribe to all user tables after connection
                self.ws_subscribe_all_tables().await;
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
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("WebSocket disconnected: {reason}")),
                );
            }
            WsEvent::Reconnecting { attempt, delay_ms } => {
                tracing::info!(
                    "WebSocket reconnect attempt {attempt} in {delay_ms}ms"
                );
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

    /// Send subscription queries for all user tables in the current schema.
    async fn ws_subscribe_all_tables(&mut self) {
        let queries: Vec<String> = self
            .state
            .tables
            .iter()
            .filter(|t| t.table_type != "system")
            .map(|t| format!("SELECT * FROM {}", t.table_name))
            .collect();

        if queries.is_empty() {
            return;
        }

        if let Some(ref handle) = self.ws_handle {
            if let Err(e) = handle.subscribe(queries, 1).await {
                tracing::warn!("WS subscribe failed: {e}");
            }
        }
    }

    /// Apply a decoded WebSocket server message to the application state.
    fn handle_ws_server_message(&mut self, msg: crate::api::types::WsServerMessage) {
        use crate::api::types::WsServerMessage;
        match msg {
            WsServerMessage::InitialSubscription(payload) => {
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
                    AppEvent::Notification(format!(
                        "Live subscription active — {total_rows} rows"
                    )),
                );
            }
            WsServerMessage::TransactionUpdate(payload) => {
                // Incremental update — apply inserts/deletes to the cached
                // live data. Deletes are matched by exact JSON value equality
                // (the server's row identity model isn't exposed in the JSON
                // protocol, so this is a best-effort match).
                let mut total_changes = 0usize;
                for table_update in payload.database_update.tables {
                    total_changes += table_update.inserts.len() + table_update.deletes.len();
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
                tracing::info!(
                    "WebSocket identity confirmed: {:?}",
                    payload.identity
                );
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

            AppEvent::DatabasesLoaded(dbs) => {
                self.state.connection.status = ConnectionStatus::Connected;
                // Preserve any pre-selected DB
                let existing: Vec<_> = self.state.databases.drain(..).collect();
                self.state.databases = dbs;
                for db in existing {
                    if !self.state.databases.contains(&db) {
                        self.state.databases.insert(0, db);
                    }
                }
                if !self.state.databases.is_empty() && self.state.selected_database_idx.is_none() {
                    self.state.select_database(0);
                    self.load_schema().await;
                }
            }

            AppEvent::SchemaLoaded(schema) => {
                self.state.tables = schema.tables.clone();
                if !self.state.tables.is_empty() && self.state.selected_table_idx.is_none() {
                    self.state.selected_table_idx = Some(0);
                }
                self.state.current_schema = Some(schema);
                let table_count = self.state.tables.len();
                send_event(
                    &self.event_tx,
                    AppEvent::Notification(format!("Schema loaded — {table_count} tables")),
                );
                // Establish WebSocket subscription for live data
                self.connect_ws().await;
            }

            AppEvent::QueryResult { result, duration, sql } => {
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
                self.state.set_notification(format!("{row_count} rows returned"));
            }

            AppEvent::QueryError { sql, error } => {
                self.state.query_loading = false;
                self.state.push_sql_history(SqlHistoryEntry {
                    sql,
                    executed_at: chrono::Utc::now(),
                    duration: Duration::ZERO,
                    row_count: None,
                    error: Some(error.clone()),
                });
                self.state.set_error(error);
            }

            AppEvent::TableBrowseResult { result } => {
                self.state.query_loading = false;
                let row_count = result.row_count();
                self.state.table_browse_result = Some(result);
                // Reset the Tables grid scroll/selection on fresh data.
                self.tables_grid = TableGridState::new();
                self.state
                    .set_notification(format!("{row_count} rows loaded"));
            }

            AppEvent::TableBrowseError { error } => {
                self.state.query_loading = false;
                self.state.set_error(error);
            }

            AppEvent::LogsLoaded(logs) => {
                self.state.extend_logs(logs);
                self.state.set_notification("Logs refreshed".to_string());
            }

            AppEvent::MetricsLoaded(snapshot) => {
                self.state.update_metrics(snapshot);
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
            snapshot.extra.insert(key.to_string(), serde_json::json!(val));
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
        components::{help::HelpOverlay, status_bar::StatusBar},
        layout::render_layout,
        sidebar::render_sidebar,
        tabs::{
            logs::render_logs,
            metrics::render_metrics,
            module::render_module,
            sql::render_sql,
            tables::render_tables,
        },
    };
    use ratatui::layout::{Constraint, Direction, Layout};

    let area = frame.area();

    // ── Outer layout: content + status bar ───────────────────────────────
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let main_area   = outer[0];
    let status_area = outer[1];

    // ── Render chrome (title bar, tab bar, sidebar border) ────────────────
    let content_areas = render_layout(main_area, frame.buffer_mut(), state);

    // ── Sidebar ───────────────────────────────────────────────────────────
    render_sidebar(content_areas.sidebar, frame.buffer_mut(), state);

    // ── Tab content ───────────────────────────────────────────────────────
    match state.current_tab {
        crate::state::Tab::Tables => {
            render_tables(content_areas.content, frame.buffer_mut(), state, tables_grid);
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
    }

    // ── Status bar ────────────────────────────────────────────────────────
    StatusBar::new(state).render(status_area, frame.buffer_mut());

    // ── Help overlay (drawn on top of everything) ─────────────────────────
    if state.show_help {
        HelpOverlay::new(state.help_scroll).render(area, frame.buffer_mut());
    }
}
