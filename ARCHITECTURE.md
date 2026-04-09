# ARCHITECTURE.md — SpacetimeDB TUI Codebase Audit

> **Audit date:** 2025-01-XX  
> **Codebase:** 24 source files, ~6,997 lines of Rust  
> **Compiler warnings:** 27 (0 errors)  
> **Status:** Compiles, partially functional — multiple bugs in core flows

---

## Table of Contents

1. [Current Architecture & Data Flow](#1-current-architecture--data-flow)
2. [Compiler Warnings — Categorized with Fix Instructions](#2-compiler-warnings--categorized-with-fix-instructions)
3. [Bug Analysis](#3-bug-analysis)
4. [Per-File Fix Plan](#4-per-file-fix-plan)
5. [Enhancement Plan](#5-enhancement-plan)

---

## 1. Current Architecture & Data Flow

### 1.1 Module Dependency Graph

```
main.rs
  ├── config.rs           (CLI parsing, SpacetimeDB CLI config auto-detection)
  ├── app.rs              (Event loop orchestrator, key dispatch, async event handler)
  │   ├── api/
  │   │   ├── client.rs   (HTTP client: SQL, schema, logs, databases, metrics)
  │   │   ├── types.rs    (Serde types: QueryResult, SchemaResponse, LogEntry, WsServerMessage)
  │   │   └── ws.rs       (WebSocket client: subscription + log-follow tasks)
  │   ├── state/
  │   │   └── app_state.rs (All UI state: AppState, Tab, FocusPanel, ConnectionStatus, etc.)
  │   └── ui/
  │       ├── layout.rs           (Title bar, tab bar, sidebar/content split)
  │       ├── sidebar.rs          (Database/table tree navigator)
  │       ├── components/
  │       │   ├── help.rs         (Help overlay popup)
  │       │   ├── input.rs        (Single-line text input widget + InputState)
  │       │   ├── status_bar.rs   (Bottom status bar)
  │       │   └── table_grid.rs   (Reusable data table widget + TableGridState)
  │       └── tabs/
  │           ├── tables.rs       (Tab 1: Table browser — rows of selected table)
  │           ├── sql.rs          (Tab 2: SQL console — input + history + results)
  │           ├── logs.rs         (Tab 3: Log viewer — scrollable, follow mode)
  │           ├── metrics.rs      (Tab 4: Metrics dashboard — cards + sparklines)
  │           └── module.rs       (Tab 5: Module inspector — reducers + tables)
```

### 1.2 Runtime Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  main() → tokio::runtime::block_on(async_main)                  │
│                                                                  │
│  ┌──────────────┐    mpsc::unbounded_channel    ┌────────────┐  │
│  │  Background   │  ─────── AppEvent ──────────> │  App::run  │  │
│  │  tokio::spawn │                               │  (event    │  │
│  │  tasks        │  <── SpacetimeClient ──────── │   loop)    │  │
│  └──────────────┘                               └────────────┘  │
│        │                                              │          │
│        ▼                                              ▼          │
│  SpacetimeClient (HTTP)                     crossterm::event     │
│  - ping, list_databases                     - KeyEvent           │
│  - get_schema, query_sql                    - Resize             │
│  - get_logs, get_metrics                                         │
│                                                                  │
│  WsHandle (WebSocket) ← NOT YET INTEGRATED                      │
└─────────────────────────────────────────────────────────────────┘
```

### 1.3 Data Flow — Key Paths

**Startup:**
1. `Config::parse()` reads CLI args + `~/.config/spacetime/cli.toml`
2. `SpacetimeClient::new()` creates HTTP client with auth token
3. `App::new()` creates `AppState`, mpsc channel, `InputState`, `TableGridState`
4. If `--database` provided: pre-populates `state.databases` and calls `select_database(0)`
5. `App::bootstrap()` spawns a task: ping → list_databases → `AppEvent::DatabasesLoaded`

**Database Selection:**
1. `nav_down()` on sidebar with `SidebarFocus::Databases` calls `state.database_next()` + `load_schema()`
2. `load_schema()` spawns task: `client.get_schema(db)` → `AppEvent::SchemaLoaded`
3. `handle_app_event(SchemaLoaded)` populates `state.tables`, `state.current_schema`

**Table Data Loading:**
1. `nav_enter()` on sidebar with `SidebarFocus::Tables` calls `load_table_data()`
2. `load_table_data()` spawns task: `client.query_sql(db, "SELECT * FROM {table} LIMIT 200")`
3. Result arrives as `AppEvent::QueryResult` → populates `state.query_result`

**SQL Execution:**
1. User types in SQL input (FocusPanel::SqlInput), presses Enter
2. `execute_sql()` spawns task: `client.query_sql(db, sql)`
3. Result → `AppEvent::QueryResult` or `AppEvent::QueryError`

**Rendering:**
1. `draw_frame()` called every TICK_RATE (200ms) or on event
2. Outer layout → sidebar + content area
3. Tab-specific renderer fills content area
4. Status bar at bottom, help overlay on top if visible

### 1.4 State Ownership Model

```
App (owns)
  ├── state: AppState          ← Single source of truth for all UI state
  ├── client: SpacetimeClient  ← Cloneable HTTP client (cheaply shared with tasks)
  ├── event_tx/event_rx        ← mpsc channel for async events
  ├── sql_input: InputState    ← DUPLICATED with state.sql_input/state.sql_cursor
  ├── tables_grid: TableGridState  ← Grid state for Tables tab
  └── sql_grid: TableGridState     ← Grid state for SQL results tab
```

**Critical Design Issue:** `sql_input: InputState` in `App` is a **parallel copy** of `state.sql_input`/`state.sql_cursor` in `AppState`. The code manually syncs them (`self.state.sql_input = self.sql_input.value.clone()`) but this is fragile and the sync is one-directional — `AppState.history_prev()` mutates `state.sql_input` but does NOT update `App.sql_input`.

---

## 2. Compiler Warnings — Categorized with Fix Instructions

**Total: 27 warnings across 9 files, 0 errors.**

### Category A: Deprecated API — `Buffer::get_mut()` (15 warnings)

The `ratatui` 0.29 crate deprecated `Buffer::get_mut(x, y)` in favor of index syntax `buf[(x, y)]` or `buf.cell_mut(Position::new(x, y))`.

| # | File | Line | Fix |
|---|------|------|-----|
| 1 | `src/ui/components/table_grid.rs` | 267 | Replace `buf.get_mut(x, header_y)` with `buf[(x, header_y)]` |
| 2 | `src/ui/components/table_grid.rs` | 275 | Replace `buf.get_mut(x, header_y)` with `buf[(x, header_y)]` |
| 3 | `src/ui/components/table_grid.rs` | 289 | Replace `buf.get_mut(x, sep_y)` with `buf[(x, sep_y)]` |
| 4 | `src/ui/components/table_grid.rs` | 295 | Replace `buf.get_mut(x, sep_y)` with `buf[(x, sep_y)]` |
| 5 | `src/ui/components/table_grid.rs` | 303 | Replace `buf.get_mut(x, sep_y)` with `buf[(x, sep_y)]` |
| 6 | `src/ui/components/table_grid.rs` | 359 | Replace `buf.get_mut(x, screen_y)` with `buf[(x, screen_y)]` |
| 7 | `src/ui/components/table_grid.rs` | 367 | Replace `buf.get_mut(x, screen_y)` with `buf[(x, screen_y)]` |
| 8 | `src/ui/sidebar.rs` | 129 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 9 | `src/ui/tabs/logs.rs` | 84 | Replace `buf.get_mut(x, area.y)` with `buf[(x, area.y)]` |
| 10 | `src/ui/tabs/logs.rs` | 131 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 11 | `src/ui/tabs/metrics.rs` | 222 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 12 | `src/ui/tabs/module.rs` | 127 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 13 | `src/ui/tabs/sql.rs` | 117 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 14 | `src/ui/tabs/sql.rs` | 153 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| 15 | `src/ui/tabs/tables.rs` | 101 | Replace `buf.get_mut(x, area.y)` with `buf[(x, area.y)]` |

**Fix pattern:** Global search-and-replace `buf.get_mut(x_expr, y_expr)` → `buf[(x_expr, y_expr)]`. The chained `.set_char(c).set_style(s)` calls work identically on the returned `&mut Cell`.

### Category B: Dead Code — Unused Variants / Fields / Methods (10 warnings)

| # | File | Line | Item | Fix |
|---|------|------|------|-----|
| 16 | `src/app.rs` | 58 | Variant `AppEvent::LogLine` never constructed | Wire up WebSocket integration (see Bug #3) or prefix with `_` temporarily |
| 17 | `src/app.rs` | 62 | Variant `AppEvent::Notification` never constructed | Add a code path that sends this event, or prefix with `_` |
| 18 | `src/config.rs` | 251-268 | All 14 fields of `ThemeColors` never read | Wire `ThemeColors` into UI renderers (replace hardcoded `const` colors) OR add `#[allow(dead_code)]` with a TODO comment |
| 19 | `src/config.rs` | 348 | Field `Config::ws_url` never read | Use in WebSocket connection setup (see Bug #3) |
| 20 | `src/config.rs` | 354-356 | Fields `Config::theme`, `Config::theme_name` never read | Pass to UI layer; convert `(u8,u8,u8)` → `Color::Rgb` |
| 21 | `src/config.rs` | 425 | Method `Config::uses_tls()` never used | Use in WS URL construction, or remove |
| 22 | `src/ui/components/table_grid.rs` | 58, 65 | Methods `scroll_right()`, `scroll_left()` never used | Wire into key handlers for horizontal scrolling (h/l keys in main panel) |
| 23 | `src/ui/components/table_grid.rs` | 119 | Method `max_col_width()` never used | Use in `render_tables` / `render_sql` or remove |

### Category C: Unused Constants (3 warnings)

| # | File | Line | Constant | Fix |
|---|------|------|----------|-----|
| 24 | `src/ui/sidebar.rs` | 30 | `BORDER_FOCUSED` | Remove — sidebar border is drawn by `layout.rs` |
| 25 | `src/ui/sidebar.rs` | 31 | `BORDER_NORMAL` | Remove — sidebar border is drawn by `layout.rs` |
| 26 | `src/ui/tabs/metrics.rs` | 24 | `FG_PRIMARY` | Remove or use in `render_extra_metrics` |
| 27 | `src/ui/tabs/module.rs` | 19 | `FG_PRIMARY` | Remove or use in `render_tables_panel` |
| — | `src/ui/tabs/sql.rs` | 30 | `WARNING` | Remove or use for loading indicator |

*(Note: the compiler reports 27 warnings total; the `WARNING` constant in sql.rs is warning #27 per the output but I've listed it with the constants group.)*

### Category D: Unused Assignment (1 warning)

| # | File | Line | Issue | Fix |
|---|------|------|-------|-----|
| 28 | `src/ui/components/input.rs` | 260 | `phase = 1` assigned but never read | The `phase` variable at line 260 is set inside the `byte_pos == cursor_pos` branch but the value is only checked in subsequent iterations. The fix: remove the redundant assignment at line 260 OR restructure the logic. The assignment on line 260 is inside the "cursor at end of text" handler (`if byte_pos == cursor_pos && phase == 0`) — `phase = 1` is set but the loop has already ended. Remove the assignment. |

---

## 3. Bug Analysis

### Bug #1: Database Selection — Stale State on Navigation

**Files:** `src/api/client.rs`, `src/state/app_state.rs`, `src/app.rs`

**Symptoms:**
- Navigating between databases with j/k in sidebar triggers `load_schema()` on every cursor move (even when moving to the same database)
- `select_database()` clears tables and schema on every call, even when re-selecting the same database
- Pre-selected database via `--database` flag is added to `state.databases` before `bootstrap()` fetches the real list; when `DatabasesLoaded` arrives, the deduplication logic (`if !self.state.databases.contains(&db)`) may fail if the user passed a friendly name but the server returns a hex identity

**Root Causes:**

1. **`app.rs:344-348` (`nav_down`):** Compares `old` vs `new` index to decide whether to load schema, but `database_next()` internally calls `select_database()` which **always clears tables/schema** even when the index doesn't change (e.g., at the end of the list). The comparison happens *after* the state is already cleared.

2. **`app_state.rs:331-337` (`select_database`):** Unconditionally clears `selected_table_idx`, `tables`, and `current_schema`. Should be a no-op when `idx == self.selected_database_idx.unwrap_or(usize::MAX)`.

3. **`app.rs:67-70` (pre-selection):** `app.state.databases.push(db.clone())` then `app.state.select_database(0)`. But when `DatabasesLoaded` arrives (line 439), the code drains `state.databases`, replaces with server list, and re-inserts missing entries. The name-matching uses exact string equality — if the user passed `my_db` but the server returns the hex identity `a1b2c3...`, the pre-selected name is lost.

4. **`app.rs:445-447`:** After `DatabasesLoaded`, if `selected_database_idx.is_none()`, it selects index 0 and loads schema. But if a database was already selected via `--database`, `selected_database_idx` is `Some(0)` and this branch is skipped — the schema is never loaded for the pre-selected database.

**Fix Plan:**
- In `select_database()`: add early return if `Some(idx) == self.selected_database_idx`
- In `nav_down()`: remove the stale `old` comparison; rely on `select_database()` being a no-op
- In `async_main()`: after pre-selecting, trigger `load_schema()` explicitly (or defer to `bootstrap`)
- In `DatabasesLoaded` handler: resolve pre-selected name to server identity; re-validate `selected_database_idx` after list replacement

### Bug #2: Live Data Viewing — WebSocket Never Connected

**Files:** `src/api/ws.rs`, `src/app.rs`, `src/state/app_state.rs`

**Symptoms:**
- The README promises "Live Table Viewer" with real-time row updates, but no WebSocket connection is ever established
- The Logs tab shows "no log entries — connect to a database to stream logs" forever
- `AppEvent::LogLine` variant is never constructed (compiler warning confirms this)

**Root Cause:** The WebSocket module (`ws.rs`) is **fully implemented** — `spawn_subscription()` and `spawn_log_follow()` are complete with proper connection handling, subscription messages, and frame decoding. However, **no code in `app.rs` or anywhere else ever calls these functions**. The `WsHandle` is never stored in `App`. The `WsEvent` channel is never drained.

**Evidence:**
- `ws.rs` exports `spawn_subscription`, `spawn_log_follow`, `WsConfig`, `WsHandle`, `WsEvent`
- `api/mod.rs` re-exports `WsConfig`, `WsEvent`, `WsHandle`
- `app.rs` does NOT import any of these
- `App` struct has no `ws_handle` field
- `Config::ws_url` is computed but never read (compiler warning)
- The only log loading is via HTTP `GET /v1/database/{db}/logs` (non-streaming)

**Impact:** The core real-time features advertised in the README (live row updates, log streaming) are non-functional.

### Bug #3: UI State Management — Dual SQL Input State

**Files:** `src/app.rs`, `src/state/app_state.rs`, `src/ui/components/input.rs`

**Symptoms:**
- SQL history navigation (↑/↓) populates `state.sql_input` via `state.history_prev()`/`state.history_next()`, but the actual input widget reads from `App.sql_input: InputState`
- When the user presses ↑ in SQL input mode, `app.rs:213-218` calls `state.history_prev()` then reads from `state.sql_history` to call `self.sql_input.set()` — this works but the index computation is wrong

**Root Causes:**

1. **Dual state:** `App.sql_input: InputState` and `AppState.sql_input: String` / `AppState.sql_cursor: usize` represent the same data. Sync is manual and one-directional:
   - `app.rs:237-238`: After key handling in SQL mode: `self.state.sql_input = self.sql_input.value.clone()` and `self.state.sql_cursor = self.sql_input.cursor`
   - But `AppState::history_prev()` (line 515) mutates `self.sql_input` and `self.sql_cursor` directly — these changes are **never propagated back** to `App.sql_input`

2. **History index computation is inverted:** In `app.rs:213-218`:
   ```
   self.state.history_prev();
   if let Some(cursor) = self.state.history_cursor {
       let idx = self.state.sql_history.len().saturating_sub(1 + cursor);
   ```
   But `state.history_prev()` (app_state.rs:508) sets `history_cursor` to count from the END (0 = most recent). Then `app.rs` computes `idx = len - 1 - cursor` which is correct. However, `state.history_prev()` ALSO sets `self.sql_input` and `self.sql_cursor` — so by the time `app.rs` reads from `self.state.sql_history.get(idx)`, the state has already been updated. The `self.sql_input.set(entry.sql.clone())` call in `app.rs` is redundant but not harmful.

3. **The real bug:** When history navigation happens, `state.history_prev()` updates `state.sql_input` (the String), but the rendering reads from `App.sql_input` (the InputState). The `app.rs` code at lines 213-218 does correctly sync by calling `self.sql_input.set(...)`, but this sync happens ONLY inside the `if let Some(cursor)` block — if `history_cursor` becomes `None` (which shouldn't happen after `history_prev` unless history is empty), the sync is skipped.

4. **`AppState` has full SQL editing methods that are never used:** `sql_insert_char`, `sql_backspace`, `sql_delete`, `sql_cursor_left`, `sql_cursor_right`, `sql_cursor_home`, `sql_cursor_end`, `sql_clear` — all of these exist in `app_state.rs` but are never called because `app.rs` delegates to `App.sql_input: InputState` instead.

### Bug #4: Tab/Grid State Mismatch

**Files:** `src/app.rs`

**Symptoms:** The Tables tab (Tab::Query) and SQL tab (Tab::Schema) share `state.query_result` for their data, but use separate `TableGridState` instances (`tables_grid` vs `sql_grid`). When the user runs a SQL query from the SQL tab, the result is stored in `state.query_result`, but if they switch to the Tables tab, the same result is displayed there too (unintentionally).

**Root Cause:** There is no separation between "table browse result" and "SQL query result". Both tabs read from `state.query_result`. The Tables tab should have its own result or use the table cache.

### Bug #5: Schema Endpoint Returns `SchemaResponse` but `get_schema` Returns `Schema`

**Files:** `src/api/client.rs`, `src/api/types.rs`

**Non-bug but confusing:** `types.rs:158` defines `pub type Schema = SchemaResponse;`. The `get_schema()` method signature says `Result<Schema>` but the parser function `parse_schema_response` returns `Result<SchemaResponse>`. This is technically fine due to the type alias but confusing. The naming should be unified.

---

## 4. Per-File Fix Plan

### 4.1 `src/main.rs` (114 lines)

| Action | Details |
|--------|---------|
| **Remove** | `#[allow(dead_code, unused_imports)]` on `mod api` (line 11) — fix the actual dead code instead of suppressing |
| **Change** | After `App::new()`, if `--database` is set, also trigger schema load: move the pre-selection logic into `App::bootstrap()` or call `app.load_schema().await` after pre-selection |

### 4.2 `src/config.rs` (489 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Keep** | All `ThemeColors` fields | They should be wired into the UI; add `#[allow(dead_code)]` temporarily with `// TODO: wire into UI renderers` |
| **Keep** | `ws_url`, `theme`, `theme_name` fields | They will be used once WebSocket integration and theming are wired up |
| **Remove** | `uses_tls()` method (line 425) | Redundant — TLS is already encoded in the URL scheme. Or keep and use in WS code |
| **No changes needed** | Config parsing, CLI detection | Solid implementation |

### 4.3 `src/api/mod.rs` (21 lines)

| Action | Details |
|--------|---------|
| **Remove** | `#![allow(dead_code)]` (line 8) — fix actual dead code instead |
| **Keep** | All re-exports — they're correct and will be used |

### 4.4 `src/api/client.rs` (900 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Change** | `get_schema()` return type | Rename to return `SchemaResponse` explicitly for clarity (or keep the alias) |
| **Remove** | `extract_database_names()` function (line 476-510) | It's defined but never called — `list_databases()` uses its own inline parsing. Either use it or remove it. Currently it's dead code hidden by the module-level `#[allow(dead_code)]` |
| **Add** | Error context to `list_databases()` | When JWT parsing fails, the error message is good but should also suggest `--token` flag |
| **Verify** | SQL endpoint URL | Currently `POST /v1/database/{db}/sql` — matches SpacetimeDB 2.0 API ✓ |
| **Verify** | Schema endpoint URL | Currently `GET /v1/database/{db}/schema?version=9` — matches SpacetimeDB 2.0 API ✓ |
| **Verify** | Logs endpoint URL | Currently `GET /v1/database/{db}/logs` — matches SpacetimeDB 2.0 API ✓ |

### 4.5 `src/api/types.rs` (285 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Consider removing** | `type Schema = SchemaResponse` (line 158) | Confusing alias — use `SchemaResponse` everywhere or rename the struct to `Schema` |
| **Add** | `#[serde(default)]` to `LogEntry.level` | If the server omits `level`, deserialization fails. Add a default |
| **Add** | `Display` impl for `TransactionStatus` | For error messages |
| **No changes needed** | `WsServerMessage`, `DatabaseUpdate`, `TableUpdate` | Well-structured for WS integration |

### 4.6 `src/api/ws.rs` (503 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **No code changes needed** | — | The WebSocket implementation is complete and well-structured |
| **Add** | Reconnection logic | Currently, if the connection drops, the task exits. Add exponential backoff reconnection (see Enhancement Plan §5.2) |
| **Add** | BSATN binary frame decoding | Line 449: binary frames are logged but not decoded. SpacetimeDB 2.0 uses BSATN for subscription data. This requires a BSATN decoder or requesting JSON protocol |
| **Change** | Protocol header | Line 224: `"v1.bsatn.spacetimedb"` requests BSATN encoding. Change to `"v1.json.spacetimedb"` to receive JSON-encoded messages that the existing `serde_json` decoders can handle |

### 4.7 `src/state/mod.rs` (15 lines)

| Action | Details |
|--------|---------|
| **Remove** | `#![allow(dead_code)]` (line 7) — fix actual dead code |

### 4.8 `src/state/app_state.rs` (830 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Change** | `select_database()` (line 331) | Add early return: `if self.selected_database_idx == Some(idx) { return; }` |
| **Remove** | `sql_insert_char`, `sql_backspace`, `sql_delete`, `sql_cursor_left`, `sql_cursor_right`, `sql_cursor_home`, `sql_cursor_end`, `sql_clear` (lines 391-473) | These duplicate `InputState` methods and are never called. Remove them to eliminate the dual-state confusion. OR: remove `InputState` from `App` and use these methods exclusively |
| **Add** | `ws_connected: bool` field | Track WebSocket connection state |
| **Add** | `live_table_data: HashMap<String, Vec<Vec<Value>>>` field | Store live subscription data separately from query results |
| **Change** | `history_prev()` / `history_next()` (lines 505-535) | Remove the `self.sql_input = ...` and `self.sql_cursor = ...` mutations — let the caller (`app.rs`) handle the sync to avoid dual-state issues |

### 4.9 `src/app.rs` (965 lines) — **Most Changes Needed**

| Action | Line(s) | Details |
|--------|---------|---------|
| **Add** | Import `WsConfig`, `WsHandle`, `WsEvent` from `crate::api` | Required for WebSocket integration |
| **Add** | `ws_handle: Option<WsHandle>` field to `App` struct | Store the WebSocket connection handle |
| **Add** | `connect_ws()` method | After database selection, spawn a WebSocket subscription: `ws::spawn_subscription(WsConfig { base_url: config.ws_url, database: db, auth_token, channel_capacity: 256 })` |
| **Add** | WsEvent drain in main loop | After draining `event_rx`, also drain `ws_handle.event_rx` if connected |
| **Change** | `bootstrap()` (line 104) | After `DatabasesLoaded` + schema load, also call `connect_ws()` |
| **Change** | `handle_app_event(DatabasesLoaded)` (line 439) | After selecting database and loading schema, establish WS connection |
| **Remove** | Duplicate history index computation in `handle_key` (lines 213-233) | Simplify: call `state.history_prev()`, then sync `self.sql_input.set(state.sql_input.clone())` |
| **Change** | `nav_down()` for Sidebar/Databases (line 344) | Remove the `old` comparison hack. Instead, check if the database actually changed after `database_next()` returns |
| **Add** | `load_schema()` call in `async_main()` after pre-selection | Line 70: after `app.state.select_database(0)`, add `app.load_schema().await` |
| **Change** | `AppEvent::LogLine` variant | Wire it to `state.push_log()` — it's already handled at line 472 but never sent |
| **Add** | Periodic metrics refresh | Spawn a timer task that sends `AppEvent::MetricsLoaded` every 10 seconds when on the Metrics tab |
| **Fix** | Tab naming confusion | `Tab::Query` renders as "Tables" and `Tab::Schema` renders as "SQL" — the enum variant names are swapped relative to their display titles. Either rename the variants or accept the confusing mapping |

### 4.10 `src/ui/components/input.rs` (299 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Line 260 | Remove the `phase = 1;` assignment inside the "cursor at end of text" block (it's dead code — the loop has already exited at that point) |
| **Improve** | Cursor rendering logic (lines 230-270) | The char-by-char rendering is complex and has edge cases. Consider simplifying: split text at cursor position, render before/cursor_char/after as three spans |

### 4.11 `src/ui/components/table_grid.rs` (411 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Lines 267, 275, 289, 295, 303, 359, 367 | Replace all `buf.get_mut(x, y)` with `buf[(x, y)]` |
| **Remove** | `scroll_right()`, `scroll_left()` (lines 58, 65) | OR wire them into key handlers in `app.rs` for horizontal scrolling |
| **Remove** | `max_col_width()` method (line 119) | OR use it when constructing `TableGrid` |

### 4.12 `src/ui/sidebar.rs` (284 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Remove** | `BORDER_FOCUSED` constant (line 30) | Unused — border drawn by `layout.rs` |
| **Remove** | `BORDER_NORMAL` constant (line 31) | Unused — border drawn by `layout.rs` |
| **Fix** | Line 129 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |

### 4.13 `src/ui/layout.rs` (209 lines)

| Action | Details |
|--------|---------|
| **No changes needed** | Clean implementation, correct layout math |
| **Consider** | Using `ThemeColors` from `Config` instead of hardcoded constants |

### 4.14 `src/ui/components/help.rs` (182 lines)

| Action | Details |
|--------|---------|
| **No changes needed** | Clean implementation |
| **Consider** | Adding WebSocket-related key bindings once WS is integrated |

### 4.15 `src/ui/components/status_bar.rs` (183 lines)

| Action | Details |
|--------|---------|
| **No changes needed** | Clean implementation |
| **Consider** | Adding WS connection status indicator |

### 4.16 `src/ui/tabs/tables.rs` (205 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Line 101 | Replace `buf.get_mut(x, area.y)` with `buf[(x, area.y)]` |
| **Change** | `build_table_data()` | Should read from a dedicated table-browse result, not shared `state.query_result` |

### 4.17 `src/ui/tabs/sql.rs` (258 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Lines 117, 153 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| **Remove** | `WARNING` constant (line 30) | Unused |

### 4.18 `src/ui/tabs/logs.rs` (234 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Lines 84, 131 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| **Add** | Log level filter UI | `state.log_filter_level` exists but there's no UI to change it |

### 4.19 `src/ui/tabs/metrics.rs` (286 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Line 222 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| **Remove** | `FG_PRIMARY` constant (line 24) | Unused |

### 4.20 `src/ui/tabs/module.rs` (304 lines)

| Action | Line(s) | Details |
|--------|---------|---------|
| **Fix** | Line 127 | Replace `buf.get_mut(x, y)` with `buf[(x, y)]` |
| **Remove** | `FG_PRIMARY` constant (line 19) | Unused |

### 4.21 `src/ui/mod.rs` (7 lines), `src/ui/components/mod.rs` (6 lines), `src/ui/tabs/mod.rs` (7 lines), `src/state/mod.rs` (15 lines), `src/api/mod.rs` (21 lines)

| Action | Details |
|--------|---------|
| **No structural changes** | Module re-exports are correct |
| **Remove** | `#![allow(dead_code)]` from `api/mod.rs` and `state/mod.rs` after fixing actual dead code |

---

## 5. Enhancement Plan

### 5.1 Error Handling

**Current State:** Error handling is reasonable — `anyhow::Result` throughout, context strings on most operations. But there are gaps:

| Issue | Location | Fix |
|-------|----------|-----|
| **Silent channel send failures** | Throughout `app.rs` | All `let _ = tx.send(...)` silently drop errors. Add `tracing::warn!` on failure |
| **No retry on transient HTTP errors** | `client.rs` | Add retry with exponential backoff for 5xx errors and timeouts |
| **Panic on invalid UTF-8 in SQL input** | `input.rs` | `value.remove(bc)` can panic if `bc` is not a char boundary despite the check. Add `debug_assert!(self.value.is_char_boundary(bc))` |
| **No timeout on schema/query operations** | `app.rs` | Background tasks can hang indefinitely. Add `tokio::time::timeout` wrappers |
| **Error popup blocks all input** | `app.rs:190` | Any key clears the error — even accidental keypresses. Consider requiring Esc or Enter specifically |

### 5.2 Reconnection Logic

**Current State:** No reconnection exists. If the server goes down, the status shows "Error" but there's no recovery path.

**Plan:**

1. **HTTP Health Check Loop:**
   - Add a periodic ping task (every 5 seconds) that sends `AppEvent::PingResult`
   - On `PingResult(false)`: set `ConnectionStatus::Error`, show notification
   - On `PingResult(true)` after an error: set `ConnectionStatus::Connected`, re-fetch schema

2. **WebSocket Reconnection (once WS is integrated):**
   - In `subscription_task` and `log_follow_task`: wrap the main loop in a retry loop
   - On disconnect: wait with exponential backoff (1s, 2s, 4s, 8s, max 30s)
   - Send `WsEvent::Disconnected` with retry info
   - On reconnect: re-send subscription queries
   - Store `WsConfig` in `App` so reconnection can reuse it

3. **UI Feedback:**
   - Add a "Reconnecting..." status to `ConnectionStatus` enum
   - Show retry countdown in status bar
   - Allow manual reconnect with a key binding (e.g., `Ctrl+R`)

### 5.3 WebSocket Integration Plan (Critical Missing Feature)

**Priority: HIGH — This is the most impactful missing feature.**

**Step 1: Wire up WebSocket on database selection**
```
App::connect_ws(database: &str)
  → build WsConfig from Config.ws_url + database + auth_token
  → call ws::spawn_subscription(config)
  → store WsHandle in App.ws_handle
```

**Step 2: Drain WsEvent in event loop**
```
In App::run() main loop, after draining event_rx:
  if let Some(ref mut ws) = self.ws_handle {
      while let Ok(event) = ws.event_rx.try_recv() {
          self.handle_ws_event(event).await;
      }
  }
```

**Step 3: Handle WsEvent variants**
```
WsEvent::Connected → state.ws_connected = true, notification
WsEvent::ServerMessage(InitialSubscription{..}) → populate live table data
WsEvent::ServerMessage(TransactionUpdate{..}) → apply inserts/deletes to live data
WsEvent::LogLine(entry) → state.push_log(entry)
WsEvent::Disconnected{..} → state.ws_connected = false, trigger reconnect
```

**Step 4: Subscribe to all tables after schema load**
```
After SchemaLoaded event:
  let queries: Vec<String> = schema.tables.iter()
      .filter(|t| t.table_type == "user")
      .map(|t| format!("SELECT * FROM {}", t.table_name))
      .collect();
  ws_handle.subscribe(queries, 1).await;
```

**Step 5: Change WS protocol to JSON**
- In `ws.rs:224`: change `"v1.bsatn.spacetimedb"` to `"v1.json.spacetimedb"`
- This ensures all frames are JSON-decodable with existing `serde_json` code

### 5.4 UI/UX Improvements

| Improvement | Priority | Details |
|-------------|----------|---------|
| **Theming** | Medium | Wire `Config.theme` (`ThemeColors`) into all UI renderers. Replace 50+ hardcoded `const` color values with lookups from a shared theme struct passed through render functions |
| **Horizontal scrolling** | Medium | Wire `TableGridState::scroll_right/left` to `h`/`l` keys when focus is on a table grid |
| **Log level filter** | Medium | Add key binding (e.g., `f` in Logs tab) to cycle through log levels. `state.log_filter_level` already exists |
| **Confirmation on quit** | Low | When queries are in-flight or WS is connected, show "Are you sure?" |
| **Resize handling** | Low | `state.needs_redraw` is set on resize but never consumed. Remove it or use it to trigger an immediate redraw |
| **Search in SQL results** | Low | Add `/` search within the table grid results |
| **Copy to clipboard** | Low | Add `y` to yank selected row/cell to clipboard |
| **Tab rename** | Low | `Tab::Query` displays as "Tables" and `Tab::Schema` displays as "SQL" — rename enum variants to match: `Tab::Tables` and `Tab::Sql` |
| **Error popup improvement** | Low | Show error text with word-wrap in a bordered popup, not just in status bar |
| **Sidebar width** | Low | Currently fixed at 20%. Make configurable or auto-size based on longest database name |

### 5.5 Performance

| Issue | Impact | Fix |
|-------|--------|-----|
| **200ms tick rate** | Low | TICK_RATE of 200ms means up to 200ms input latency. Reduce to 50ms or use `event::poll(Duration::ZERO)` with a separate timer for non-event redraws |
| **Full redraw every tick** | Medium | Every 200ms, the entire frame is redrawn. Ratatui's `Terminal::draw` already diffs, but building all widgets is CPU work. Consider skipping `draw` when nothing changed (check `state.needs_redraw`) |
| **Clone-heavy rendering** | Low | `entry.message.clone()` in logs renderer (line 183 of logs.rs) clones every log message every frame. Use `Span::raw(&entry.message)` with borrowed references |
| **Unbounded channel** | Low | `mpsc::unbounded_channel` for AppEvents can grow without bound if the UI thread is slow. Consider bounded channel with backpressure |
| **String allocations in table grid** | Medium | `value_to_display()` allocates a new String for every cell on every frame. Cache display strings in `TableCache` or compute once on data arrival |
| **SQL history rendering** | Low | History panel re-renders all visible entries every frame. Pre-compute `Line` objects and cache them |

### 5.6 Testing

| Area | Current | Needed |
|------|---------|--------|
| **Unit tests** | Good coverage in `client.rs`, `config.rs`, `app_state.rs`, `ws.rs` | Add tests for `parse_prometheus_metrics`, `value_to_display` |
| **Integration tests** | None | Add mock HTTP server tests for `SpacetimeClient` methods |
| **UI tests** | None | Add `ratatui` backend buffer tests for key renderers |
| **WebSocket tests** | URL construction only | Add mock WS server tests once integration is complete |

---

## Appendix A: File Size Summary

| File | Lines | Role |
|------|-------|------|
| `src/app.rs` | 965 | Event loop, key dispatch, async handlers |
| `src/api/client.rs` | 900 | HTTP client + response parsers |
| `src/state/app_state.rs` | 830 | All UI state |
| `src/api/ws.rs` | 503 | WebSocket client (unused) |
| `src/config.rs` | 489 | CLI parsing, config auto-detection |
| `src/ui/components/table_grid.rs` | 411 | Data table widget |
| `src/ui/tabs/module.rs` | 304 | Module inspector tab |
| `src/ui/components/input.rs` | 299 | Text input widget |
| `src/ui/tabs/metrics.rs` | 286 | Metrics dashboard tab |
| `src/api/types.rs` | 285 | API response types |
| `src/ui/sidebar.rs` | 284 | Sidebar tree navigator |
| `src/ui/tabs/sql.rs` | 258 | SQL console tab |
| `src/ui/tabs/logs.rs` | 234 | Log viewer tab |
| `src/ui/layout.rs` | 209 | Layout chrome |
| `src/ui/tabs/tables.rs` | 205 | Table browser tab |
| `src/ui/components/status_bar.rs` | 183 | Status bar |
| `src/ui/components/help.rs` | 182 | Help overlay |
| `src/main.rs` | 114 | Entry point |
| `src/api/mod.rs` | 21 | API module re-exports |
| `src/state/mod.rs` | 15 | State module re-exports |
| `src/ui/tabs/mod.rs` | 7 | Tab module declarations |
| `src/ui/mod.rs` | 7 | UI module declarations |
| `src/ui/components/mod.rs` | 6 | Component module declarations |
| **Total** | **~6,997** | |

## Appendix B: SpacetimeDB 2.0 API Reference (Used Endpoints)

| Endpoint | Method | Used In | Status |
|----------|--------|---------|--------|
| `/v1/ping` | GET | `client.rs:ping()` | ✅ Working |
| `/v1/database/{db}/sql` | POST (text/plain body) | `client.rs:query_sql()` | ✅ Working |
| `/v1/database/{db}/schema?version=9` | GET | `client.rs:get_schema()` | ✅ Working |
| `/v1/database/{db}/logs` | GET | `client.rs:get_logs()` | ✅ Working |
| `/v1/identity/{id}/databases` | GET | `client.rs:list_databases()` | ✅ Working |
| `/v1/database/{id}/names` | GET | `client.rs:get_database_names()` | ✅ Working |
| `/metrics` | GET | `client.rs:get_metrics()` | ✅ Working |
| `ws://.../v1/database/{db}/subscribe` | WebSocket | `ws.rs:spawn_subscription()` | ⚠️ Implemented but **not wired** |
| `ws://.../v1/database/{db}/logs?follow=true` | WebSocket | `ws.rs:spawn_log_follow()` | ⚠️ Implemented but **not wired** |

---

*End of audit. This document should be used as the authoritative reference for all implementation work on this codebase.*
