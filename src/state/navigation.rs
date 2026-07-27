//! Navigation state: active database, resource, and selection identity.
//!
//! This module owns only navigation references. Mode, workspace, and focused
//! pane live in [`crate::state::workbench`].

/// Semantic navigation state for the workbench.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigationState {
    pub active_database: Option<String>,
    pub active_resource: Option<String>,
    pub selection_identity: Option<String>,
}
