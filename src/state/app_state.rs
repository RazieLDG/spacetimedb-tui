//! Central application state for the SpacetimeDB TUI.
//!
//! [`AppState`] is the single source of truth consumed by every UI widget.
//! It is **not** `Send` or `Sync` by design; all mutations happen on the main
//! thread inside the synchronous event loop.  Background tasks communicate
//! via channels and the event loop applies mutations here.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::api::types::{LogEntry, LogLevel, QueryResult, Schema, TableInfo};

// ---------------------------------------------------------------------------
// Tab / focus enums
// ---------------------------------------------------------------------------

/// Top-level tabs shown in the main pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    /// Table browser (rows of selected table).
    Query,
    /// Interactive SQL query editor and results view.
    Schema,
    /// Live log viewer.
    Logs,
    /// Server / module metrics.
    Metrics,
    /// Module inspector (reducers, tables, scheduled).
    Module,
}

impl Tab {
    pub const ALL: &'static [Tab] = &[Tab::Query, Tab::Schema, Tab::Logs, Tab::Metrics, Tab::Module];

    pub fn title(&self) -> &'static str {
        match self {
            Tab::Query   => "Tables",
            Tab::Schema  => "SQL",
            Tab::Logs    => "Logs",
            Tab::Metrics => "Metrics",
            Tab::Module  => "Module",
        }
    }

    /// Cycle to the next tab.
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Cycle to the previous tab.
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl std::fmt::Display for Tab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.title())
    }
}

/// Which panel currently owns keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPanel {
    /// The left-hand database/table sidebar.
    Sidebar,
    /// The main content area (query editor, schema view, …).
    Main,
    /// The SQL input box at the bottom.
    SqlInput,
    /// A modal dialog (e.g. error popup, help overlay).
    #[allow(dead_code)]
    Modal,
}

/// Which item in the sidebar is highlighted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFocus {
    /// The database list at the top of the sidebar.
    Databases,
    /// The table list below the selected database.
    Tables,
}

// ---------------------------------------------------------------------------
// Connection state
// ---------------------------------------------------------------------------

/// Current state of the server connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not yet attempted.
    Disconnected,
    /// Connection attempt in progress.
    Connecting,
    /// Successfully connected and authenticated.
    Connected,
    /// Connection was lost; contains a human-readable reason.
    Error(String),
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionStatus::Disconnected => write!(f, "Disconnected"),
            ConnectionStatus::Connecting => write!(f, "Connecting…"),
            ConnectionStatus::Connected => write!(f, "Connected"),
            ConnectionStatus::Error(e) => write!(f, "Error: {e}"),
        }
    }
}

/// Connection parameters and live status.
#[derive(Debug, Clone)]
pub struct ConnectionInfo {
    /// e.g. `"http://localhost:3000"`
    pub base_url: String,
    /// Current connection status.
    pub status: ConnectionStatus,
    /// Server version string, if reported (populated when available).
    #[allow(dead_code)]
    pub server_version: Option<String>,
    /// Authenticated identity token, if present (for display in status bar).
    #[allow(dead_code)]
    pub auth_token: Option<String>,
    /// When the last successful connection was made.
    #[allow(dead_code)]
    pub connected_at: Option<DateTime<Utc>>,
}

impl ConnectionInfo {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            status: ConnectionStatus::Disconnected,
            server_version: None,
            auth_token: None,
            connected_at: None,
        }
    }

    /// Returns `true` when the connection is in the `Connected` state.
    #[allow(dead_code)]
    pub fn is_connected(&self) -> bool {
        self.status == ConnectionStatus::Connected
    }
}

// ---------------------------------------------------------------------------
// SQL history
// ---------------------------------------------------------------------------

/// Maximum number of SQL history entries retained.
const SQL_HISTORY_LIMIT: usize = 200;

/// A single entry in the SQL execution history.
#[derive(Debug, Clone)]
pub struct SqlHistoryEntry {
    /// The SQL text that was executed.
    pub sql: String,
    /// When it was executed.
    pub executed_at: DateTime<Utc>,
    /// How long the query took (round-trip including network).
    pub duration: Duration,
    /// Row count returned, or `None` if the query errored.
    /// Available for display in the history panel and future export features.
    #[allow(dead_code)]
    pub row_count: Option<usize>,
    /// Error message, if the query failed.
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// A snapshot of server / module metrics.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    /// Total reducer calls processed.
    pub total_reducer_calls: u64,
    /// Total energy quanta consumed.
    pub total_energy_used: u64,
    /// Number of connected WebSocket clients.
    pub connected_clients: u64,
    /// Module memory usage in bytes.
    pub memory_bytes: u64,
    /// When this snapshot was taken.
    pub sampled_at: Option<DateTime<Utc>>,
    /// Raw key-value pairs for metrics not captured by the fields above.
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Table data cache
// ---------------------------------------------------------------------------

/// A cached query result for a specific table.
#[derive(Debug, Clone)]
pub struct TableCache {
    /// The cached result set.
    pub result: QueryResult,
    /// When the cache entry was populated (used for cache expiry checks).
    #[allow(dead_code)]
    pub fetched_at: Instant,
    /// Whether a refresh is currently in flight.
    #[allow(dead_code)]
    pub loading: bool,
}

// ---------------------------------------------------------------------------
// Log buffer
// ---------------------------------------------------------------------------

/// Maximum log lines kept in memory.
const LOG_BUFFER_LIMIT: usize = 10_000;

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// The complete application state.
///
/// This struct is intentionally large — it is the single place where all UI
/// state lives, making it easy to reason about what the TUI is displaying at
/// any point in time.
#[derive(Debug)]
pub struct AppState {
    // ------------------------------------------------------------------
    // Connection
    // ------------------------------------------------------------------
    /// Connection info and status.
    pub connection: ConnectionInfo,

    // ------------------------------------------------------------------
    // Database / table navigation
    // ------------------------------------------------------------------
    /// All databases visible to the current identity.
    pub databases: Vec<String>,
    /// Index of the currently selected database in `databases`.
    pub selected_database_idx: Option<usize>,
    /// Tables belonging to the currently selected database.
    pub tables: Vec<TableInfo>,
    /// Index of the currently selected table in `tables`.
    pub selected_table_idx: Option<usize>,
    /// Schema for the currently selected database.
    pub current_schema: Option<Schema>,

    // ------------------------------------------------------------------
    // Tab / focus
    // ------------------------------------------------------------------
    /// The currently visible top-level tab.
    pub current_tab: Tab,
    /// Which panel owns keyboard focus.
    pub focus: FocusPanel,
    /// Which section of the sidebar is highlighted.
    pub sidebar_focus: SidebarFocus,

    // ------------------------------------------------------------------
    // Query tab
    // ------------------------------------------------------------------
    /// Text currently in the SQL input box.
    pub sql_input: String,
    /// Cursor position (byte offset) inside `sql_input`.
    pub sql_cursor: usize,
    /// Result of the most recently executed query.
    pub query_result: Option<QueryResult>,
    /// Scroll offset for the results table (row index of the top visible row).
    /// Managed by `TableGridState`; kept here for persistence across tab switches.
    #[allow(dead_code)]
    pub result_scroll_row: usize,
    /// Scroll offset for the results table (column index of the leftmost visible column).
    /// Managed by `TableGridState`; kept here for persistence across tab switches.
    #[allow(dead_code)]
    pub result_scroll_col: usize,
    /// Whether a query is currently in flight.
    pub query_loading: bool,

    // ------------------------------------------------------------------
    // SQL history
    // ------------------------------------------------------------------
    /// Ordered list of past SQL executions (most recent last).
    pub sql_history: VecDeque<SqlHistoryEntry>,
    /// Index into `sql_history` when the user is browsing history (↑/↓).
    pub history_cursor: Option<usize>,

    // ------------------------------------------------------------------
    // Table data cache
    // ------------------------------------------------------------------
    /// Cached query results keyed by `"<database>.<table_name>"`.
    pub table_cache: HashMap<String, TableCache>,

    // ------------------------------------------------------------------
    // Log buffer
    // ------------------------------------------------------------------
    /// Buffered log lines (capped at `LOG_BUFFER_LIMIT`).
    pub log_buffer: VecDeque<LogEntry>,
    /// Scroll offset for the log view (index of the top visible line).
    pub log_scroll: usize,
    /// Whether log auto-scroll (follow mode) is enabled.
    pub log_follow: bool,
    /// Minimum log level to display.
    /// Used by `visible_logs()` for filtering; UI key binding to change it is a future enhancement.
    pub log_filter_level: LogLevel,

    // ------------------------------------------------------------------
    // Metrics
    // ------------------------------------------------------------------
    /// Latest metrics snapshot.
    pub metrics: MetricsSnapshot,
    /// Historical metric samples (for sparkline charts).
    pub metrics_history: VecDeque<MetricsSnapshot>,

    // ------------------------------------------------------------------
    // Error / notification state
    // ------------------------------------------------------------------
    /// A transient error message shown in a popup (cleared on any keypress).
    pub error_message: Option<String>,
    /// A transient informational notification.
    pub notification: Option<(String, Instant)>,

    // ------------------------------------------------------------------
    // Application lifecycle
    // ------------------------------------------------------------------
    /// Set to `true` to request a clean shutdown.
    pub should_quit: bool,
    /// Whether the terminal was resized since the last render.
    pub needs_redraw: bool,
    /// When the application was started (used by `uptime()`).
    pub started_at: Instant,

    // ------------------------------------------------------------------
    // UI-only state (not persisted)
    // ------------------------------------------------------------------
    /// Current search/filter query in the sidebar.
    pub search_query: String,
    /// Whether the help overlay is visible.
    pub show_help: bool,
    /// Scroll offset for the help overlay.
    pub help_scroll: usize,
    /// Selected reducer index in the module inspector tab.
    pub module_selected_reducer: usize,
}

impl AppState {
    /// Create a fresh `AppState` with sensible defaults.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            connection: ConnectionInfo::new(base_url),

            databases: Vec::new(),
            selected_database_idx: None,
            tables: Vec::new(),
            selected_table_idx: None,
            current_schema: None,

            current_tab: Tab::Query,
            focus: FocusPanel::Sidebar,
            sidebar_focus: SidebarFocus::Databases,

            sql_input: String::new(),
            sql_cursor: 0,
            query_result: None,
            result_scroll_row: 0,
            result_scroll_col: 0,
            query_loading: false,

            sql_history: VecDeque::new(),
            history_cursor: None,

            table_cache: HashMap::new(),

            log_buffer: VecDeque::new(),
            log_scroll: 0,
            log_follow: true,
            log_filter_level: LogLevel::Info,

            metrics: MetricsSnapshot::default(),
            metrics_history: VecDeque::new(),

            error_message: None,
            notification: None,

            should_quit: false,
            needs_redraw: true,
            started_at: Instant::now(),

            search_query: String::new(),
            show_help: false,
            help_scroll: 0,
            module_selected_reducer: 0,
        }
    }

    // ------------------------------------------------------------------
    // Database navigation helpers
    // ------------------------------------------------------------------

    /// The name of the currently selected database, if any.
    pub fn selected_database(&self) -> Option<&str> {
        self.selected_database_idx
            .and_then(|i| self.databases.get(i))
            .map(String::as_str)
    }

    /// Select the database at `idx`, resetting table selection.
    ///
    /// This is a no-op when `idx` is already the selected database, preventing
    /// unnecessary state clears on repeated navigation to the same database.
    pub fn select_database(&mut self, idx: usize) {
        if idx < self.databases.len() {
            // No-op if this database is already selected
            if self.selected_database_idx == Some(idx) {
                return;
            }
            self.selected_database_idx = Some(idx);
            self.selected_table_idx = None;
            self.tables.clear();
            self.current_schema = None;
        }
    }

    /// Move the database cursor down by one.
    pub fn database_next(&mut self) {
        if self.databases.is_empty() {
            return;
        }
        let next = match self.selected_database_idx {
            Some(i) => (i + 1).min(self.databases.len() - 1),
            None => 0,
        };
        self.select_database(next);
    }

    /// Move the database cursor up by one.
    pub fn database_prev(&mut self) {
        if self.databases.is_empty() {
            return;
        }
        let prev = match self.selected_database_idx {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.select_database(prev);
    }

    // ------------------------------------------------------------------
    // Table navigation helpers
    // ------------------------------------------------------------------

    /// The currently selected [`TableInfo`], if any.
    pub fn selected_table(&self) -> Option<&TableInfo> {
        self.selected_table_idx.and_then(|i| self.tables.get(i))
    }

    /// Move the table cursor down by one.
    pub fn table_next(&mut self) {
        if self.tables.is_empty() {
            return;
        }
        self.selected_table_idx = Some(match self.selected_table_idx {
            Some(i) => (i + 1).min(self.tables.len() - 1),
            None => 0,
        });
    }

    /// Move the table cursor up by one.
    pub fn table_prev(&mut self) {
        if self.tables.is_empty() {
            return;
        }
        self.selected_table_idx = Some(match self.selected_table_idx {
            Some(i) => i.saturating_sub(1),
            None => 0,
        });
    }

    // ------------------------------------------------------------------
    // SQL input helpers
    //
    // These methods mirror `InputState` in `ui/components/input.rs` and are
    // used in tests and available for future refactoring that consolidates
    // the dual-state design.
    // ------------------------------------------------------------------

    /// Append a character to the SQL input at the current cursor position.
    #[allow(dead_code)]
    pub fn sql_insert_char(&mut self, ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        self.sql_input.insert_str(self.sql_cursor, s);
        self.sql_cursor += s.len();
    }

    /// Delete the character immediately before the cursor (backspace).
    #[allow(dead_code)]
    pub fn sql_backspace(&mut self) {
        if self.sql_cursor == 0 || self.sql_input.is_empty() {
            return;
        }
        // Walk back to the start of the previous UTF-8 char.
        let mut cursor = self.sql_cursor;
        loop {
            cursor -= 1;
            if self.sql_input.is_char_boundary(cursor) {
                break;
            }
        }
        self.sql_input.drain(cursor..self.sql_cursor);
        self.sql_cursor = cursor;
    }

    /// Delete the character at the cursor (delete key).
    #[allow(dead_code)]
    pub fn sql_delete(&mut self) {
        if self.sql_cursor >= self.sql_input.len() {
            return;
        }
        let next = self
            .sql_input[self.sql_cursor..]
            .char_indices()
            .nth(1)
            .map(|(i, _)| self.sql_cursor + i)
            .unwrap_or(self.sql_input.len());
        self.sql_input.drain(self.sql_cursor..next);
    }

    /// Move the cursor left by one character.
    #[allow(dead_code)]
    pub fn sql_cursor_left(&mut self) {
        if self.sql_cursor == 0 {
            return;
        }
        let mut cursor = self.sql_cursor;
        loop {
            cursor -= 1;
            if self.sql_input.is_char_boundary(cursor) {
                break;
            }
        }
        self.sql_cursor = cursor;
    }

    /// Move the cursor right by one character.
    #[allow(dead_code)]
    pub fn sql_cursor_right(&mut self) {
        if self.sql_cursor >= self.sql_input.len() {
            return;
        }
        let ch = self.sql_input[self.sql_cursor..].chars().next().unwrap_or('\0');
        self.sql_cursor += ch.len_utf8();
    }

    /// Move the cursor to the beginning of the input.
    #[allow(dead_code)]
    pub fn sql_cursor_home(&mut self) {
        self.sql_cursor = 0;
    }

    /// Move the cursor to the end of the input.
    #[allow(dead_code)]
    pub fn sql_cursor_end(&mut self) {
        self.sql_cursor = self.sql_input.len();
    }

    /// Clear the SQL input and reset the cursor.
    #[allow(dead_code)]
    pub fn sql_clear(&mut self) {
        self.sql_input.clear();
        self.sql_cursor = 0;
    }

    // ------------------------------------------------------------------
    // SQL history helpers
    // ------------------------------------------------------------------

    /// Push a completed query execution into the history ring.
    pub fn push_sql_history(&mut self, entry: SqlHistoryEntry) {
        self.sql_history.push_back(entry);
        if self.sql_history.len() > SQL_HISTORY_LIMIT {
            self.sql_history.pop_front();
        }
        self.history_cursor = None;
    }

    /// Navigate to the previous history entry (↑).
    pub fn history_prev(&mut self) {
        if self.sql_history.is_empty() {
            return;
        }
        let new_cursor = match self.history_cursor {
            None => self.sql_history.len() - 1,
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(new_cursor);
        if let Some(entry) = self.sql_history.get(new_cursor) {
            self.sql_input = entry.sql.clone();
            self.sql_cursor = self.sql_input.len();
        }
    }

    /// Navigate to the next history entry (↓).
    pub fn history_next(&mut self) {
        match self.history_cursor {
            None => {}
            Some(i) if i + 1 >= self.sql_history.len() => {
                self.history_cursor = None;
                self.sql_clear();
            }
            Some(i) => {
                self.history_cursor = Some(i + 1);
                if let Some(entry) = self.sql_history.get(i + 1) {
                    self.sql_input = entry.sql.clone();
                    self.sql_cursor = self.sql_input.len();
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Log buffer helpers
    // ------------------------------------------------------------------

    /// Append a log entry to the buffer, evicting old entries if needed.
    pub fn push_log(&mut self, entry: LogEntry) {
        if self.log_buffer.len() >= LOG_BUFFER_LIMIT {
            self.log_buffer.pop_front();
            // Adjust scroll so the view doesn't jump.
            if self.log_scroll > 0 {
                self.log_scroll -= 1;
            }
        }
        self.log_buffer.push_back(entry);
        if self.log_follow {
            // Pin scroll to the bottom.
            self.log_scroll = self.log_buffer.len().saturating_sub(1);
        }
    }

    /// Append multiple log entries at once.
    pub fn extend_logs(&mut self, entries: impl IntoIterator<Item = LogEntry>) {
        for entry in entries {
            self.push_log(entry);
        }
    }

    /// Log entries that pass the current `log_filter_level`.
    ///
    /// Available for future use in the log viewer when level filtering UI is added.
    #[allow(dead_code)]
    pub fn visible_logs(&self) -> impl Iterator<Item = &LogEntry> {
        let min_level = &self.log_filter_level;
        self.log_buffer.iter().filter(move |e| level_gte(&e.level, min_level))
    }

    // ------------------------------------------------------------------
    // Metrics helpers
    // ------------------------------------------------------------------

    /// Replace the current metrics snapshot and push the old one to history.
    pub fn update_metrics(&mut self, snapshot: MetricsSnapshot) {
        const HISTORY_LIMIT: usize = 120;
        let old = std::mem::replace(&mut self.metrics, snapshot);
        self.metrics_history.push_back(old);
        if self.metrics_history.len() > HISTORY_LIMIT {
            self.metrics_history.pop_front();
        }
    }

    // ------------------------------------------------------------------
    // Error / notification helpers
    // ------------------------------------------------------------------

    /// Set a transient error message (shown in a popup).
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error_message = Some(msg.into());
    }

    /// Clear the current error message.
    pub fn clear_error(&mut self) {
        self.error_message = None;
    }

    /// Set a transient notification (auto-expires after a few seconds).
    pub fn set_notification(&mut self, msg: impl Into<String>) {
        self.notification = Some((msg.into(), Instant::now()));
    }

    /// Clear expired notifications (older than `ttl`).
    pub fn tick_notifications(&mut self, ttl: Duration) {
        if let Some((_, ts)) = &self.notification {
            if ts.elapsed() > ttl {
                self.notification = None;
            }
        }
    }

    // ------------------------------------------------------------------
    // Cache helpers
    // ------------------------------------------------------------------

    /// Cache key for a table: `"<database>.<table_name>"`.
    pub fn cache_key(database: &str, table_name: &str) -> String {
        format!("{}.{}", database, table_name)
    }

    /// Store a query result in the table cache.
    #[allow(dead_code)]
    pub fn cache_table_result(&mut self, database: &str, table_name: &str, result: QueryResult) {
        let key = Self::cache_key(database, table_name);
        self.table_cache.insert(
            key,
            TableCache {
                result,
                fetched_at: Instant::now(),
                loading: false,
            },
        );
    }

    /// Retrieve a cached result, if present and not older than `max_age`.
    #[allow(dead_code)]
    pub fn get_cached_table(
        &self,
        database: &str,
        table_name: &str,
        max_age: Duration,
    ) -> Option<&TableCache> {
        let key = Self::cache_key(database, table_name);
        self.table_cache
            .get(&key)
            .filter(|c| c.fetched_at.elapsed() <= max_age)
    }

    // ------------------------------------------------------------------
    // Uptime
    // ------------------------------------------------------------------

    /// How long the application has been running.
    ///
    /// Available for display in the status bar or metrics tab.
    #[allow(dead_code)]
    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}

// ---------------------------------------------------------------------------
// Level ordering helper
// ---------------------------------------------------------------------------

/// Returns `true` when level `a` is at least as severe as `b`.
#[allow(dead_code)]
fn level_gte(a: &LogLevel, b: &LogLevel) -> bool {
    level_rank(a) >= level_rank(b)
}

/// Numeric severity rank for log level comparison.
#[allow(dead_code)]
fn level_rank(level: &LogLevel) -> u8 {
    match level {
        LogLevel::Trace => 0,
        LogLevel::Debug => 1,
        LogLevel::Info => 2,
        LogLevel::Warn => 3,
        LogLevel::Error => 4,
        LogLevel::Panic => 5,
        LogLevel::Unknown => 6,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> AppState {
        AppState::new("http://localhost:3000")
    }

    #[test]
    fn test_tab_cycle() {
        assert_eq!(Tab::Query.next(), Tab::Schema);
        assert_eq!(Tab::Module.next(), Tab::Query);
        assert_eq!(Tab::Query.prev(), Tab::Module);
    }

    #[test]
    fn test_sql_insert_and_backspace() {
        let mut s = make_state();
        s.sql_insert_char('H');
        s.sql_insert_char('i');
        assert_eq!(s.sql_input, "Hi");
        assert_eq!(s.sql_cursor, 2);
        s.sql_backspace();
        assert_eq!(s.sql_input, "H");
        assert_eq!(s.sql_cursor, 1);
    }

    #[test]
    fn test_sql_cursor_movement() {
        let mut s = make_state();
        for ch in "hello".chars() {
            s.sql_insert_char(ch);
        }
        s.sql_cursor_home();
        assert_eq!(s.sql_cursor, 0);
        s.sql_cursor_right();
        assert_eq!(s.sql_cursor, 1);
        s.sql_cursor_end();
        assert_eq!(s.sql_cursor, 5);
    }

    #[test]
    fn test_database_navigation() {
        let mut s = make_state();
        s.databases = vec!["alpha".into(), "beta".into(), "gamma".into()];
        s.database_next();
        assert_eq!(s.selected_database(), Some("alpha"));
        s.database_next();
        assert_eq!(s.selected_database(), Some("beta"));
        s.database_prev();
        assert_eq!(s.selected_database(), Some("alpha"));
    }

    #[test]
    fn test_log_buffer_eviction() {
        let mut s = make_state();
        for i in 0..10_001usize {
            s.push_log(LogEntry {
                ts: None,
                level: LogLevel::Info,
                message: format!("line {i}"),
                target: None,
                filename: None,
                line_number: None,
            });
        }
        assert_eq!(s.log_buffer.len(), 10_000);
    }

    #[test]
    fn test_sql_history_limit() {
        let mut s = make_state();
        for i in 0..201usize {
            s.push_sql_history(SqlHistoryEntry {
                sql: format!("SELECT {i}"),
                executed_at: Utc::now(),
                duration: Duration::from_millis(1),
                row_count: Some(0),
                error: None,
            });
        }
        assert_eq!(s.sql_history.len(), 200);
    }

    #[test]
    fn test_notification_expiry() {
        let mut s = make_state();
        s.set_notification("hello");
        // Should not expire immediately.
        s.tick_notifications(Duration::from_secs(5));
        assert!(s.notification.is_some());
        // Simulate expiry by using a zero TTL.
        s.tick_notifications(Duration::ZERO);
        assert!(s.notification.is_none());
    }
}
