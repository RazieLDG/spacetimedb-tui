//! SpacetimeDB API response types.
//!
//! All types implement `serde::Deserialize` so they can be decoded directly
//! from the JSON payloads returned by the SpacetimeDB HTTP and WebSocket APIs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// SQL query response
// ---------------------------------------------------------------------------

/// A single column descriptor returned in a SQL query response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaElement {
    /// Column name.
    pub name: String,
    /// SpacetimeDB algebraic type tag (e.g. `"String"`, `"U64"`, …).
    pub algebraic_type: Value,
}

/// The full result of a `POST /v1/sql/<database>` request.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryResult {
    /// Column descriptors, in the same order as each row.
    pub schema: Vec<SchemaElement>,
    /// Data rows.  Each inner `Vec` has one entry per column.
    pub rows: Vec<Vec<Value>>,
    /// Server-side execution time in microseconds.
    #[serde(default)]
    pub total_duration_micros: u64,
}

impl QueryResult {
    /// Returns the number of rows in this result.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Returns the number of columns in this result.
    pub fn column_count(&self) -> usize {
        self.schema.len()
    }

    /// Returns column names as a `Vec<&str>`.
    pub fn column_names(&self) -> Vec<&str> {
        self.schema.iter().map(|s| s.name.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// Schema / catalog types
// ---------------------------------------------------------------------------

/// All system tables in SpacetimeDB 2.0.
pub const SYSTEM_TABLES: &[&str] = &[
    "st_table",
    "st_column",
    "st_index",
    "st_constraint",
    "st_sequence",
    "st_client",
    "st_scheduled",
    "st_module",
    "st_var",
    "st_view",
];

/// A single table entry inside a schema response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TableInfo {
    pub table_id: u32,
    pub table_name: String,
    /// `"system"` or `"user"`.
    #[serde(default)]
    pub table_type: String,
    /// `"public"` or `"private"`.
    #[serde(default)]
    pub table_access: String,
    #[serde(default)]
    pub columns: Vec<ColumnInfo>,
    #[serde(default)]
    pub indexes: Vec<IndexInfo>,
    #[serde(default)]
    pub constraints: Vec<Value>,
}

/// Metadata for a single column inside a `TableInfo`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ColumnInfo {
    pub col_id: u32,
    pub col_name: String,
    pub col_type: Value,
    #[serde(default)]
    pub is_autoinc: bool,
}

/// Metadata for a single index inside a `TableInfo`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexInfo {
    pub index_id: u32,
    pub index_name: String,
    pub index_type: String,
    /// Column positions covered by this index.
    #[serde(default)]
    pub columns: Vec<u32>,
    #[serde(default)]
    pub is_unique: bool,
}

/// Metadata for a single reducer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReducerInfo {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ReducerParam>,
}

/// A single parameter of a reducer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReducerParam {
    pub name: String,
    pub algebraic_type: Value,
}

/// The full schema response for a database.
///
/// SpacetimeDB returns a JSON object with a `typespace` (the type registry)
/// and a list of `tables`.  Reducers are included when available.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SchemaResponse {
    /// The algebraic type registry shared by all tables/reducers.
    #[serde(default)]
    pub typespace: Value,
    /// All tables in the database.
    #[serde(default)]
    pub tables: Vec<TableInfo>,
    /// All reducers exposed by the database module.
    #[serde(default)]
    pub reducers: Vec<ReducerInfo>,
}

/// A convenience alias used throughout the codebase.
pub type Schema = SchemaResponse;

// ---------------------------------------------------------------------------
// Log entry
// ---------------------------------------------------------------------------

/// Severity level of a log message.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Panic,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Panic => "PANIC",
            LogLevel::Unknown => "?",
        };
        write!(f, "{s}")
    }
}

/// A single log line emitted by a SpacetimeDB module.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LogEntry {
    /// When the message was produced (may be absent in older server versions).
    #[serde(default)]
    pub ts: Option<DateTime<Utc>>,
    /// Log level.
    pub level: LogLevel,
    /// The human-readable log message.
    pub message: String,
    /// Target / module path (optional).
    #[serde(default)]
    pub target: Option<String>,
    /// Filename (optional).
    #[serde(default)]
    pub filename: Option<String>,
    /// Line number inside the file (optional).
    #[serde(default)]
    pub line_number: Option<u32>,
}

impl LogEntry {
    /// Format the entry as a single display line.
    pub fn display_line(&self) -> String {
        let ts = self
            .ts
            .map(|t| t.format("%H:%M:%S%.3f").to_string())
            .unwrap_or_else(|| "??:??:??".to_string());
        format!("[{}] {} {}", ts, self.level, self.message)
    }
}

// ---------------------------------------------------------------------------
// WebSocket / subscription message types
// ---------------------------------------------------------------------------

/// The type of a WebSocket message received from SpacetimeDB.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerMessage {
    /// Initial data snapshot after subscribing to a query.
    InitialSubscription {
        database_update: DatabaseUpdate,
        request_id: u32,
        total_host_execution_duration_micros: u64,
    },
    /// Incremental update pushed by the server.
    TransactionUpdate {
        status: TransactionStatus,
        timestamp: Value,
        caller_identity: String,
        caller_address: String,
        reducer_call: Value,
        energy_quanta_used: Value,
        total_host_execution_duration_micros: u64,
        database_update: DatabaseUpdate,
    },
    /// Server acknowledges an identity.
    IdentityToken {
        identity: String,
        token: String,
        address: String,
    },
    /// Catch-all for unknown message types.
    #[serde(other)]
    Unknown,
}

/// Status of a committed transaction.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionStatus {
    Committed,
    Failed,
    OutOfEnergy,
}

/// A collection of table row updates within a transaction.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DatabaseUpdate {
    #[serde(default)]
    pub tables: Vec<TableUpdate>,
}

/// Row-level inserts/deletes for a single table.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TableUpdate {
    pub table_id: u32,
    pub table_name: String,
    #[serde(default)]
    pub num_rows: u64,
    #[serde(default)]
    pub inserts: Vec<Value>,
    #[serde(default)]
    pub deletes: Vec<Value>,
}
