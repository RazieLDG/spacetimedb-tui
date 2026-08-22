//! Keyboard input dispatch: routes key events to tab/modal/palette handlers.

use super::*;

impl super::App {
    // ── Key dispatch ──────────────────────────────────────────────────────

    /// Dispatch a keyboard event to the appropriate handler.
    ///
    /// Uses explicit `return` statements to make early-exit control flow clear.
    #[allow(clippy::needless_return)]
    pub(crate) async fn handle_key(&mut self, key: KeyEvent) {
        // Windows emits Press + Repeat + Release for every key. Handling
        // Release (and treating it like Press) made j/k and arrows skip
        // two or three sidebar rows per tap.
        if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return;
        }

        // ── Command palette intercept ─────────────────────────────────────
        // The palette owns every key while it's open. Ctrl+C still quits.
        if self.state.palette.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.state.should_quit = true;
                return;
            }
            self.handle_palette_key(key).await;
            return;
        }

        // ── Modal dialog intercept ────────────────────────────────────────
        // When a confirm prompt or form is open, the modal owns every
        // key. Ctrl+C still quits as a panic-button escape hatch.
        if self.state.modal.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('c'))
            {
                self.state.should_quit = true;
                return;
            }
            self.handle_modal_key(key).await;
            return;
        }

        // ── Spreadsheet edit-mode intercept ───────────────────────────────
        // When edit mode is active on the Tables tab, the whole key map
        // changes (cell cursor / inline editor / save / revert). We still
        // honour Ctrl+C as an escape hatch and Ctrl+E to toggle off.
        if self.state.edit_mode.is_some()
            && self.state.focus == FocusPanel::Main
            && self.state.current_tab == Tab::Tables
        {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                match key.code {
                    KeyCode::Char('c') => {
                        self.state.should_quit = true;
                        return;
                    }
                    KeyCode::Char('e') => {
                        self.exit_edit_mode().await;
                        return;
                    }
                    _ => {}
                }
            }
            self.handle_edit_mode_key(key).await;
            return;
        }

        // ── Global always-active bindings ─────────────────────────────────
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.state.should_quit = true;
                    return;
                }
                KeyCode::Char('r') => {
                    // Force a fresh WebSocket connection (e.g. after a server bounce).
                    self.connect_ws().await;
                    self.state
                        .set_notification("Reconnecting WebSocket…".to_string());
                    return;
                }
                KeyCode::Char('e')
                    if self.state.focus == FocusPanel::Main
                        && self.state.current_tab == Tab::Tables =>
                {
                    // Ctrl+E on the Tables tab enters spreadsheet edit
                    // mode. Only fires when we're *not* already in it —
                    // the intercept above catches the toggle-off case.
                    self.enter_edit_mode();
                    return;
                }
                KeyCode::Char('p') => {
                    // Open the command palette.
                    self.state.palette = Some(crate::state::palette::CommandPalette::new());
                    return;
                }
                KeyCode::Char('a') | KeyCode::Home if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.home();
                    return;
                }
                KeyCode::Char('e') | KeyCode::End if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.end();
                    return;
                }
                KeyCode::Char('k') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.kill_to_end();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('u') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.kill_to_start();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('l') if self.state.focus == FocusPanel::SqlInput => {
                    self.sql_input.clear();
                    self.state.history_cursor = None;
                    return;
                }
                KeyCode::Char('l') => {
                    self.toggle_live().await;
                    return;
                }
                KeyCode::Char('f')
                    if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                        && self.state.focus == FocusPanel::Main =>
                {
                    // Ctrl+F opens the grid search prompt.
                    self.state.grid_search = Some(String::new());
                    self.state.grid_search_editing = true;
                    return;
                }
                KeyCode::Char('w') if self.state.focus == FocusPanel::SqlInput => {
                    // Delete the previous word (Ctrl+W, classic Unix convention).
                    let before = &self.sql_input.value[..self.sql_input.cursor];
                    let trimmed_end = before.trim_end_matches(|c: char| c.is_whitespace());
                    let word_start = trimmed_end
                        .rfind(|c: char| c.is_whitespace() || !(c.is_alphanumeric() || c == '_'))
                        .map(|i| i + 1)
                        .unwrap_or(0);
                    let range = word_start..self.sql_input.cursor;
                    if !range.is_empty() {
                        self.sql_input.replace_range(range, "");
                        self.state.history_cursor = None;
                    }
                    return;
                }
                _ => {}
            }
        }

        // ── Help overlay ──────────────────────────────────────────────────
        if self.state.show_help {
            // Clamp the scroll offset to the actual number of lines so a
            // user mashing `↓` doesn't push the value into the millions
            // (and then have to bash `↑` for ages to recover).
            let max_scroll =
                crate::ui::components::help::HelpOverlay::total_lines().saturating_sub(1);
            match key.code {
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => {
                    self.state.show_help = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.state.help_scroll =
                        self.state.help_scroll.saturating_add(1).min(max_scroll);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.state.help_scroll = self.state.help_scroll.saturating_sub(1);
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    self.state.help_scroll = 0;
                }
                KeyCode::End | KeyCode::Char('G') => {
                    self.state.help_scroll = max_scroll;
                }
                _ => {}
            }
            return;
        }

        // ── Error popup — only Esc / Enter dismiss it so accidental keys
        // don't silently swallow the message before the user has read it.
        if self.state.error_message.is_some() {
            if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                self.state.clear_error();
            }
            return;
        }

        // ── Grid search prompt mode ───────────────────────────────────────
        // When Ctrl+F is active on a data-grid tab, we intercept every key
        // for the search buffer instead of running the regular bindings.
        if self.state.grid_search_editing {
            match key.code {
                KeyCode::Esc => {
                    // Cancel search entirely — clears the highlight.
                    self.state.grid_search = None;
                    self.state.grid_search_editing = false;
                }
                KeyCode::Enter => {
                    // Commit the query; highlights stay and n/N navigate.
                    self.state.grid_search_editing = false;
                    self.jump_to_next_match(true);
                }
                KeyCode::Backspace => {
                    if let Some(q) = self.state.grid_search.as_mut() {
                        q.pop();
                    }
                }
                KeyCode::Char(ch) => {
                    if let Some(q) = self.state.grid_search.as_mut() {
                        q.push(ch);
                    }
                }
                _ => {}
            }
            return;
        }

        // ── SQL input mode ────────────────────────────────────────────────
        if self.state.focus == FocusPanel::SqlInput {
            match key.code {
                KeyCode::Esc => {
                    self.state.focus = FocusPanel::Main;
                }
                KeyCode::Enter => {
                    self.execute_sql().await;
                }
                KeyCode::Tab => {
                    self.complete_sql_input();
                }
                KeyCode::Up if self.state.history_prev() => {
                    if let Some(sql) = self.state.current_history_sql() {
                        self.sql_input.set(sql.to_string());
                    }
                }
                KeyCode::Down => match self.state.history_next() {
                    HistoryAdvance::Moved => {
                        if let Some(sql) = self.state.current_history_sql() {
                            self.sql_input.set(sql.to_string());
                        }
                    }
                    HistoryAdvance::Cleared => {
                        self.sql_input.clear();
                    }
                    HistoryAdvance::Unchanged => {}
                },
                KeyCode::Left => self.sql_input.move_left(),
                KeyCode::Right => self.sql_input.move_right(),
                KeyCode::Home => self.sql_input.home(),
                KeyCode::End => self.sql_input.end(),
                KeyCode::Backspace => {
                    self.sql_input.backspace();
                    self.state.history_cursor = None;
                }
                KeyCode::Delete => {
                    self.sql_input.delete();
                    self.state.history_cursor = None;
                }
                KeyCode::Char(ch) => {
                    self.sql_input.insert(ch);
                    // Any edit drops the user out of "browsing history"
                    // mode so ↓ no longer snaps back to the old entry.
                    self.state.history_cursor = None;
                }
                _ => {}
            }
            return;
        }

        // ── Global bindings (not in SQL input mode) ───────────────────────
        match key.code {
            // Quit
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.state.should_quit = true;
                return;
            }

            // Help overlay
            KeyCode::Char('?') => {
                self.state.show_help = !self.state.show_help;
                self.state.help_scroll = 0;
                return;
            }

            // Tab cycling
            KeyCode::Tab => {
                self.state.current_tab = self.state.current_tab.next();
                self.state.focus = FocusPanel::Main;
                return;
            }
            KeyCode::BackTab => {
                self.state.current_tab = self.state.current_tab.prev();
                self.state.focus = FocusPanel::Main;
                return;
            }

            // Direct tab jump
            KeyCode::Char('1') => {
                self.state.current_tab = Tab::Tables;
                return;
            }
            KeyCode::Char('2') => {
                self.state.current_tab = Tab::Sql;
                return;
            }
            KeyCode::Char('3') => {
                self.state.current_tab = Tab::Logs;
                return;
            }
            KeyCode::Char('4') => {
                self.state.current_tab = Tab::Metrics;
                return;
            }
            KeyCode::Char('5') => {
                self.state.current_tab = Tab::Module;
                return;
            }
            KeyCode::Char('6') => {
                self.state.current_tab = Tab::Live;
                return;
            }

            // Sidebar focus: h/← steps up Tables → Databases; l/→ opens the
            // highlighted item (database → its tables, table → load + main).
            KeyCode::Left | KeyCode::Char('h') if self.state.focus == FocusPanel::Sidebar => {
                if self.state.sidebar_focus == SidebarFocus::Tables {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                }
                return;
            }
            KeyCode::Right | KeyCode::Char('l') if self.state.focus == FocusPanel::Sidebar => {
                self.nav_enter().await;
                return;
            }

            // Main focus: h/← and l/→ move the cell cursor inside a data
            // grid (Tables or SQL tabs). Use Esc to drop back to sidebar.
            KeyCode::Left | KeyCode::Char('h')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.prev_col();
                return;
            }
            KeyCode::Right | KeyCode::Char('l')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let col_count = if self.state.current_tab == Tab::Tables {
                    self.state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0)
                } else {
                    self.state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0)
                };
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.next_col(col_count);
                return;
            }

            // Enter SQL mode
            KeyCode::Char(':') => {
                self.state.current_tab = Tab::Sql;
                self.state.focus = FocusPanel::SqlInput;
                return;
            }

            // Search / filter (sidebar)
            KeyCode::Char('/') => {
                // Toggle search mode — simple: enter a char into search_query
                if self.state.search_query.is_empty() {
                    self.state.focus = FocusPanel::Sidebar;
                } else {
                    self.state.search_query.clear();
                }
                return;
            }

            // Refresh current view
            KeyCode::Char('r') => {
                self.refresh_current_view().await;
                return;
            }

            // Navigation — delegate to focus owner
            KeyCode::Char('j') | KeyCode::Down => {
                self.nav_down().await;
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.nav_up().await;
                return;
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.nav_home().await;
                return;
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.nav_end().await;
                return;
            }

            // Enter / select. On the Tables tab with main focus,
            // Enter opens the edit popup for the selected row —
            // the most discoverable path to row editing.
            KeyCode::Enter
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_update_form();
                return;
            }

            // Enter / select
            KeyCode::Enter => {
                self.nav_enter().await;
                return;
            }

            // Escape — multi-level "go back":
            //   1. clear an active search-as-you-type query, else
            //   2. if sidebar focus is on Tables, step back up to Databases, else
            //   3. snap keyboard focus from the main pane back to the sidebar.
            KeyCode::Esc => {
                if !self.state.search_query.is_empty() {
                    self.state.search_query.clear();
                } else if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Tables
                {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                } else {
                    self.state.focus = FocusPanel::Sidebar;
                }
                return;
            }

            // Clipboard — `y` yanks the currently selected cell, `Y`
            // yanks the whole row (TSV-joined). Works on the data-grid
            // tabs (Tables / SQL) when focus is in the main pane.
            KeyCode::Char('y')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.copy_selected_cell();
                return;
            }
            KeyCode::Char('Y')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.copy_selected_row();
                return;
            }

            // Insert row (Tables tab only). Opens a form prefilled
            // with the current schema's columns. Submit issues an
            // INSERT INTO ... VALUES (...) SQL statement.
            KeyCode::Char('i')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_insert_form();
                return;
            }

            // Delete row (Tables tab only). Opens a y/n confirm dialog
            // that issues a DELETE FROM ... WHERE pk = ... statement.
            // PK is heuristically the first column of the table.
            KeyCode::Char('d')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_delete_confirm();
                return;
            }

            // Delete key is a discoverable alias for `d`.
            KeyCode::Delete
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_delete_confirm();
                return;
            }

            // Update row (Tables tab only). Opens an edit form
            // prefilled with the current row's values. The first
            // field is the PK display; submit issues an UPDATE.
            KeyCode::Char('U')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_update_form();
                return;
            }

            // ── Destructive admin ops (typed-confirm forms) ───────
            // Shift+D on the Tables tab → truncate the selected
            // table (DELETE FROM <table>). Same key on the sidebar
            // when focused on the Databases panel → delete the
            // entire database via DELETE /v1/database/<name>.
            KeyCode::Char('D')
                if self.state.focus == FocusPanel::Main
                    && self.state.current_tab == Tab::Tables =>
            {
                self.open_truncate_table_form();
                return;
            }
            KeyCode::Char('D')
                if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Databases =>
            {
                self.open_delete_db_form();
                return;
            }

            // `a` on the Databases sidebar panel opens a form to
            // attach a new alias (human name) to the selected DB.
            // Non-destructive, no typed-confirm required.
            KeyCode::Char('a')
                if self.state.focus == FocusPanel::Sidebar
                    && self.state.sidebar_focus == SidebarFocus::Databases =>
            {
                self.open_add_alias_form();
                return;
            }

            // Sort — `s` cycles the sort state (off → asc → desc → off)
            // on the currently-selected column.
            KeyCode::Char('s')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let col_count = self
                    .active_grid()
                    .map(|(qr, _)| qr.column_count())
                    .unwrap_or(0);
                if col_count == 0 {
                    return;
                }
                // Snapshot the underlying data row the cursor points at
                // *before* mutating the sort state — we'll translate it
                // back into a display row after the permutation changes
                // so the cursor appears to stay on the "same" record
                // instead of jumping to a random position.
                let anchor_data_row = self.active_data_row_index();
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.cycle_sort(grid.selected_col);

                // Re-map the anchor through the new permutation. If
                // anything goes sideways (empty rows, missing anchor)
                // the cursor stays where it was — no worse than the
                // old behaviour.
                if let Some(data_idx) = anchor_data_row {
                    if let Some(new_display) = self.display_row_for_data_idx(data_idx) {
                        let grid = if self.state.current_tab == Tab::Tables {
                            &mut self.tables_grid
                        } else {
                            &mut self.sql_grid
                        };
                        grid.selected_row = new_display;
                    }
                }
                let col_name = self
                    .active_grid()
                    .and_then(|(qr, g)| {
                        qr.column_names().get(g.selected_col).map(|s| s.to_string())
                    })
                    .unwrap_or_default();
                let dir = match (
                    self.active_grid().map(|(_, g)| g.sort_col),
                    self.active_grid().map(|(_, g)| g.sort_desc),
                ) {
                    (Some(Some(_)), Some(false)) => "asc",
                    (Some(Some(_)), Some(true)) => "desc",
                    _ => "off",
                };
                self.state
                    .set_notification(format!("Sort {col_name} {dir}"));
                return;
            }

            // Export — `e` writes a CSV, `E` writes a JSON file under
            // `./exports/` for the currently visible query result.
            KeyCode::Char('e')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.export_current_result(crate::ui::export::ExportFormat::Csv);
                return;
            }
            KeyCode::Char('E')
                if self.state.focus == FocusPanel::Main
                    && matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                self.export_current_result(crate::ui::export::ExportFormat::Json);
                return;
            }

            // Log-specific
            KeyCode::Char(' ') if self.state.current_tab == Tab::Logs => {
                self.state.log_follow = !self.state.log_follow;
                return;
            }
            KeyCode::Char('c') if self.state.current_tab == Tab::Logs => {
                self.state.log_buffer.clear();
                self.state.log_scroll = 0;
                return;
            }
            KeyCode::Char('f') if self.state.current_tab == Tab::Logs => {
                self.state.log_filter_level = self.state.log_filter_level.clone().next_filter();
                self.state
                    .set_notification(format!("Log filter: {}", self.state.log_filter_level));
                return;
            }

            // `n` / `N` on data-grid tabs: jump to next / previous
            // search match when a search query is active, otherwise
            // fall through to page-scroll on the Tables tab.
            KeyCode::Char('n')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                    && self.state.grid_search.is_some() =>
            {
                self.jump_to_next_match(true);
                return;
            }
            KeyCode::Char('N')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql)
                    && self.state.grid_search.is_some() =>
            {
                self.jump_to_next_match(false);
                return;
            }

            // Server-side pagination on the Tables tab: `,` / `.` (and
            // PageUp / PageDown) move the browse window one page of
            // BROWSE_PAGE_SIZE rows forward or back.
            KeyCode::Char(',') if self.state.current_tab == Tab::Tables => {
                self.browse_page_prev().await;
                return;
            }
            KeyCode::Char('.') if self.state.current_tab == Tab::Tables => {
                self.browse_page_next().await;
                return;
            }
            KeyCode::PageUp if self.state.current_tab == Tab::Tables => {
                self.browse_page_prev().await;
                return;
            }
            KeyCode::PageDown if self.state.current_tab == Tab::Tables => {
                self.browse_page_next().await;
                return;
            }

            // Page navigation in tables (no active search)
            KeyCode::Char('n') if self.state.current_tab == Tab::Tables => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_add(20);
                return;
            }
            KeyCode::Char('p') if self.state.current_tab == Tab::Tables => {
                self.tables_grid.scroll_row = self.tables_grid.scroll_row.saturating_sub(20);
                return;
            }

            // Horizontal scroll in table/SQL results (< / > or H / L)
            KeyCode::Char('<') | KeyCode::Char('H')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let grid = if self.state.current_tab == Tab::Tables {
                    &mut self.tables_grid
                } else {
                    &mut self.sql_grid
                };
                grid.scroll_left();
                return;
            }
            KeyCode::Char('>') | KeyCode::Char('L')
                if matches!(self.state.current_tab, Tab::Tables | Tab::Sql) =>
            {
                let (col_count, grid) = if self.state.current_tab == Tab::Tables {
                    let cc = self
                        .state
                        .table_browse_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0);
                    (cc, &mut self.tables_grid)
                } else {
                    let cc = self
                        .state
                        .query_result
                        .as_ref()
                        .map(|qr| qr.column_count())
                        .unwrap_or(0);
                    (cc, &mut self.sql_grid)
                };
                grid.scroll_right(col_count);
                return;
            }

            // Search input (when in sidebar search mode) — also acts as
            // "step up" when there's no search text and the user is on the
            // Tables sub-panel.
            KeyCode::Backspace if self.state.focus == FocusPanel::Sidebar => {
                if !self.state.search_query.is_empty() {
                    self.state.search_query.pop();
                } else if self.state.sidebar_focus == SidebarFocus::Tables {
                    self.state.sidebar_focus = SidebarFocus::Databases;
                }
                return;
            }
            KeyCode::Char(ch)
                if self.state.focus == FocusPanel::Sidebar && !ch.is_ascii_control() =>
            {
                // In sidebar, typing filters the list
                self.state.search_query.push(ch);
                return;
            }

            _ => {}
        }
    }
}
