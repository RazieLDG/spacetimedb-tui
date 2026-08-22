//! Table data lifecycle: schema/table-browse loads, grid accessors,
//! clipboard copy helpers, in-row search, and export dispatch.

use super::*;

impl super::App {
    // ── Data loading ──────────────────────────────────────────────────────

    pub(crate) async fn load_schema(&mut self) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        self.bump_schema_generation();
        self.state.tables.clear();
        self.state.selected_table_idx = None;
        self.state.current_schema = None;
        self.clear_table_browse_for_selection_change();
        // Track the in-flight schema fetch so the sidebar can show
        // a real loading spinner and clear it on both success and
        // failure (fixes the "stuck on (loading…)" bug after HTTP
        // 500s).
        self.state.schema_loading = true;
        self.state.schema_load_failed = false;

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.current_schema_request_context(db.clone());
        self.task_registry.spawn("load schema", async move {
            match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.get_schema(&db)).await {
                Ok(Ok(schema)) => send_event(
                    &tx,
                    AppEvent::SchemaLoaded {
                        context: context.clone(),
                        schema,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::SchemaError {
                        context: context.clone(),
                        error: format!("Schema load failed: {e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::SchemaError {
                        context: context.clone(),
                        error: "Schema load timed out".to_string(),
                    },
                ),
            }
        });
    }

    pub(crate) async fn load_table_data(&mut self, origin: TableBrowseOrigin) {
        let db = match self.state.selected_database() {
            Some(d) => d.to_string(),
            None => return,
        };
        let table = match self.state.selected_table() {
            Some(t) => t.table_name.clone(),
            None => return,
        };

        self.bump_table_generation();
        self.state.query_loading = true;
        self.state.table_browse_result = None;

        let offset = self.state.browse_offset;
        // Prefix fetch: LIMIT grows with the page depth (see
        // `table_browse_sql`). The visible slice is cut out later, when
        // the result lands.
        let sql = match table_browse_sql(&table, offset) {
            Ok(sql) => sql,
            Err(error) => {
                self.state.query_loading = false;
                self.state
                    .set_error(format!("Cannot browse table: {error}"));
                return;
            }
        };
        // Parallel side query for the pager's "of N" display. The
        // aggregate REQUIRES a column alias on SpacetimeDB SQL
        // (`COUNT(*) AS total`), otherwise the server rejects it with
        // HTTP 400. A failure or timeout here never blocks the page
        // itself — total stays unknown and the header shows just the
        // row range.
        let count_sql = match encode_identifier(&table) {
            Ok(id) => format!("SELECT COUNT(*) AS total FROM {id}"),
            Err(_) => String::new(),
        };

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let context = self.current_table_browse_request_context(db.clone(), table.clone(), origin);

        self.task_registry.spawn("browse table", async move {
            let rows_fut = tokio::time::timeout(HTTP_REQUEST_TIMEOUT, client.query_sql(&db, &sql));
            let count_job = async {
                if count_sql.is_empty() {
                    None
                } else {
                    let fut = tokio::time::timeout(
                        HTTP_REQUEST_TIMEOUT,
                        client.query_sql(&db, &count_sql),
                    );
                    fut.await.ok()
                }
            };
            let (rows_res, count_res) = tokio::join!(rows_fut, count_job);
            let total_rows = count_res.and_then(|r| r.ok()).and_then(extract_total_count);
            match rows_res {
                Ok(Ok(result)) => send_event(
                    &tx,
                    AppEvent::TableBrowseResult {
                        context: context.clone(),
                        result,
                        total_rows,
                    },
                ),
                Ok(Err(e)) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        context: context.clone(),
                        error: format!("{e:#}"),
                    },
                ),
                Err(_) => send_event(
                    &tx,
                    AppEvent::TableBrowseError {
                        context: context.clone(),
                        error: "table load timed out".to_string(),
                    },
                ),
            }
        });
    }

    /// Server-side "next page": advance `OFFSET` by one page and reload.
    /// No-op while a load is in flight or when already on the last page
    /// (detected via a short page result or the known total).
    pub(crate) async fn browse_page_next(&mut self) {
        if self.state.query_loading || self.state.table_browse_result.is_none() {
            return;
        }
        if let Some(total) = self.state.browse_total_rows {
            if self.state.browse_offset + BROWSE_PAGE_SIZE >= total {
                self.state
                    .set_notification(format!("Already on the last page ({total} rows)"));
                return;
            }
        } else {
            let short_page = self
                .state
                .table_browse_result
                .as_ref()
                .is_some_and(|qr| (qr.row_count() as u64) < BROWSE_PAGE_SIZE);
            if short_page {
                self.state.set_notification("Already on the last page");
                return;
            }
        }
        self.state.browse_offset += BROWSE_PAGE_SIZE;
        self.load_table_data(TableBrowseOrigin::ManualRefresh).await;
    }

    /// Server-side "previous page". No-op on the first page or while
    /// a load is in flight.
    pub(crate) async fn browse_page_prev(&mut self) {
        if self.state.query_loading || self.state.browse_offset == 0 {
            return;
        }
        self.state.browse_offset = self.state.browse_offset.saturating_sub(BROWSE_PAGE_SIZE);
        self.load_table_data(TableBrowseOrigin::ManualRefresh).await;
    }

    /// Return a reference to the `QueryResult` / `TableGridState` pair
    /// that backs the currently focused data-grid tab, together with
    /// the table-name hint (if any) used for notifications.
    pub(crate) fn active_grid(&self) -> Option<(&crate::api::types::QueryResult, &TableGridState)> {
        match self.state.current_tab {
            Tab::Tables => self
                .state
                .table_browse_result
                .as_ref()
                .map(|qr| (qr, &self.tables_grid)),
            Tab::Sql => self
                .state
                .query_result
                .as_ref()
                .map(|qr| (qr, &self.sql_grid)),
            _ => None,
        }
    }

    /// Translate a grid's `selected_row` (which is in display order
    /// when a sort is active) back to the underlying `QueryResult.rows`
    /// index, so clipboard / export operations read the cells the user
    /// is actually looking at.
    pub(crate) fn active_data_row_index(&self) -> Option<usize> {
        let (qr, grid) = self.active_grid()?;
        // Re-project the rows into the same `Vec<Vec<String>>` that the
        // renderer sorts, then ask `sorted_data_index` for the mapping.
        let string_rows: Vec<Vec<String>> = qr
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(crate::ui::tabs::tables::value_to_display)
                    .collect()
            })
            .collect();
        crate::ui::components::table_grid::sorted_data_index(
            &string_rows,
            grid.sort_col,
            grid.sort_desc,
            grid.selected_row,
        )
    }

    /// Reverse of [`active_data_row_index`]: given an underlying
    /// `data_idx` (stable across sort permutations), return the
    /// display row index it currently lives at under the active
    /// grid's sort state. Used by the `s` key binding to keep the
    /// cell cursor anchored to the same record when the permutation
    /// changes.
    pub(crate) fn display_row_for_data_idx(&self, data_idx: usize) -> Option<usize> {
        let (qr, grid) = self.active_grid()?;
        if qr.rows.is_empty() || data_idx >= qr.rows.len() {
            return None;
        }
        let Some(sort_col) = grid.sort_col else {
            return Some(data_idx);
        };
        let string_rows: Vec<Vec<String>> = qr
            .rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(crate::ui::tabs::tables::value_to_display)
                    .collect()
            })
            .collect();
        // Rebuild the permutation the renderer uses and scan for
        // the display index whose mapped data index matches.
        // O(n²) in the worst case but data grids cap at a few
        // hundred rows — acceptable for a one-shot key event.
        (0..string_rows.len()).find(|&display_idx| {
            crate::ui::components::table_grid::sorted_data_index(
                &string_rows,
                Some(sort_col),
                grid.sort_desc,
                display_idx,
            ) == Some(data_idx)
        })
    }

    /// Copy the currently-highlighted cell to the terminal clipboard.
    pub(crate) fn copy_selected_cell(&mut self) {
        let cell_text = {
            let data_idx = match self.active_data_row_index() {
                Some(i) => i,
                None => return,
            };
            let Some((qr, grid)) = self.active_grid() else {
                return;
            };
            let row = match qr.rows.get(data_idx) {
                Some(r) => r,
                None => return,
            };
            let value = match row.get(grid.selected_col) {
                Some(v) => v,
                None => return,
            };
            crate::ui::tabs::tables::value_to_display(value)
        };

        match crate::ui::clipboard::copy_to_clipboard(&cell_text) {
            Ok(n) => {
                let preview: String = cell_text.chars().take(40).collect();
                self.state
                    .set_notification(format!("Copied {n}B: {preview}"));
            }
            Err(e) => {
                tracing::warn!("clipboard copy failed: {e}");
                self.state.set_error(format!("Clipboard copy failed: {e}"));
            }
        }
    }

    /// Copy the currently-selected row to the terminal clipboard as a
    /// TSV (tab-separated values) line.
    pub(crate) fn copy_selected_row(&mut self) {
        let (row_text, col_count) = {
            let data_idx = match self.active_data_row_index() {
                Some(i) => i,
                None => return,
            };
            let Some((qr, _grid)) = self.active_grid() else {
                return;
            };
            let row = match qr.rows.get(data_idx) {
                Some(r) => r,
                None => return,
            };
            let tsv = row
                .iter()
                .map(crate::ui::tabs::tables::value_to_display)
                .collect::<Vec<_>>()
                .join("\t");
            (tsv, row.len())
        };

        match crate::ui::clipboard::copy_to_clipboard(&row_text) {
            Ok(n) => {
                self.state
                    .set_notification(format!("Copied row ({col_count} cells, {n}B)"));
            }
            Err(e) => {
                tracing::warn!("clipboard copy failed: {e}");
                self.state.set_error(format!("Clipboard copy failed: {e}"));
            }
        }
    }

    /// Move the cell cursor to the next (or previous, if `forward` is
    /// `false`) row that contains a match for the current grid search
    /// query. Wraps around the end of the result set.
    ///
    /// A "match" is any cell whose string representation contains the
    /// query as a case-insensitive substring. Used by Enter on the
    /// search prompt and by `n` / `N` afterwards.
    pub(crate) fn jump_to_next_match(&mut self, forward: bool) {
        let query = match self.state.grid_search.as_ref() {
            Some(q) if !q.is_empty() => q.to_ascii_lowercase(),
            _ => return,
        };

        // Snapshot the rows we're searching so we can release the
        // immutable borrow on `state` before mutating the grid.
        let rows: Vec<Vec<String>> = {
            let qr = match self.state.current_tab {
                Tab::Tables => self.state.table_browse_result.as_ref(),
                Tab::Sql => self.state.query_result.as_ref(),
                _ => return,
            };
            let Some(qr) = qr else {
                return;
            };
            qr.rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(crate::ui::tabs::tables::value_to_display)
                        .collect()
                })
                .collect()
        };

        if rows.is_empty() {
            self.state.set_notification("No rows to search".to_string());
            return;
        }

        let grid = if self.state.current_tab == Tab::Tables {
            &mut self.tables_grid
        } else {
            &mut self.sql_grid
        };

        // Walk display order (which is `rows` when unsorted, or the
        // sort permutation when a sort is active) so `n` / `N`
        // visually steps by one row on screen each time.
        let order: Vec<usize> = match grid.sort_col {
            Some(col) => {
                let mut idxs: Vec<usize> = (0..rows.len()).collect();
                idxs.sort_by(|&a, &b| {
                    let av = rows[a].get(col).map(String::as_str).unwrap_or("");
                    let bv = rows[b].get(col).map(String::as_str).unwrap_or("");
                    // Replicate `compare_cells` locally so we don't have
                    // to expose it outside `table_grid`.
                    match (av.parse::<f64>(), bv.parse::<f64>()) {
                        (Ok(na), Ok(nb)) => {
                            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
                        }
                        _ => av.to_ascii_lowercase().cmp(&bv.to_ascii_lowercase()),
                    }
                });
                if grid.sort_desc {
                    idxs.reverse();
                }
                idxs
            }
            None => (0..rows.len()).collect(),
        };

        let n = order.len();
        let start = grid.selected_row.min(n - 1);
        for step in 1..=n {
            let display_idx = if forward {
                (start + step) % n
            } else {
                (start + n - step) % n
            };
            let data_idx = order[display_idx];
            if rows[data_idx]
                .iter()
                .any(|cell| cell.to_ascii_lowercase().contains(&query))
            {
                grid.selected_row = display_idx;
                return;
            }
        }
        self.state
            .set_notification(format!("No match for \"{query}\""));
    }

    /// Serialise the currently visible query result to CSV or JSON and
    /// write it under `./exports/`. Shows the resulting path in the
    /// status bar notification so the user can `cat` / open it.
    pub(crate) fn export_current_result(&mut self, format: crate::ui::export::ExportFormat) {
        let (qr, label) = match self.state.current_tab {
            Tab::Tables => {
                let qr = match self.state.table_browse_result.as_ref() {
                    Some(qr) => qr,
                    None => {
                        self.state.set_notification("Nothing to export".to_string());
                        return;
                    }
                };
                let label = self
                    .state
                    .selected_table()
                    .map(|t| t.table_name.clone())
                    .unwrap_or_else(|| "table".to_string());
                (qr.clone(), label)
            }
            Tab::Sql => {
                let qr = match self.state.query_result.as_ref() {
                    Some(qr) => qr,
                    None => {
                        self.state.set_notification("Nothing to export".to_string());
                        return;
                    }
                };
                (qr.clone(), "query".to_string())
            }
            _ => return,
        };

        match crate::ui::export::write_export(&qr, format, &label) {
            Ok(path) => {
                self.state
                    .set_notification(format!("Exported to {}", path.display()));
            }
            Err(e) => {
                tracing::warn!("export failed: {e:#}");
                self.state.set_error(format!("Export failed: {e:#}"));
            }
        }
    }
}

/// Pull the scalar out of a `SELECT COUNT(*)` result.
///
/// Tolerates both a numeric cell and a string-encoded number, since
/// server versions have differed on how `U64` aggregates serialise.
fn extract_total_count(result: crate::api::types::QueryResult) -> Option<u64> {
    let cell = result.rows.first()?.first()?;
    match cell {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}
