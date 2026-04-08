//! Application state module.
//!
//! The [`app_state`] sub-module contains [`AppState`], the single source of
//! truth for all TUI state.  Import from here rather than from the sub-module
//! directly.

pub mod app_state;

// Re-export the most commonly used items.
pub use app_state::{
    AppState, ConnectionStatus, FocusPanel, HistoryAdvance, MetricsSnapshot, SidebarFocus,
    SqlHistoryEntry, Tab,
};
