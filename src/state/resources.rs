// Phase 2/3 foundation: wired into production event loop in Phase 3.
#![allow(dead_code)]

//! Resource state: scoped resource data, `LoadState<T>`, and request tracking.
//!
//! This module owns typed resource payloads and the per-scope generation
//! tracker. It reuses Phase 1 request types from
//! [`crate::effects::request`] and never redefines them.

use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::effects::request::{RequestContext, RequestId, RequestScope};

// ---------------------------------------------------------------------------
// Payload type aliases
// ---------------------------------------------------------------------------

pub type CatalogSnapshot = Vec<String>;
pub type SchemaSnapshot = crate::api::types::Schema;
pub type TableRows = crate::api::types::QueryResult;
pub type LogSnapshot = Vec<crate::api::types::LogEntry>;
pub type LiveClientSnapshot = Vec<LiveClientEntry>;
pub type MetricSnapshot = MetricsSnapshot;

// ---------------------------------------------------------------------------
// Moved types (formerly in app_state.rs)
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

/// A connected client entry from the Live tab.
#[derive(Debug, Clone)]
pub struct LiveClientEntry {
    /// Hex identity or connection id (whichever the server returned).
    pub identity: String,
    /// When the client first connected (best-effort from `st_client`).
    pub connected_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// LoadState
// ---------------------------------------------------------------------------

/// Scoped load lifecycle for a single resource.
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    Idle,
    Loading {
        request: RequestContext,
    },
    Ready {
        data: T,
        refreshed_at: Instant,
    },
    Refreshing {
        data: T,
        request: RequestContext,
    },
    Empty {
        refreshed_at: Instant,
    },
    Stale {
        data: T,
        reason: StaleReason,
    },
    Error {
        previous: Option<T>,
        error: AppError,
    },
}

impl<T> Default for LoadState<T> {
    #[allow(clippy::derivable_impls)]
    fn default() -> Self {
        Self::Idle
    }
}

impl<T: Clone> LoadState<T> {
    /// Transition to a loading/refreshing state for a new request.
    /// Existing data remains visible during refresh.
    pub fn begin_read(self, request: RequestContext) -> Self {
        match self {
            LoadState::Ready { data, .. }
            | LoadState::Refreshing { data, .. }
            | LoadState::Stale { data, .. } => LoadState::Refreshing { data, request },
            LoadState::Error {
                previous: Some(data),
                ..
            } => LoadState::Refreshing { data, request },
            _ => LoadState::Loading { request },
        }
    }

    /// Apply a successful read result. Only accepted if `completed` matches
    /// the currently loading/refreshing request context.
    pub fn apply_success(self, completed: &RequestContext, data: T, refreshed_at: Instant) -> Self {
        match &self {
            LoadState::Loading { request } | LoadState::Refreshing { request, .. }
                if request == completed =>
            {
                LoadState::Ready { data, refreshed_at }
            }
            _ => self,
        }
    }

    /// Apply an empty result (zero rows). Same staleness guard as `apply_success`.
    pub fn apply_empty(self, completed: &RequestContext, refreshed_at: Instant) -> Self {
        match &self {
            LoadState::Loading { request } | LoadState::Refreshing { request, .. }
                if request == completed =>
            {
                LoadState::Empty { refreshed_at }
            }
            _ => self,
        }
    }

    /// Apply an error. Preserves previous data if we were refreshing.
    pub fn apply_error(self, completed: &RequestContext, error: AppError) -> Self {
        match self {
            LoadState::Loading { request } if &request == completed => LoadState::Error {
                previous: None,
                error,
            },
            LoadState::Refreshing { data, request } if &request == completed => LoadState::Error {
                previous: Some(data),
                error,
            },
            other => other,
        }
    }
}

/// Why data became stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    Offline,
    NewerGeneration,
    BudgetExceeded,
    ManualRefreshRequired,
}

/// A user-visible error attached to a failed load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub message: String,
}

// ---------------------------------------------------------------------------
// ResourceState
// ---------------------------------------------------------------------------

/// All scoped resource data owned by the application.
#[derive(Debug, Clone, Default)]
pub struct ResourceState {
    pub catalog: LoadState<CatalogSnapshot>,
    pub schema: LoadState<SchemaSnapshot>,
    pub table_rows: LoadState<TableRows>,
    pub logs: LoadState<LogSnapshot>,
    pub metrics: LoadState<MetricSnapshot>,
    pub live_clients: LoadState<LiveClientSnapshot>,
}

// ---------------------------------------------------------------------------
// RequestTracker
// ---------------------------------------------------------------------------

/// Per-scope generation tracker with a global unique id counter.
#[derive(Debug, Clone, Default)]
pub struct RequestTracker {
    next_id: u64,
    next_generation_by_scope: HashMap<RequestScope, u64>,
    latest_by_scope: HashMap<RequestScope, RequestContext>,
}

impl RequestTracker {
    /// Issue a fresh [`RequestContext`] for `scope`, incrementing both the
    /// global id counter and the per-scope generation counter.
    pub fn next_context(&mut self, scope: RequestScope) -> RequestContext {
        self.next_id += 1;
        let generation = self
            .next_generation_by_scope
            .entry(scope.clone())
            .or_insert(0);
        *generation += 1;
        let context = RequestContext::new(
            RequestId::from_u64(self.next_id),
            scope.clone(),
            *generation,
        );
        self.latest_by_scope.insert(scope, context.clone());
        context
    }

    /// Return `true` if `context` is the most recently issued context for its
    /// scope.
    pub fn is_current(&self, context: &RequestContext) -> bool {
        self.latest_by_scope.get(context.scope()) == Some(context)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod load_state_tests {
    use super::*;

    #[test]
    fn refresh_keeps_existing_data_visible() {
        let data = vec!["row1".to_string()];
        let ready: LoadState<Vec<String>> = LoadState::Ready {
            data: data.clone(),
            refreshed_at: Instant::now(),
        };

        let scope = RequestScope::DatabaseCatalog;
        let ctx = RequestContext::new(RequestId::from_u64(1), scope, 1);
        let refreshing = ready.begin_read(ctx.clone());

        match &refreshing {
            LoadState::Refreshing { data, request } => {
                assert_eq!(data, &vec!["row1".to_string()]);
                assert_eq!(request, &ctx);
            }
            _ => panic!("expected Refreshing"),
        }
    }

    #[test]
    fn stale_completion_does_not_replace_newer_generation() {
        let mut tracker = RequestTracker::default();
        let scope = RequestScope::DatabaseCatalog;

        let first = tracker.next_context(scope.clone());
        let second = tracker.next_context(scope.clone());

        assert!(!tracker.is_current(&first));
        assert!(tracker.is_current(&second));
        assert_eq!(first.generation, 1);
        assert_eq!(second.generation, 2);
    }

    #[test]
    fn refresh_keeps_existing_typed_query_result_visible() {
        let request = RequestContext::new(
            RequestId::from_u64(1),
            RequestScope::TableRows {
                database: "inventory".into(),
                table: "items".into(),
                view: "browse".into(),
            },
            1,
        );
        let state = LoadState::Ready {
            data: crate::api::types::QueryResult {
                schema: vec![],
                rows: vec![],
                total_duration_micros: 0,
            },
            refreshed_at: Instant::now(),
        };

        let next = state.begin_read(request.clone());

        assert!(matches!(next, LoadState::Refreshing { request: actual, .. } if actual == request));
    }

    #[test]
    fn stale_completion_does_not_replace_newer_request_context() {
        let current = RequestContext::new(
            RequestId::from_u64(8),
            RequestScope::TableRows {
                database: "inventory".into(),
                table: "items".into(),
                view: "browse".into(),
            },
            3,
        );
        let stale = RequestContext::new(
            RequestId::from_u64(7),
            RequestScope::TableRows {
                database: "inventory".into(),
                table: "items".into(),
                view: "browse".into(),
            },
            2,
        );
        let state = LoadState::<TableRows>::Loading {
            request: current.clone(),
        };

        let next = state.apply_success(
            &stale,
            crate::api::types::QueryResult {
                schema: vec![],
                rows: vec![],
                total_duration_micros: 0,
            },
            Instant::now(),
        );

        assert!(matches!(next, LoadState::Loading { request } if request == current));
    }
}
