// Phase 2/3 foundation: wired into production event loop in Phase 3.
#![allow(dead_code)]

//! Normalized actions, events, effects, and transition types.
//!
//! `AppEvent` is the single inbound event type for the reducer. `Effect` is
//! the single outbound side-effect type. `Transition` bundles the effects
//! produced by one reducer step.

use crate::api::types::{LogEntry, QueryResult, Schema};
use crate::app::command::CommandId;
use crate::effects::request::RequestContext;
use crate::state::resources::MetricsSnapshot;

// ---------------------------------------------------------------------------
// Action
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Invoke(CommandId),
    SelectDatabase {
        database: String,
    },
    SelectResource {
        database: String,
        resource: String,
    },
    NavigateUp,
    ConfirmWritePlan {
        plan_id: crate::state::safety::WritePlanId,
    },
    RestoreSession,
}

// ---------------------------------------------------------------------------
// AppEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum AppEvent {
    Action(Action),
    ScopedReadCompleted {
        context: RequestContext,
        result: ReadResult,
    },
    ScopedReadFailed {
        context: RequestContext,
        failure: ReadFailure,
        retry: Option<ReadOperation>,
    },
    Tick,
}

// ---------------------------------------------------------------------------
// Read payloads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ReadResult {
    Catalog(Vec<String>),
    Schema(Schema),
    TableRows(QueryResult),
    Logs(Vec<LogEntry>),
    Metrics(MetricsSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    Transport(String),
}

// ---------------------------------------------------------------------------
// ReadOperation (retry descriptor)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOperation {
    Catalog,
    Schema { database: String },
    TableRows { target: TableTarget },
}

// ---------------------------------------------------------------------------
// TableTarget
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTarget {
    pub database: String,
    pub table: String,
}

// ---------------------------------------------------------------------------
// Effect
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    LoadCatalog {
        context: RequestContext,
    },
    LoadSchema {
        database: String,
        context: RequestContext,
    },
    LoadTableRows {
        target: TableTarget,
        context: RequestContext,
    },
    PersistSession {
        snapshot: crate::user_config::SessionState,
    },
}

// ---------------------------------------------------------------------------
// Transition
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub effects: Vec<Effect>,
}

impl Transition {
    pub fn none() -> Self {
        Self {
            effects: Vec::new(),
        }
    }

    pub fn effects(effects: Vec<Effect>) -> Self {
        Self { effects }
    }
}
