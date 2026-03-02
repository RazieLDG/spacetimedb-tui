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
    api::SpacetimeClient,
    config::Config,
    state::{
        AppState, ConnectionStatus, FocusPanel, SidebarFocus, SqlHistoryEntry, Tab,
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
    /// SQL query result arrived.
    QueryResult {
        result: crate::api::types::QueryResult,
        duration: Duration,
        sql: String,
    },
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
    /// Standalone SQL input state (mirrors app.state.sql_input).
    pub sql_input: InputState,
    /// Table grid state for the tables tab.
    pub tables_grid: TableGridState,
    /// Table grid state for the SQL results.
    pub sql_grid: TableGridState,
}

impl App {
    /// Create a new [`App`] from config and a pre-built client.
    pub fn new(config: &Config, client: SpacetimeClient) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let state = AppState::new(config.server_url.clone());
        Self {
            state,
            client,
            event_tx: tx,
            event_rx: rx,
            sql_input: InputState::new(),
            tables_grid: TableGridState::new(),
            sql_grid: TableGridState::new(),
        }
    }

    // ── Bootstrap ─────────────────────────────────────────────────────────

    /// Perform the initial connection check and database listing.
    pub async fn bootstrap(&mut self) {
        self.state.connection.status = ConnectionStatus::Connecting;
        let client = self.client.clone();
        let tx = self.event_tx.clone();

        tokio::spawn(async move {
            if client.ping().await {
                let _ = tx.send(AppEvent::PingResult(true));
                match client.list_databases().await {
                    Ok(dbs) => {
                        let _ = tx.send(AppEvent::DatabasesLoaded(dbs));
                    }
                    Err(e) => {
                        let _ = tx.send(AppEvent::Error(format!("list_databases: {e:#}")));
                    }
                }
            } else {
                let _ = tx.send(AppEvent::PingResult(false));
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
            loop {
                match self.event_rx.try_recv() {
                    Ok(ev) => self.handle_app_event(ev).await,
                    Err(_) => break,
                }
            }

            // Expire notifications
            self.state.tick_notifications(Duration::from_secs(5));

            if self.state.should_quit {
                break;
            }
        }

        Ok(())
    }

    // ── Key dispatch ──────────────────────────────────────────────────────

    async fn handle_key(&mut self, key: KeyEvent) {
        // ── Global always-active bindings ─────────────────────────────────
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.state.should_quit = true;
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
                    return;
                }
                KeyCode::Char('u') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.kill_to_start();
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

        // ── Error popup — any key clears it ───────────────────────────────
        if self.state.error_message.is_some() {
            self.state.clear_error();
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
                KeyCode::Up => {
                    self.state.history_prev();
                    if let Some(cursor) = self.state.history_cursor {
                        let idx = self.state.sql_history.len().saturating_sub(1 + cursor);
                        if let Some(entry) = self.state.sql_history.get(idx) {
                            self.sql_input.set(entry.sql.clone());
                        }
                    }
                }
                KeyCode::Down => {
                    self.state.history_next();
                    match self.state.history_cursor {
                        Some(cursor) => {
                            let idx = self.state.sql_history.len().saturating_sub(1 + cursor);
                            if let Some(entry) = self.state.sql_history.get(idx) {
                                self.sql_input.set(entry.sql.clone());
                            }
                        }
                        None => {
                            self.sql_input.clear();
                        }
                    }
                }
                KeyCode::Left => self.sql_input.move_left(),
                KeyCode::Right => self.sql_input.move_right(),
                KeyCode::Home => self.sql_input.home(),
                KeyCode::End => self.sql_input.end(),
                KeyCode::Backspace => self.sql_input.backspace(),
                KeyCode::Delete => self.sql_input.delete(),
                KeyCode::Char(ch) => self.sql_input.insert(ch),
                _ => {}
            }
            // Keep app state in sync for rendering
            self.state.sql_input = self.sql_input.value.clone();
            self.state.sql_cursor = self.sql_input.cursor;
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
                self.state.current_tab = Tab::Query;
                return;
            }
            KeyCode::Char('2') => {
                self.state.current_tab = Tab::Schema;
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

            // Focus toggle between sidebar and main
            KeyCode::Left | KeyCode::Char('h') if self.state.focus == FocusPanel::Main => {
                self.state.focus = FocusPanel::Sidebar;
                return;
            }
            KeyCode::Right | KeyCode::Char('l') if self.state.focus == FocusPanel::Sidebar => {
                self.state.focus = FocusPanel::Main;
                return;
            }

            // Enter SQL mode
            KeyCode::Char(':') => {
                self.state.current_tab = Tab::Schema;
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

            // Escape — go back / clear search
            KeyCode::Esc => {
                if !self.state.search_query.is_empty() {
                    self.state.search_query.clear();
                } else {
                    self.state.focus = FocusPanel::Sidebar;
                }
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

            // Page navigation in tables
            KeyCode::Char('n') if self.state.current_tab == Tab::Query => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_add(20);
                return;
            }
            KeyCode::Char('p') if self.state.current_tab == Tab::Query => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_sub(20);
                return;
            }

            // Search input (when in sidebar search mode)
            KeyCode::Backspace if self.state.focus == FocusPanel::Sidebar => {
                self.state.search_query.pop();
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
                Tab::Query => {
                    let row_count = self
                        .state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.row_count())
                        .unwrap_or(0);
                    self.tables_grid.next_row(row_count);
                }
                Tab::Schema => {
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
                Tab::Query => {
                    self.tables_grid.prev_row();
                }
                Tab::Schema => {
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
                Tab::Query => {
                    self.tables_grid.selected_row = 0;
                    self.tables_grid.scroll_row = 0;
                }
                Tab::Schema => {
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
        match self.state.focus {
            FocusPanel::Main => match self.state.current_tab {
                Tab::Query => {
                    if let Some(ref qr) = self.state.query_result {
                        let count = qr.row_count();
                        self.tables_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Schema => {
                    if let Some(ref qr) = self.state.query_result {
                        let count = qr.row_count();
                        self.sql_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Logs => {
                    self.state.log_follow = true;
                }
                _ => {}
            },
            _ => {}
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
                        self.state.current_tab = Tab::Query;
                        self.tables_grid = TableGridState::new();
                    }
                }
            }
            FocusPanel::Main => {
                if self.state.current_tab == Tab::Schema {
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
            match client.get_schema(&db).await {
                Ok(schema) => {
                    let _ = tx.send(AppEvent::SchemaLoaded(schema));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Schema load failed: {e:#}")));
                }
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
        self.state.query_result = None;

        let sql = format!("SELECT * FROM {table} LIMIT 200");
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let start = Instant::now();

        tokio::spawn(async move {
            match client.query_sql(&db, &sql).await {
                Ok(result) => {
                    let _ = tx.send(AppEvent::QueryResult {
                        result,
                        duration: start.elapsed(),
                        sql,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::QueryError {
                        sql,
                        error: format!("{e:#}"),
                    });
                }
            }
        });
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
            match client.query_sql(&db, &sql_clone).await {
                Ok(result) => {
                    let _ = tx.send(AppEvent::QueryResult {
                        result,
                        duration: start.elapsed(),
                        sql: sql_clone,
                    });
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::QueryError {
                        sql: sql_clone,
                        error: format!("{e:#}"),
                    });
                }
            }
        });
    }

    async fn refresh_current_view(&mut self) {
        match self.state.current_tab {
            Tab::Query => {
                self.load_table_data().await;
            }
            Tab::Schema => {
                // Re-execute last SQL if any
                if let Some(entry) = self.state.sql_history.back() {
                    let sql = entry.sql.clone();
                    self.sql_input.set(sql);
                    self.state.sql_input = self.sql_input.value.clone();
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
                    if let Ok(text) = client.get_metrics().await {
                        let snapshot = parse_prometheus_metrics(&text);
                        let _ = tx.send(AppEvent::MetricsLoaded(snapshot));
                    }
                    let ok = client.ping().await;
                    let _ = tx.send(AppEvent::PingResult(ok));
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
            match client.get_logs(&db, 500, false).await {
                Ok(logs) => {
                    let _ = tx.send(AppEvent::LogsLoaded(logs));
                }
                Err(e) => {
                    let _ = tx.send(AppEvent::Error(format!("Logs fetch failed: {e:#}")));
                }
            }
        });
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
                self.state.set_notification("Schema loaded".to_string());
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
        crate::state::Tab::Query => {
            render_tables(content_areas.content, frame.buffer_mut(), state, tables_grid);
        }
        crate::state::Tab::Schema => {
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
