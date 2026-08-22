//! Semantic navigation actions shared by key bindings and commands.

use super::*;

impl super::App {
    // ── Navigation helpers ────────────────────────────────────────────────

    pub(crate) async fn nav_sidebar_delta(&mut self, delta: i32) {
        let items = nav_items(&self.state);
        if items.is_empty() {
            return;
        }
        let current = current_nav_index(&self.state, &items).unwrap_or(0);
        let next = if delta > 0 {
            current.saturating_add(1).min(items.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        if current_nav_index(&self.state, &items) == Some(next) {
            return;
        }
        self.apply_sidebar_nav_item(items[next]).await;
    }

    pub(crate) async fn apply_sidebar_nav_item(&mut self, item: SidebarNavItem) {
        match item {
            SidebarNavItem::Database(idx) => {
                let changed = self.state.selected_database_idx != Some(idx);
                self.state.sidebar_focus = SidebarFocus::Databases;
                if changed {
                    self.state.select_database(idx);
                    self.bump_database_generation();
                    self.load_schema().await;
                }
            }
            SidebarNavItem::Table(idx) => {
                if idx >= self.state.tables.len() {
                    return;
                }
                let changed = self.state.selected_table_idx != Some(idx);
                self.state.sidebar_focus = SidebarFocus::Tables;
                if changed {
                    self.state.selected_table_idx = Some(idx);
                    self.bump_table_generation();
                    self.clear_table_browse_for_selection_change();
                }
                if changed || self.state.table_browse_result.is_none() {
                    self.load_table_data(TableBrowseOrigin::Navigation).await;
                    self.ensure_live_subscription().await;
                }
            }
        }
    }

    pub(crate) async fn nav_down(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                self.nav_sidebar_delta(1).await;
            }
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    let row_count = self
                        .state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.row_count())
                        .unwrap_or(0);
                    self.tables_grid.next_row(row_count);
                }
                Tab::Sql => {
                    let row_count = self
                        .state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.row_count())
                        .unwrap_or(0);
                    self.sql_grid.next_row(row_count);
                }
                Tab::Logs if !self.state.log_follow => {
                    self.state.log_scroll = self
                        .state
                        .log_scroll
                        .saturating_add(1)
                        .min(self.state.log_buffer.len().saturating_sub(1));
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

    pub(crate) async fn nav_up(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                self.nav_sidebar_delta(-1).await;
            }
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    self.tables_grid.prev_row();
                }
                Tab::Sql => {
                    self.sql_grid.prev_row();
                }
                Tab::Logs if !self.state.log_follow => {
                    self.state.log_scroll = self.state.log_scroll.saturating_sub(1);
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

    pub(crate) async fn nav_home(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                let items = nav_items(&self.state);
                if let Some(item) = items.first().copied() {
                    self.apply_sidebar_nav_item(item).await;
                }
            }
            FocusPanel::Main => match self.state.current_tab {
                Tab::Tables => {
                    self.tables_grid.selected_row = 0;
                    self.tables_grid.scroll_row = 0;
                }
                Tab::Sql => {
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

    pub(crate) async fn nav_end(&mut self) {
        if self.state.focus == FocusPanel::Sidebar {
            let items = nav_items(&self.state);
            if let Some(item) = items.last().copied() {
                self.apply_sidebar_nav_item(item).await;
            }
            return;
        }
        if self.state.focus == FocusPanel::Main {
            match self.state.current_tab {
                Tab::Tables => {
                    if let Some(ref qr) = self.state.table_browse_result {
                        let count = qr.row_count();
                        self.tables_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Sql => {
                    if let Some(ref qr) = self.state.query_result {
                        let count = qr.row_count();
                        self.sql_grid.selected_row = count.saturating_sub(1);
                    }
                }
                Tab::Logs => {
                    self.state.log_follow = true;
                }
                _ => {}
            }
        }
    }

    pub(crate) async fn nav_enter(&mut self) {
        match self.state.focus {
            FocusPanel::Sidebar => {
                match self.state.sidebar_focus {
                    SidebarFocus::Databases => {
                        // Descend into the expanded database's tables.
                        self.state.sidebar_focus = SidebarFocus::Tables;
                        if self.state.selected_table_idx.is_none() {
                            self.state.selected_table_idx = self.state.preferred_table_index(None);
                        }
                        if self.state.selected_table_idx.is_some()
                            && self.state.table_browse_result.is_none()
                        {
                            self.load_table_data(TableBrowseOrigin::Navigation).await;
                            self.ensure_live_subscription().await;
                        }
                    }
                    SidebarFocus::Tables => {
                        self.load_table_data(TableBrowseOrigin::Navigation).await;
                        self.ensure_live_subscription().await;
                        self.state.focus = FocusPanel::Main;
                        self.state.current_tab = Tab::Tables;
                        self.tables_grid = TableGridState::new();
                    }
                }
            }
            FocusPanel::Main => {
                if self.state.current_tab == Tab::Sql {
                    self.state.focus = FocusPanel::SqlInput;
                } else if self.state.current_tab == Tab::Module {
                    // Enter on a reducer in the module inspector opens
                    // a call form (or a no-arg confirm, when the
                    // reducer has no parameters).
                    self.open_reducer_form();
                }
            }
            _ => {}
        }
    }
}
