// Foundation: wired into production event loop in a future milestone.
#![allow(dead_code)]

//! Activity state: active work, notices, connection lifecycle, retry.

use crate::effects::request::RequestId;

/// Connection lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

/// Retry countdown state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetryState {
    pub attempt: u32,
    pub next_retry_label: Option<String>,
}

/// Central activity state: active reads/mutations, notices, connection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityState {
    pub active_reads: Vec<RequestId>,
    pub active_mutations: Vec<RequestId>,
    pub notices: Vec<String>,
    pub connection: ConnectionState,
    pub retry: RetryState,
}

impl ActivityState {
    pub fn active_task_count(&self) -> usize {
        self.active_reads.len() + self.active_mutations.len()
    }
}
