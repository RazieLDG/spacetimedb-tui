//! Workbench state: mode, workspace, focused pane, and widget-level UI state.
//!
//! This is the single owner for Phase 2/3 workbench concepts. Navigation and
//! resource data live in sibling modules; this module owns only interaction
//! state.

/// Top-level workbench mode (Phase 3 navigation shell).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkbenchMode {
    #[default]
    Data,
    Observe,
    Operate,
}

/// Active workspace within the current mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Workspace {
    #[default]
    Tables,
    Sql,
    Logs,
    Metrics,
    Module,
    Live,
}

/// Which pane currently owns keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkbenchPane {
    #[default]
    Explorer,
    Workspace,
    Inspector,
    Activity,
}

/// Central workbench interaction state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkbenchState {
    pub mode: WorkbenchMode,
    pub workspace: Workspace,
    pub focused_pane: WorkbenchPane,
    pub sql_input: String,
    pub sql_history_cursor: Option<usize>,
    pub table_grid_cursor: Option<(usize, usize)>,
    pub table_search: String,
    pub palette_open: bool,
    pub help_open: bool,
    pub modal_open: bool,
}
