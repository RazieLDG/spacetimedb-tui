// Foundation: wired into production event loop in a future milestone.
#![allow(dead_code)]

//! Workbench state: mode, workspace, focused pane, and widget-level UI state.
//!
//! This is the single owner for workbench concepts. Navigation and
//! resource data live in sibling modules; this module owns only interaction
//! state.

/// Top-level workbench mode (navigation shell).
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

impl WorkbenchState {
    pub fn set_sql_input(&mut self, sql: String) {
        self.sql_input = sql;
    }

    pub fn set_grid_cursor(&mut self, row: usize, column: usize) {
        self.table_grid_cursor = Some((row, column));
    }

    pub fn open_palette(&mut self) {
        self.palette_open = true;
    }

    pub fn open_help(&mut self) {
        self.help_open = true;
    }
}

#[cfg(test)]
mod workbench_state_tests {
    use super::*;

    #[test]
    fn sql_input_grid_cursor_palette_and_help_are_owned_by_workbench_state() {
        let mut state = WorkbenchState::default();

        state.set_sql_input("select * from items".to_string());
        state.set_grid_cursor(3, 2);
        state.open_palette();
        state.open_help();

        assert_eq!(state.sql_input, "select * from items");
        assert_eq!(state.table_grid_cursor, Some((3, 2)));
        assert!(state.palette_open);
        assert!(state.help_open);
    }
}
