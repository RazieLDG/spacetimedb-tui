# Contextual Workbench Phase 3 Workbench Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build Phase 3 Contextual Workbench UX with DATA/OBSERVE/OPERATE modes, responsive Explorer/Workspace/Inspector/Activity surfaces, command-registry generated palette/help, mouse parity, migration bindings, and explicit lifecycle UI.

**Architecture:** Add focused state and UI modules around the existing `src/app.rs` shell instead of a risky big-bang rewrite. Preserve shared type names from the approved investigation: `WorkbenchMode`, `Workspace`, `WorkbenchPane`, `LayoutClass`, `CompactSurface`, `WorkbenchState`, `LifecyclePresentation`, `ActivityOutcome`, `ActivityItem`, `ActivityState`, `CommandId`, `CommandSpec`, `Availability`, `DisabledReason`, `WorkbenchAreas`, `HitRegion`, and `HitMap`. Phase 2 owns the command registry in `src/app/command.rs`; Phase 3 extends that registry in place and must not create a competing registry elsewhere. Existing `Tab`, `FocusPanel`, and current tab renderers stay bridged for compatibility during Phase 3.

**Tech Stack:** Rust, Ratatui, Crossterm, Tokio, existing unit tests in `src/main.rs`, Ratatui `TestBackend` render tests, `cargo fmt`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build --release`.

---

## Current code anchors

- Approved UX spec: `docs/superpowers/specs/2026-07-26-contextual-workbench-ux-design.md`
- App shell and input: `src/app.rs`
  - `AppEvent` lines 41-90
  - `App` lines 95-127
  - `handle_key` lines 369-1057
  - `handle_palette_key` lines 1930-1965
  - `dispatch_command` lines 1968-1997
  - `draw_frame` lines 3977-4065
  - `run` currently ignores `Event::Mouse(_)` around line 237
- State: `src/state/app_state.rs`
  - Current repo names: `Tab`, `FocusPanel`, `SidebarFocus`, `ConnectionStatus`, `ConnectionInfo`, `AppState`
- Phase 2 inherited/future contract state names, not current anchors: `crate::state::activity::ConnectionState` and retry state APIs such as `RetryState`. Task steps below may reference those Phase 2-created contracts and should adapt to the exact Phase 2 names when implemented.
- Palette: `src/state/palette.rs`, `src/ui/components/palette.rs`
- Help: `src/ui/components/help.rs`
- Layout/sidebar/status: `src/ui/layout.rs`, `src/ui/sidebar.rs`, `src/ui/components/status_bar.rs`
- Existing workspace renderers: `src/ui/tabs/{tables,sql,logs,metrics,module,live}.rs`

## Responsive contracts

- Wide: width `>= 120` and height `>= 20`. Show Explorer, Workspace, Inspector, and Activity simultaneously.
- Medium: width `>= 90` and height `>= 16`. Show Explorer and Workspace, with Inspector as a drawer and Activity condensed.
- Compact: width `>= 40` and height `>= 12`. Show one primary surface at a time with breadcrumb and connection state always visible.
- Too small: width `< 40` or height `< 12`. Render a minimal `terminal too small` screen and never panic.

## TestBackend matrix

Every render task that touches layout must include at least these dimensions:

```rust
const RENDER_SIZES: &[(u16, u16)] = &[
    (120, 20),
    (119, 20),
    (90, 16),
    (89, 16),
    (40, 12),
    (39, 12),
    (40, 11),
    (20, 5),
    (0, 0),
];
```

---

### Task 1: Add Ratatui TestBackend render support

**Files:**
- Create: `src/ui/test_support.rs`
- Modify: `src/ui/mod.rs`

- [ ] **RED: Write failing render helper tests**

Add this file:

```rust
// src/ui/test_support.rs
#![cfg(test)]

use ratatui::{backend::TestBackend, buffer::Buffer, layout::Rect, Terminal};

pub fn render_to_buffer<F>(width: u16, height: u16, render: F) -> Buffer
where
    F: FnOnce(Rect, &mut Buffer),
{
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend creates terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(area, frame.buffer_mut());
        })
        .expect("test render succeeds");
    terminal.backend().buffer().clone()
}

pub fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in area.y..area.y.saturating_add(area.height) {
        for x in area.x..area.x.saturating_add(area.width) {
            out.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

pub fn render_to_string<F>(width: u16, height: u16, render: F) -> String
where
    F: FnOnce(Rect, &mut Buffer),
{
    buffer_text(&render_to_buffer(width, height, render))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{text::Line, widgets::{Paragraph, Widget}};

    #[test]
    fn render_to_string_extracts_visible_text() {
        let text = render_to_string(20, 3, |area, buf| {
            Paragraph::new(Line::from("hello")).render(area, buf);
        });
        assert!(text.contains("hello"));
    }

    #[test]
    fn render_to_string_handles_unicode_without_panicking() {
        let text = render_to_string(30, 3, |area, buf| {
            Paragraph::new(Line::from("é 你 🚀 … µs")).render(area, buf);
        });
        assert!(text.contains("é"));
        assert!(text.contains("你"));
        assert!(text.contains("…"));
        assert!(text.contains("µ"));
    }

    #[test]
    fn zero_sized_backend_does_not_panic() {
        let text = render_to_string(0, 0, |_area, _buf| {});
        assert_eq!(text, "");
    }
}
```

In `src/ui/mod.rs`, add:

```rust
#[cfg(test)]
pub mod test_support;
```

- [ ] **RED: Run test and confirm failure before implementation if module export is missing**

```bash
cargo test ui::test_support -- --nocapture
```

Expected initial failure before `src/ui/mod.rs` export: unresolved module or missing helper.

- [ ] **GREEN: Run test after adding helper and export**

```bash
cargo test ui::test_support -- --nocapture
```

Expected: all `ui::test_support` tests pass.

- [ ] **Commit boundary**

```bash
git add src/ui/mod.rs src/ui/test_support.rs
git commit -m "test: add ratatui render test support"
```

Do not commit during this plan-writing session.

---

### Task 2: Extend Phase 2 contextual workbench state

**Files:**
- Modify: `src/state/workbench.rs`
- Modify as needed only: `src/state/mod.rs`
- Modify as needed only: `src/state/app_state.rs`

- [ ] **RED: Add failing tests for Phase 3 workbench extensions**

Extend the Phase 2-owned `src/state/workbench.rs`. Do not replace the file wholesale and do not create a competing owner for workbench state. Add missing variants, fields, helpers, and tests only if Phase 2 did not already define them:

```rust
// Additive extension only. Phase 2 already defines WorkbenchMode, Workspace,
// and WorkbenchPane. Do not redefine those three enums.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutClass { Wide, Medium, Compact, TooSmall }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactSurface {
    Explorer,
    #[default]
    Workspace,
    Inspector,
    Activity,
}

// Apply this exact additive replacement to the existing WorkbenchState struct.
// It preserves every Phase 2 field and adds only Phase 3 workbench UI fields.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub compact_surface: CompactSurface,
    pub inspector_open: bool,
    pub activity_open: bool,
    pub workspace_scroll_offset: u16,
}

impl Default for WorkbenchState {
    fn default() -> Self {
        Self {
            mode: WorkbenchMode::Data,
            workspace: Workspace::Tables,
            focused_pane: WorkbenchPane::Explorer,
            sql_input: String::new(),
            sql_history_cursor: None,
            table_grid_cursor: None,
            table_search: String::new(),
            palette_open: false,
            help_open: false,
            modal_open: false,
            compact_surface: CompactSurface::Workspace,
            inspector_open: false,
            activity_open: true,
            workspace_scroll_offset: 0,
        }
    }
}

// Never add `layout_class` as mutable state. It is derived from terminal size
// with LayoutClass::for_size(width, height).
impl LayoutClass {
    pub fn for_size(width: u16, height: u16) -> Self {
        if width < 40 || height < 12 { Self::TooSmall }
        else if width >= 120 && height >= 20 { Self::Wide }
        else if width >= 90 && height >= 16 { Self::Medium }
        else { Self::Compact }
    }
}

impl Workspace {
    pub fn mode(self) -> WorkbenchMode {
        match self {
            Workspace::Tables | Workspace::Sql => WorkbenchMode::Data,
            Workspace::Logs | Workspace::Metrics | Workspace::Live => WorkbenchMode::Observe,
            Workspace::Module => WorkbenchMode::Operate,
        }
    }
}

impl WorkbenchState {
    pub fn set_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
        self.mode = workspace.mode();
        self.compact_surface = CompactSurface::Workspace;
        self.workspace_scroll_offset = 0;
    }

    pub fn set_compact_surface(&mut self, surface: CompactSurface) {
        self.compact_surface = surface;
        self.focused_pane = match surface {
            CompactSurface::Explorer => WorkbenchPane::Explorer,
            CompactSurface::Workspace => WorkbenchPane::Workspace,
            CompactSurface::Inspector => WorkbenchPane::Inspector,
            CompactSurface::Activity => WorkbenchPane::Activity,
        };
    }

    pub fn scroll_workspace_by_pages(&mut self, pages: i16, max_offset: u16) {
        let delta = pages.saturating_mul(10);
        let next = if delta.is_negative() {
            self.workspace_scroll_offset.saturating_sub(delta.unsigned_abs())
        } else {
            self.workspace_scroll_offset.saturating_add(delta as u16)
        };
        self.workspace_scroll_offset = next.min(max_offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_class_is_derived_from_width_and_height_without_stored_state() {
        assert_eq!(LayoutClass::for_size(120, 20), LayoutClass::Wide);
        assert_eq!(LayoutClass::for_size(119, 20), LayoutClass::Medium);
        assert_eq!(LayoutClass::for_size(90, 16), LayoutClass::Medium);
        assert_eq!(LayoutClass::for_size(89, 16), LayoutClass::Compact);
        assert_eq!(LayoutClass::for_size(40, 12), LayoutClass::Compact);
        assert_eq!(LayoutClass::for_size(39, 12), LayoutClass::TooSmall);
        assert_eq!(LayoutClass::for_size(40, 11), LayoutClass::TooSmall);
        assert_eq!(LayoutClass::for_size(120, 11), LayoutClass::TooSmall);
    }

    #[test]
    fn defaults_initialize_phase_3_fields_without_changing_phase_2_defaults() {
        let state = WorkbenchState::default();
        assert_eq!(state.mode, WorkbenchMode::Data);
        assert_eq!(state.workspace, Workspace::Tables);
        assert_eq!(state.focused_pane, WorkbenchPane::Explorer);
        assert_eq!(state.sql_input, "");
        assert_eq!(state.sql_history_cursor, None);
        assert_eq!(state.table_grid_cursor, None);
        assert_eq!(state.table_search, "");
        assert!(!state.palette_open);
        assert!(!state.help_open);
        assert!(!state.modal_open);
        assert_eq!(state.compact_surface, CompactSurface::Workspace);
        assert!(!state.inspector_open);
        assert!(state.activity_open);
        assert_eq!(state.workspace_scroll_offset, 0);
    }

    #[test]
    fn phase_2_workbench_fields_survive_phase_3_extension() {
        let mut state = WorkbenchState::default();
        state.sql_input = "select * from users".into();
        state.sql_history_cursor = Some(2);
        state.table_grid_cursor = Some((3, 4));
        state.table_search = "user".into();
        state.palette_open = true;
        state.help_open = false;
        state.modal_open = true;

        state.set_workspace(Workspace::Live);
        state.set_compact_surface(CompactSurface::Workspace);

        assert_eq!(state.sql_input, "select * from users");
        assert_eq!(state.sql_history_cursor, Some(2));
        assert_eq!(state.table_grid_cursor, Some((3, 4)));
        assert_eq!(state.table_search, "user");
        assert!(state.palette_open);
        assert!(!state.help_open);
        assert!(state.modal_open);
        assert_eq!(state.mode, WorkbenchMode::Observe);
        assert_eq!(state.workspace, Workspace::Live);
        assert_eq!(state.focused_pane, WorkbenchPane::Workspace);
    }

    #[test]
    fn workspace_scroll_offset_is_clamped_and_resets_on_workspace_change() {
        let mut state = WorkbenchState::default();
        state.scroll_workspace_by_pages(1, 15);
        assert_eq!(state.workspace_scroll_offset, 10);
        state.scroll_workspace_by_pages(1, 15);
        assert_eq!(state.workspace_scroll_offset, 15);
        state.scroll_workspace_by_pages(-3, 15);
        assert_eq!(state.workspace_scroll_offset, 0);
        state.scroll_workspace_by_pages(1, 15);
        state.set_workspace(Workspace::Logs);
        assert_eq!(state.workspace_scroll_offset, 0);
    }
}
```

- [ ] **GREEN: Extend existing exports and AppState only if Phase 2 has not already done so**

Phase 2 already owns `src/state/workbench.rs`, and `src/state/mod.rs` plus `AppState` should already declare it. Verify the existing exports first. If any export is missing, add only the missing item:

```rust
pub mod workbench;
pub use workbench::{CompactSurface, LayoutClass, WorkbenchMode, WorkbenchPane, WorkbenchState, Workspace};
```

If `src/state/app_state.rs` does not already contain the Phase 2 workbench field, add it:

```rust
pub workbench: crate::state::workbench::WorkbenchState,
```

If the field was missing and added, initialize it in `AppState::new`:

```rust
workbench: crate::state::workbench::WorkbenchState::default(),
```

- [ ] **GREEN: Run focused tests**

```bash
cargo test state::workbench -- --nocapture
```

Expected: all workbench tests pass and existing Phase 2 workbench tests still pass.

- [ ] **Commit boundary**

```bash
git add src/state/workbench.rs
git add src/state/mod.rs src/state/app_state.rs # only if this task actually changed them
git commit -m "feat: extend contextual workbench state"
```

---

### Task 3: Extend Phase 2 activity state and add shared state views

**Files:**
- Modify: `src/state/activity.rs`
- Create: `src/ui/states.rs`
- Create: `src/ui/styles.rs`
- Modify as needed only: `src/state/mod.rs`
- Modify: `src/ui/mod.rs`

- [ ] **RED: Add lifecycle state extension tests**

Extend the Phase 2-owned `src/state/activity.rs`. Do not replace the file wholesale and do not create a competing owner for activity state. Add missing lifecycle presentation helpers, bounded recent activity behavior, and tests only if Phase 2 did not already define them:

```rust
// Additive extension only. Preserve Phase 2 ActivityState fields such as
// active_reads, mutations, notices, connection, retry, and any existing fields.
// Do not store Loading/Refreshing/Stale/Offline/Retrying here as a second
// lifecycle authority. Resource lifecycle presentation is derived on demand
// from Phase 2 LoadState<T>, ConnectionState, and RetryState.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    pub label: String,
    pub outcome: ActivityOutcome,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityOutcome { Started, Succeeded, Failed, Cancelled }

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityState {
    pub active_reads: Vec<RequestId>,
    pub active_mutations: Vec<RequestId>,
    pub notices: Vec<String>,
    pub connection: ConnectionState,
    pub retry: RetryState,
    pub recent: Vec<ActivityItem>,
}

impl ActivityState {
    pub fn push_recent(&mut self, item: ActivityItem) {
        self.recent.push(item);
        if self.recent.len() > 50 { self.recent.remove(0); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_activity_outcome_log_is_bounded_without_copying_lifecycle_state() {
        let mut activity = ActivityState::default();
        for i in 0..60 {
            activity.push_recent(ActivityItem { label: format!("item {i}"), outcome: ActivityOutcome::Succeeded, detail: None });
        }
        assert_eq!(activity.recent.len(), 50);
        assert_eq!(activity.recent[0].label, "item 10");
    }

    #[test]
    fn phase_2_activity_fields_survive_phase_3_extension() {
        let mut activity = ActivityState::default();
        activity.active_reads.push(RequestId::from_u64(7));
        activity.active_mutations.push(RequestId::from_u64(8));
        activity.notices.push("schema refreshed".into());
        activity.connection = ConnectionState::Reconnecting;
        activity.retry = RetryState { attempt: 2, next_retry_label: Some("in 5s".into()) };

        activity.push_recent(ActivityItem { label: "refresh users".into(), outcome: ActivityOutcome::Started, detail: None });

        assert_eq!(activity.active_reads, vec![RequestId::from_u64(7)]);
        assert_eq!(activity.active_mutations, vec![RequestId::from_u64(8)]);
        assert_eq!(activity.notices, vec!["schema refreshed".to_string()]);
        assert_eq!(activity.connection, ConnectionState::Reconnecting);
        assert_eq!(activity.retry, RetryState { attempt: 2, next_retry_label: Some("in 5s".into()) });
    }
}
```

Create `src/ui/states.rs`:

```rust
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    config::ThemeColors,
    state::{activity::{ConnectionState, RetryState}, resources::{LoadState, StaleReason}},
    ui::styles::WorkbenchThemeExt,
};

pub struct LifecyclePresentation {
    pub title: &'static str,
    pub message: String,
    pub action: Option<&'static str>,
}

pub fn lifecycle_from_load_connection_retry<T>(
    load: &LoadState<T>,
    connection: &ConnectionState,
    retry: &RetryState,
) -> LifecyclePresentation {
    if *connection == ConnectionState::Disconnected {
        return LifecyclePresentation {
            title: "Offline",
            message: "Connection is offline. Cached data may be stale.".into(),
            action: Some("Reconnect when the server is available"),
        };
    }
    if retry.attempt > 0 {
        return LifecyclePresentation {
            title: "Retrying",
            message: retry.next_retry_label.clone().unwrap_or_else(|| format!("Retry attempt {} scheduled", retry.attempt)),
            action: Some("Waiting before retry"),
        };
    }
    match load {
        LoadState::Idle => LifecyclePresentation { title: "Ready", message: "No active request".into(), action: None },
        LoadState::Loading { request } => LifecyclePresentation { title: "Loading", message: format!("Loading {}…", request.scope()), action: None },
        LoadState::Ready { refreshed_at, .. } => LifecyclePresentation { title: "Ready", message: format!("Fresh at {refreshed_at:?}"), action: None },
        LoadState::Refreshing { request, .. } => LifecyclePresentation { title: "Refreshing", message: format!("Refreshing {}…", request.scope()), action: None },
        LoadState::Empty { refreshed_at } => LifecyclePresentation { title: "Empty", message: format!("No data returned at {refreshed_at:?}"), action: Some("Select another resource or refresh") },
        LoadState::Stale { reason, .. } => LifecyclePresentation { title: "Stale", message: format!("Cached data may be stale: {}", stale_reason_label(reason)), action: Some("Refresh to verify latest data") },
        LoadState::Error { error, previous } => LifecyclePresentation {
            title: "Error",
            message: if previous.is_some() { format!("{}; showing previous data", error.message) } else { error.message.clone() },
            action: Some("Open Activity for details or retry"),
        },
    }
}

fn stale_reason_label(reason: &StaleReason) -> &'static str {
    match reason {
        StaleReason::Offline => "offline",
        StaleReason::NewerGeneration => "newer generation superseded this request",
        StaleReason::BudgetExceeded => "refresh budget exceeded",
        StaleReason::ManualRefreshRequired => "manual refresh required",
    }
}

pub fn render_lifecycle_presentation(area: Rect, buf: &mut Buffer, view: LifecyclePresentation, theme: &ThemeColors) {
    if area.width == 0 || area.height == 0 { return; }
    let mut lines = vec![
        Line::from(Span::styled(view.title, theme.lifecycle_title_style())),
        Line::from(view.message),
    ];
    if let Some(action) = view.action {
        lines.push(Line::from(Span::styled(action, theme.lifecycle_action_style())));
    }
    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).border_style(theme.panel_border_style()))
        .alignment(Alignment::Center)
        .render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use crate::{
        effects::request::{RequestContext, RequestId, RequestScope},
        state::resources::AppError,
        ui::test_support::render_to_string,
    };

    fn request_context(table: &str) -> RequestContext {
        RequestContext::new(
            RequestId::from_u64(42),
            RequestScope::TableRows { database: "testdb".into(), table: table.to_string(), view: String::new() },
            7,
        )
    }

    fn fixed_instant() -> Instant { Instant::now() }

    fn app_error(message: &str) -> AppError { AppError { message: message.to_string() } }

    fn monochrome_theme() -> ThemeColors {
        let mut theme = ThemeColors::dark();
        theme.fg_secondary = theme.fg_primary;
        theme.fg_muted = theme.fg_primary;
        theme.accent = theme.fg_primary;
        theme.highlight = theme.fg_primary;
        theme.success = theme.fg_primary;
        theme.warning = theme.fg_primary;
        theme.error = theme.fg_primary;
        theme.info = theme.fg_primary;
        theme.border_normal = theme.fg_primary;
        theme.border_focused = theme.fg_primary;
        theme
    }

    #[test]
    fn lifecycle_presentation_covers_phase_2_states_without_activity_duplication() {
        let connection = ConnectionState::Connected;
        let retry = RetryState::default();
        let cases = [
            lifecycle_from_load_connection_retry(&LoadState::<Vec<String>>::Loading { request: request_context("rows/users") }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::Refreshing { data: vec!["alice".to_string()], request: request_context("rows/users") }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::Ready { data: vec!["alice".to_string()], refreshed_at: fixed_instant() }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::<Vec<String>>::Empty { refreshed_at: fixed_instant() }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::Stale { data: vec!["alice".to_string()], reason: StaleReason::Offline }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::<Vec<String>>::Error { previous: None, error: app_error("boom") }, &connection, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::Ready { data: vec!["cached".to_string()], refreshed_at: fixed_instant() }, &ConnectionState::Disconnected, &retry).title,
            lifecycle_from_load_connection_retry(&LoadState::Ready { data: vec!["cached".to_string()], refreshed_at: fixed_instant() }, &connection, &RetryState { attempt: 2, next_retry_label: Some("in 5s".into()) }).title,
        ];
        assert_eq!(cases, ["Loading", "Refreshing", "Ready", "Empty", "Stale", "Error", "Offline", "Retrying"]);
    }

    #[test]
    fn lifecycle_render_uses_theme_colors_for_light_dark_high_contrast_and_no_color() {
        let no_color = monochrome_theme();
        for theme in [ThemeColors::light(), ThemeColors::dark(), ThemeColors::high_contrast(), no_color] {
            let view = LifecyclePresentation { title: "Loading", message: "Loading data…".into(), action: None };
            let text = render_to_string(40, 12, |area, buf| render_lifecycle_presentation(area, buf, view, &theme));
            assert!(text.contains("Loading"));
        }
    }
}
```

Create `src/ui/styles.rs` for semantic theme adapters over existing `crate::config::ThemeColors`:

```rust
use ratatui::style::{Color, Modifier, Style};
use crate::config::ThemeColors;

pub trait WorkbenchThemeExt {
    fn panel_border_style(&self) -> Style;
    fn panel_border_style_for_focus(&self, focused: bool) -> Style;
    fn mode_bar_style(&self, active: bool) -> Style;
    fn lifecycle_title_style(&self) -> Style;
    fn lifecycle_action_style(&self) -> Style;
}

impl WorkbenchThemeExt for ThemeColors {
    fn panel_border_style(&self) -> Style { Style::default().fg(rgb(self.border_normal)) }
    fn panel_border_style_for_focus(&self, focused: bool) -> Style {
        Style::default().fg(rgb(if focused { self.border_focused } else { self.border_normal }))
    }
    fn mode_bar_style(&self, active: bool) -> Style {
        let style = Style::default().fg(rgb(if active { self.accent } else { self.fg_secondary }));
        if active { style.add_modifier(Modifier::BOLD) } else { style }
    }
    fn lifecycle_title_style(&self) -> Style { Style::default().fg(rgb(self.info)).add_modifier(Modifier::BOLD) }
    fn lifecycle_action_style(&self) -> Style { Style::default().fg(rgb(self.warning)) }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color { Color::Rgb(r, g, b) }
```

- [ ] **GREEN: Verify existing activity exports and add UI state view module**

Phase 2 already owns `src/state/activity.rs`, and `src/state/mod.rs` should already export its public types. Verify existing exports first. If any export is missing, add only the missing item:

```rust
pub mod activity;
pub use activity::{ActivityItem, ActivityOutcome, ActivityState, ConnectionState, RetryState};
```

In `src/ui/mod.rs`:

```rust
pub mod states;
pub mod styles;
```

- [ ] **GREEN: Run focused tests**

```bash
cargo test state::activity -- --nocapture
cargo test ui::states -- --nocapture
```

Expected: state and lifecycle render tests pass and existing Phase 2 activity tests still pass.

- [ ] **Commit boundary**

```bash
git add src/state/activity.rs src/ui/mod.rs src/ui/states.rs src/ui/styles.rs
git add src/state/mod.rs # only if this task actually changed exports
git commit -m "feat: extend activity lifecycle state views"
```

---

### Task 4: Compute and render responsive workbench shell

**Files:**
- Create: `src/ui/workbench.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/app.rs`

- [ ] **RED: Add failing workbench render tests**

Create `src/ui/workbench.rs`:

```rust
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::state::{AppState, CompactSurface, LayoutClass, WorkbenchPane};

#[derive(Debug, Clone, Copy)]
pub struct WorkbenchAreas {
    pub explorer: Option<Rect>,
    pub workspace: Option<Rect>,
    pub inspector: Option<Rect>,
    pub activity_bar: Rect,
    pub activity_panel: Option<Rect>,
    pub breadcrumb: Rect,
    pub mode_bar: Rect,
}

pub fn compute_workbench_areas(area: Rect, app: &AppState) -> WorkbenchAreas {
    let class = LayoutClass::for_size(area.width, area.height);
    match class {
        LayoutClass::TooSmall => WorkbenchAreas { explorer: None, workspace: None, inspector: None, activity_bar: area, activity_panel: None, breadcrumb: area, mode_bar: area },
        LayoutClass::Wide => {
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(3)])
                .split(area);
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(22), Constraint::Min(40), Constraint::Length(24)])
                .split(outer[2]);
            WorkbenchAreas { explorer: Some(body[0]), workspace: Some(body[1]), inspector: Some(body[2]), activity_bar: outer[3], activity_panel: Some(outer[3]), breadcrumb: outer[1], mode_bar: outer[0] }
        }
        LayoutClass::Medium => {
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
                .split(area);
            let body_constraints = if app.workbench.inspector_open {
                vec![Constraint::Length(22), Constraint::Min(32), Constraint::Length(22)]
            } else {
                vec![Constraint::Length(22), Constraint::Min(32)]
            };
            let body = Layout::default().direction(Direction::Horizontal).constraints(body_constraints).split(outer[2]);
            WorkbenchAreas { explorer: Some(body[0]), workspace: Some(body[1]), inspector: if app.workbench.inspector_open { Some(body[2]) } else { None }, activity_bar: outer[3], activity_panel: Some(outer[3]), breadcrumb: outer[1], mode_bar: outer[0] }
        }
        LayoutClass::Compact => {
            let outer = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
                .split(area);
            let primary = outer[2];
            WorkbenchAreas {
                explorer: (app.workbench.compact_surface == CompactSurface::Explorer).then_some(primary),
                workspace: (app.workbench.compact_surface == CompactSurface::Workspace).then_some(primary),
                inspector: (app.workbench.compact_surface == CompactSurface::Inspector).then_some(primary),
                activity_bar: outer[3],
                activity_panel: (app.workbench.compact_surface == CompactSurface::Activity).then_some(primary),
                breadcrumb: outer[1],
                mode_bar: outer[0],
            }
        }
    }
}

pub fn render_workbench(area: Rect, buf: &mut Buffer, app: &AppState) -> WorkbenchAreas {
    let class = LayoutClass::for_size(area.width, area.height);
    if class == LayoutClass::TooSmall {
        render_too_small(area, buf);
        return compute_workbench_areas(area, app);
    }
    let areas = compute_workbench_areas(area, app);
    render_mode_bar(areas.mode_bar, buf, app);
    render_breadcrumb(areas.breadcrumb, buf, app);
    if let Some(area) = areas.explorer { render_panel(area, buf, "Explorer", app.workbench.focused_pane == WorkbenchPane::Explorer); }
    if let Some(area) = areas.workspace { render_panel(area, buf, "Workspace", app.workbench.focused_pane == WorkbenchPane::Workspace); }
    if let Some(area) = areas.inspector { render_panel(area, buf, "Inspector", app.workbench.focused_pane == WorkbenchPane::Inspector); }
    render_activity_bar(areas.activity_bar, buf, app);
    if let Some(area) = areas.activity_panel { render_panel(area, buf, "Activity", app.workbench.focused_pane == WorkbenchPane::Activity); }
    areas
}

fn render_too_small(area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 { return; }
    Paragraph::new("terminal too small\nminimum 40x12")
        .block(Block::default().borders(Borders::ALL).title(" SpacetimeDB TUI "))
        .render(area, buf);
}

fn render_mode_bar(area: Rect, buf: &mut Buffer, app: &AppState) {
    let registry = crate::app::command::CommandRegistry::default();
    let mode_specs = [CommandId::ModeData, CommandId::ModeObserve, CommandId::ModeOperate]
        .into_iter()
        .filter_map(|id| registry.by_id(id));
    let palette_hint = registry
        .by_id(CommandId::OpenCommandPalette)
        .and_then(|spec| spec.bindings.iter().find(|binding| !binding.migration_alias))
        .map(|binding| binding.display)
        .unwrap_or("palette");
    let spans = mode_specs
        .map(|spec| Span::styled(format!(" {} ", spec.label), app.theme.mode_bar_style(spec.id == active_mode_command(app))))
        .chain(std::iter::once(Span::raw(format!("   {palette_hint} "))))
        .collect::<Vec<_>>();
    buf.set_line(area.x, area.y, &Line::from(spans), area.width);
}

fn render_breadcrumb(area: Rect, buf: &mut Buffer, app: &AppState) {
    let db = app.selected_database().unwrap_or("no database");
    let resource = app.selected_table().map(|t| t.table_name.as_str()).unwrap_or("no resource");
    let line = Line::from(format!("{} / {} / {}", app.workbench.mode_label(), db, resource));
    buf.set_line(area.x, area.y, &line, area.width);
}

fn render_panel(area: Rect, buf: &mut Buffer, title: &'static str, focused: bool, app: &AppState) {
    let style = app.theme.panel_border_style_for_focus(focused);
    Block::default().borders(Borders::ALL).border_style(style).title(title).render(area, buf);
}

trait ModeLabel {
    fn mode_label(&self) -> &'static str;
}

impl ModeLabel for crate::state::WorkbenchState {
    fn mode_label(&self) -> &'static str {
        match self.mode {
            crate::state::WorkbenchMode::Data => "DATA",
            crate::state::WorkbenchMode::Observe => "OBSERVE",
            crate::state::WorkbenchMode::Operate => "OPERATE",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, state::{CompactSurface, LayoutClass}, ui::test_support::render_to_string};

    fn app_state() -> AppState {
        AppState::new(Config::default().server_url)
    }

    #[test]
    fn wide_layout_shows_all_surfaces() {
        let app = app_state();
        let text = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
        assert!(text.contains("DATA"));
        assert!(text.contains("OBSERVE"));
        assert!(text.contains("OPERATE"));
        assert!(text.contains("Explorer"));
        assert!(text.contains("Workspace"));
        assert!(text.contains("Inspector"));
        assert!(text.contains("Activity"));
        let registry = crate::app::command::CommandRegistry::default();
        let palette = registry.by_id(crate::app::command::CommandId::OpenCommandPalette).expect("palette spec");
        assert!(palette.bindings.iter().any(|binding| text.contains(binding.display)));
    }

    #[test]
    fn too_small_renders_minimal_message() {
        for (w, h) in [(39, 12), (40, 11), (20, 5)] {
            let app = app_state();
            let text = render_to_string(w, h, |area, buf| { render_workbench(area, buf, &app); });
            assert!(text.contains("terminal too small"), "{w}x{h}: {text}");
        }
    }

    #[test]
    fn compact_shows_one_primary_surface() {
        let mut app = app_state();
        app.workbench.set_compact_surface(CompactSurface::Inspector);
        let text = render_to_string(40, 12, |area, buf| { render_workbench(area, buf, &app); });
        assert!(text.contains("Inspector"));
        assert!(!text.contains("Explorer"));
    }

    #[test]
    fn all_matrix_sizes_do_not_panic() {
        let app = app_state();
        for (w, h) in [(120,20), (119,20), (90,16), (89,16), (40,12), (39,12), (40,11), (20,5), (0,0)] {
            let _ = render_to_string(w, h, |area, buf| { render_workbench(area, buf, &app); });
        }
    }
}
```

- [ ] **GREEN: Export and wire shell into draw path**

In `src/ui/mod.rs`:

```rust
pub mod workbench;
```

In `src/app.rs::draw_frame`, initially call `render_workbench` before migrating inner panel content:

```rust
let areas = crate::ui::workbench::render_workbench(frame.area(), frame.buffer_mut(), state);
```

Keep existing tab renderers until Tasks 6-9 migrate content into areas.

- [ ] **GREEN: Run focused tests**

```bash
cargo test ui::workbench -- --nocapture
```

Expected: render tests pass at all matrix sizes.

- [ ] **Commit boundary**

```bash
git add src/ui/mod.rs src/ui/workbench.rs src/app.rs
git commit -m "feat: compute responsive workbench layout"
```

---

### Task 5: Add Explorer, Workspace, Inspector, and Activity surfaces

**Files:**
- Create: `src/ui/explorer.rs`
- Create: `src/ui/inspector.rs`
- Create: `src/ui/activity.rs`
- Modify: `src/ui/workbench.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/app.rs`

- [ ] **RED: Add render assertions for named surfaces**

Create `src/ui/explorer.rs`:

```rust
use ratatui::{buffer::Buffer, layout::Rect};
use crate::state::AppState;

pub fn render_explorer(area: Rect, buf: &mut Buffer, app: &AppState) {
    // Existing render_sidebar owns the Explorer border. Do not wrap it in a
    // second Block, or wide/medium renders get a double border.
    crate::ui::sidebar::render_sidebar(area, buf, app);
}
```

Create `src/ui/inspector.rs`:

```rust
use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::{Block, Borders, Paragraph, Widget}};
use crate::state::AppState;

pub fn render_inspector(area: Rect, buf: &mut Buffer, app: &AppState) {
    if area.width == 0 || area.height == 0 { return; }
    let db = app.selected_database().unwrap_or("no database");
    let table = app.selected_table().map(|t| t.table_name.as_str()).unwrap_or("no table");
    let columns = app.selected_table().map(|t| t.columns.len()).unwrap_or(0);
    let lines = vec![
        Line::from(format!("Database: {db}")),
        Line::from(format!("Resource: {table}")),
        Line::from(format!("Columns: {columns}")),
        Line::from("Safe actions"),
        Line::from("Disabled actions explain why"),
    ];
    Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Inspector ")).render(area, buf);
}
```

Create `src/ui/activity.rs`:

```rust
use ratatui::{buffer::Buffer, layout::Rect, text::Line, widgets::{Block, Borders, Paragraph, Widget}};
use crate::state::AppState;

pub fn render_activity(area: Rect, buf: &mut Buffer, app: &AppState) {
    if area.width == 0 || area.height == 0 { return; }
    // Read exact Phase 2 ConnectionState and RetryState fields or methods from
    // src/state/activity.rs before implementing this function. Do not invent
    // ConnectionState, retry.as_ref(), attempt, or seconds_remaining fields.
    let presentation = crate::ui::states::activity_presentation_from_phase_2(&app.activity, &app.resources);
    let lines = vec![
        Line::from(format!("Connection: {}", presentation.connection_label)),
        Line::from(format!("Lifecycle: {}", presentation.lifecycle_label)),
        Line::from(presentation.retry_label),
    ];
    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Activity ").border_style(app.theme.panel_border_style()))
        .render(area, buf);
}
```

Add tests in `src/ui/workbench.rs`:

```rust
#[test]
fn wide_render_includes_surface_content() {
    let app = app_state();
    let text = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(text.contains("Explorer"));
    assert!(text.contains("Workspace"));
    assert!(text.contains("Inspector"));
    assert!(text.contains("Activity"));
    assert!(text.contains("Safe actions"));
    assert!(text.contains("Connection:"));
}

#[test]
fn medium_inspector_is_drawer_controlled() {
    let mut app = app_state();
    app.workbench.inspector_open = false;
    let closed = render_to_string(90, 16, |area, buf| { render_workbench(area, buf, &app); });
    assert!(!closed.contains("Safe actions"));
    app.workbench.inspector_open = true;
    let open = render_to_string(90, 16, |area, buf| { render_workbench(area, buf, &app); });
    assert!(open.contains("Safe actions"));
}

#[test]
fn medium_and_compact_activity_bar_always_shows_connection_and_never_overlaps_primary_activity() {
    let mut app = app_state();
    for (w, h) in [(90, 16), (40, 12)] {
        let areas = compute_workbench_areas(Rect::new(0, 0, w, h), &app);
        assert!(areas.activity_bar.height >= 1);
        assert!(rects_do_not_overlap(areas.activity_bar, areas.workspace.unwrap_or_default()));
        let text = render_to_string(w, h, |area, buf| { render_workbench(area, buf, &app); });
        assert!(text.contains("Connection"), "activity bar missing at {w}x{h}: {text}");
    }
    app.workbench.set_compact_surface(CompactSurface::Activity);
    let areas = compute_workbench_areas(Rect::new(0, 0, 40, 12), &app);
    assert!(areas.activity_panel.is_some());
    assert!(rects_do_not_overlap(areas.activity_bar, areas.activity_panel.unwrap()));
}
```

- [ ] **GREEN: Use surface renderers from workbench**

In `src/ui/mod.rs`:

```rust
pub mod activity;
pub mod explorer;
pub mod inspector;
```

In `src/ui/workbench.rs`, replace placeholder panel rendering:

```rust
if let Some(area) = areas.explorer { crate::ui::explorer::render_explorer(area, buf, app); }
if let Some(area) = areas.workspace { render_panel(area, buf, " Workspace ", app.workbench.focused_pane == WorkbenchPane::Workspace); }
if let Some(area) = areas.inspector { crate::ui::inspector::render_inspector(area, buf, app); }
crate::ui::activity::render_activity_bar(areas.activity_bar, buf, app);
if let Some(area) = areas.activity_panel { crate::ui::activity::render_activity(area, buf, app); }
```

Then migrate existing tab renderers into Workspace in `src/app.rs::draw_frame` by rendering them into `areas.workspace`.

- [ ] **GREEN: Run focused render tests**

```bash
cargo test ui::workbench -- --nocapture
cargo test ui::inspector -- --nocapture
cargo test ui::activity -- --nocapture
```

Expected: surfaces render at wide, medium, compact, and too-small sizes.

- [ ] **Commit boundary**

```bash
git add src/ui/mod.rs src/ui/explorer.rs src/ui/inspector.rs src/ui/activity.rs src/ui/workbench.rs src/app.rs
git commit -m "feat: add workbench surfaces"
```

---

### Task 6: Extend Phase 2 command registry, palette, and generated help

**Files:**
- Modify: `src/app/command.rs`
- Modify: `src/state/palette.rs`
- Modify: `src/ui/components/palette.rs`
- Modify: `src/ui/components/help.rs`
- Modify: `src/app.rs`

- [ ] **RED: Add failing tests for Phase 3 command extensions in the exact Phase 2 registry**

Add tests to `src/app/command.rs`. Phase 2 owns `CommandRegistry`, `COMMAND_SPECS`, `CommandSpec { bindings, modes, focus: &'static [FocusContext], availability: AvailabilityRule }`, `Binding { key, display, migration_alias }`, `KeyChord`, `Availability`, and `DisabledReason`. Extend those exact types and APIs. Do not introduce alternative key-list fields, duplicate migration binding arrays on `CommandSpec`, or free-function registry lookup synonyms. Phase 2 already migrates legacy commands, so Phase 3 must add only genuinely missing workbench commands.

```rust
#[test]
fn phase_3_workbench_commands_are_in_command_specs() {
    let registry = CommandRegistry::default();
    for id in [
        CommandId::ModeData,
        CommandId::ModeObserve,
        CommandId::ModeOperate,
        CommandId::FocusExplorer,
        CommandId::FocusWorkspace,
        CommandId::FocusInspector,
        CommandId::FocusActivity,
        CommandId::ToggleInspector,
        CommandId::ToggleActivity,
        CommandId::ScrollWorkspaceUp,
        CommandId::ScrollWorkspaceDown,
    ] {
        assert!(registry.by_id(id).is_some(), "missing Phase 3 command {id:?}");
    }
}

#[test]
fn phase_3_primary_and_migration_bindings_resolve() {
    let registry = CommandRegistry::default();
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::Char('d'), modifiers: KeyModifiers::NONE }), Some(CommandId::ModeData));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::Char('o'), modifiers: KeyModifiers::NONE }), Some(CommandId::ModeObserve));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::Char('p'), modifiers: KeyModifiers::NONE }), Some(CommandId::ModeOperate));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::Char('t'), modifiers: KeyModifiers::NONE }), Some(CommandId::GotoTables));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::Char('1'), modifiers: KeyModifiers::NONE }), Some(CommandId::GotoTables));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::PageUp, modifiers: KeyModifiers::NONE }), Some(CommandId::ScrollWorkspaceUp));
    assert_eq!(registry.resolve_key(KeyChord { code: KeyCode::PageDown, modifiers: KeyModifiers::NONE }), Some(CommandId::ScrollWorkspaceDown));
}

#[test]
fn every_binding_has_one_owner_and_plain_ctrl_do_not_collide() {
    let registry = CommandRegistry::default();
    let mut owners = std::collections::HashMap::<KeyChord, CommandId>::new();
    for spec in registry.iter() {
        for binding in spec.bindings {
            assert_eq!(owners.insert(binding.key, spec.id), None, "binding {} has multiple owners", binding.display);
        }
    }
    assert_ne!(KeyChord { code: KeyCode::Char('p'), modifiers: KeyModifiers::NONE }, KeyChord::ctrl('p'));
    assert!(owners.iter().any(|(key, owner)| *key == KeyChord { code: KeyCode::Char('o'), modifiers: KeyModifiers::NONE } && *owner == CommandId::ModeObserve));
}

#[test]
fn migration_alias_help_uses_binding_flag() {
    let registry = CommandRegistry::default();
    let migration_bindings: Vec<&Binding> = registry
        .iter()
        .flat_map(|spec| spec.bindings.iter())
        .filter(|binding| binding.migration_alias)
        .collect();
    assert!(!migration_bindings.is_empty(), "Phase 2 legacy migration bindings should be flagged on bindings");
    assert!(migration_bindings.iter().any(|binding| binding.display == "1"));
}
```

- [ ] **GREEN: Extend `src/app/command.rs` without duplicating Phase 2 specs**

Add missing Phase 3 `CommandId` variants only if Phase 2 did not already add them. Extend `COMMAND_SPECS` with the exact Phase 2 shape. Each added `CommandSpec` must use `bindings: &[Binding { key: KeyChord::..., display: ..., migration_alias: ... }]`, Phase 2 modes/focus fields, and Phase 2 `AvailabilityRule`. Do not add duplicate specs for legacy commands Phase 2 already migrated.

Add these exact specs and binding constants to `src/app/command.rs`:

```rust
const MODE_DATA_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('d'), modifiers: KeyModifiers::NONE }, display: "d", migration_alias: false }];
const MODE_OBSERVE_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('o'), modifiers: KeyModifiers::NONE }, display: "o", migration_alias: false }];
const MODE_OPERATE_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('p'), modifiers: KeyModifiers::NONE }, display: "p", migration_alias: false }];
const FOCUS_EXPLORER_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('e'), modifiers: KeyModifiers::ALT }, display: "Alt+E", migration_alias: false }];
const FOCUS_WORKSPACE_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('w'), modifiers: KeyModifiers::ALT }, display: "Alt+W", migration_alias: false }];
const FOCUS_INSPECTOR_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('i'), modifiers: KeyModifiers::ALT }, display: "Alt+I", migration_alias: false }];
const FOCUS_ACTIVITY_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('a'), modifiers: KeyModifiers::ALT }, display: "Alt+A", migration_alias: false }];
const TOGGLE_INSPECTOR_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('i'), modifiers: KeyModifiers::CONTROL }, display: "Ctrl+I", migration_alias: false }];
const TOGGLE_ACTIVITY_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('a'), modifiers: KeyModifiers::CONTROL }, display: "Ctrl+A", migration_alias: false }];
const SCROLL_WORKSPACE_UP_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::PageUp, modifiers: KeyModifiers::NONE }, display: "PageUp", migration_alias: false },
    Binding { key: KeyChord::ctrl('u'), display: "Ctrl+U", migration_alias: false },
];
const SCROLL_WORKSPACE_DOWN_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::PageDown, modifiers: KeyModifiers::NONE }, display: "PageDown", migration_alias: false },
    Binding { key: KeyChord::ctrl('d'), display: "Ctrl+D", migration_alias: false },
];
const GOTO_TABLES_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('t'), modifiers: KeyModifiers::NONE }, display: "t", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('1'), modifiers: KeyModifiers::NONE }, display: "1", migration_alias: true },
];
const GOTO_SQL_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('s'), modifiers: KeyModifiers::NONE }, display: "s", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('2'), modifiers: KeyModifiers::NONE }, display: "2", migration_alias: true },
];
const GOTO_LOGS_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('g'), modifiers: KeyModifiers::NONE }, display: "g", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('3'), modifiers: KeyModifiers::NONE }, display: "3", migration_alias: true },
];
const GOTO_METRICS_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('m'), modifiers: KeyModifiers::NONE }, display: "m", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('4'), modifiers: KeyModifiers::NONE }, display: "4", migration_alias: true },
];
const GOTO_MODULE_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('b'), modifiers: KeyModifiers::NONE }, display: "b", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('5'), modifiers: KeyModifiers::NONE }, display: "5", migration_alias: true },
];
const GOTO_LIVE_BINDINGS: &[Binding] = &[
    Binding { key: KeyChord { code: KeyCode::Char('v'), modifiers: KeyModifiers::NONE }, display: "v", migration_alias: false },
    Binding { key: KeyChord { code: KeyCode::Char('6'), modifiers: KeyModifiers::NONE }, display: "6", migration_alias: true },
];

// Add these new specs, and replace the existing GotoTables/GotoSql/GotoLogs/
// GotoMetrics/GotoModule/GotoLive specs so they use these exact binding consts.
CommandSpec { id: CommandId::ModeData, label: "DATA mode", description: "Switch to DATA mode", bindings: MODE_DATA_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::ModeObserve, label: "OBSERVE mode", description: "Switch to OBSERVE mode", bindings: MODE_OBSERVE_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::ModeOperate, label: "OPERATE mode", description: "Switch to OPERATE mode", bindings: MODE_OPERATE_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::FocusExplorer, label: "Focus Explorer", description: "Move focus to Explorer", bindings: FOCUS_EXPLORER_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::FocusWorkspace, label: "Focus Workspace", description: "Move focus to Workspace", bindings: FOCUS_WORKSPACE_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::FocusInspector, label: "Focus Inspector", description: "Move focus to Inspector", bindings: FOCUS_INSPECTOR_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::FocusActivity, label: "Focus Activity", description: "Move focus to Activity", bindings: FOCUS_ACTIVITY_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::ToggleInspector, label: "Toggle Inspector", description: "Open or close the Inspector drawer", bindings: TOGGLE_INSPECTOR_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::ToggleActivity, label: "Toggle Activity", description: "Open or close the Activity surface", bindings: TOGGLE_ACTIVITY_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
CommandSpec { id: CommandId::ScrollWorkspaceUp, label: "Scroll workspace up", description: "Scroll the focused workspace upward", bindings: SCROLL_WORKSPACE_UP_BINDINGS, modes: ALL_MODES, focus: &[FocusContext::Workspace], availability: AvailabilityRule::WorkspaceScrollable },
CommandSpec { id: CommandId::ScrollWorkspaceDown, label: "Scroll workspace down", description: "Scroll the focused workspace downward", bindings: SCROLL_WORKSPACE_DOWN_BINDINGS, modes: ALL_MODES, focus: &[FocusContext::Workspace], availability: AvailabilityRule::WorkspaceScrollable },
CommandSpec { id: CommandId::GotoTables, label: "Go to tables", description: "Show table resources.", bindings: GOTO_TABLES_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
CommandSpec { id: CommandId::GotoSql, label: "Go to SQL", description: "Show the SQL workspace.", bindings: GOTO_SQL_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
CommandSpec { id: CommandId::GotoLogs, label: "Go to logs", description: "Show log output.", bindings: GOTO_LOGS_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
CommandSpec { id: CommandId::GotoMetrics, label: "Go to metrics", description: "Show metrics output.", bindings: GOTO_METRICS_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
CommandSpec { id: CommandId::GotoModule, label: "Go to module", description: "Show module details.", bindings: GOTO_MODULE_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
CommandSpec { id: CommandId::GotoLive, label: "Go to live", description: "Show the scoped live resource workspace.", bindings: GOTO_LIVE_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresLiveAvailable },
```

For DATA/OBSERVE/OPERATE and focus commands, add only missing specs and use the same Phase 2 `Binding`, `KeyChord`, modes, focus, and availability conventions. Legacy one-release keys must be represented only as `Binding { migration_alias: true, .. }` on the owning Phase 2 spec.

- [ ] **GREEN: Update palette and help to read Phase 2 registry methods**

Update `src/state/palette.rs` so `CommandPalette::filter` uses `CommandRegistry::default().search_palette(query)` matching the Phase 2 `search_palette(&str) -> Vec<CommandId>` API. This preserves the existing fuzzy subsequence palette UX while returning `Vec<crate::app::command::CommandId>`. Keep the current empty-query returns all commands, SQL fuzzy match, and no-match tests, but assert against `CommandId` values. Palette code should use registry `by_id` only after `search_palette` returns ids.

Update `src/ui/components/palette.rs` to render command labels, descriptions, binding display strings, and disabled reasons via Phase 2 `CommandRegistry` APIs. Do not manually read invented key-list fields.

Update `src/ui/components/help.rs` to generate help rows from `CommandRegistry::default().help_lines()`. The migration section must be derived from help lines or registry iteration by filtering `spec.bindings.iter().filter(|binding| binding.migration_alias)` and rendering each `binding.display`. The standard key section must filter `migration_alias == false`.

- [ ] **GREEN: Run focused tests**

```bash
cargo test app::command -- --nocapture
cargo test state::palette -- --nocapture
cargo test ui::components::help -- --nocapture
```

Expected: existing Phase 2 command tests still pass, Phase 3 command extensions pass, palette filtering uses `CommandRegistry`, and generated help contains migration bindings by filtering `Binding::migration_alias` plus scroll parity entries from real `KeyChord` bindings.

- [ ] **Commit boundary**

```bash
git add src/app/command.rs src/state/palette.rs src/ui/components/palette.rs src/ui/components/help.rs src/app.rs
git commit -m "feat: extend command registry for workbench"
```

---

### Task 7: Route keyboard, palette, help, migration bindings, and commands through Phase 2 reducer path

**Files:**
- Modify: `src/app.rs`
- Modify: `src/app/command.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/app/policy.rs`
- Modify: `src/state/workbench.rs`

- [ ] **RED: Add reducer-path equivalence tests for command invocation adapters**

Phase 2 established the exact architecture `AppEvent::Action(Action::Invoke(CommandId)) -> reduce(&mut AppState, AppEvent) -> Transition { effects }`. Phase 3 must not reintroduce direct `App` mutation or side-effect calls from keyboard, palette, help, migration bindings, or mouse. Input adapters may know where a command came from for logging, but they must normalize to the same `Action::Invoke(command)` value and then call the same reducer path.

```rust
fn app_state_fixture() -> AppState {
    let mut state = AppState::new("http://localhost:3000".to_string());
    state.workbench.mode = WorkbenchMode::Data;
    state.workbench.workspace = Workspace::Tables;
    state.workbench.focused_pane = WorkbenchPane::Workspace;
    state.workbench.inspector_open = false;
    state.workbench.activity_open = true;
    state.workbench.workspace_scroll_offset = 0;
    state
}

fn observable_workbench_state(state: &AppState) -> (WorkbenchMode, Workspace, WorkbenchPane, bool, bool, u16) {
    (state.workbench.mode, state.workbench.workspace, state.workbench.focused_pane, state.workbench.inspector_open, state.workbench.activity_open, state.workbench.workspace_scroll_offset)
}

fn assert_same_starting_state(left: &AppState, right: &AppState) {
    assert_eq!(observable_workbench_state(left), observable_workbench_state(right));
    assert_eq!(left.selected_database(), right.selected_database());
    assert_eq!(left.selected_table().map(|t| &t.table_name), right.selected_table().map(|t| &t.table_name));
}

fn action_from_key(registry: &CommandRegistry, key: KeyChord) -> Option<Action> {
    registry.resolve_key(key).map(Action::Invoke)
}

fn action_from_palette_selection(command: CommandId) -> Action { Action::Invoke(command) }
fn action_from_hit(command: CommandId) -> Action { Action::Invoke(command) }

#[test]
fn keyboard_palette_and_mouse_adapters_normalize_to_same_action() {
    let registry = CommandRegistry::default();
    let keyboard_action: Action = action_from_key(&registry, KeyChord { code: KeyCode::Char('o'), modifiers: KeyModifiers::NONE }).expect("keyboard command");
    let palette_action: Action = action_from_palette_selection(CommandId::ModeObserve);
    let mouse_action: Action = action_from_hit(CommandId::ModeObserve);

    assert_eq!(keyboard_action, Action::Invoke(CommandId::ModeObserve));
    assert_eq!(palette_action, keyboard_action);
    assert_eq!(mouse_action, keyboard_action);
}

#[test]
fn same_normalized_action_produces_same_transition_for_each_adapter() {
    let keyboard_action = Action::Invoke(CommandId::ModeObserve);
    let palette_action = Action::Invoke(CommandId::ModeObserve);
    let mouse_action = Action::Invoke(CommandId::ModeObserve);

    let mut keyboard_state = app_state_fixture();
    let mut palette_state = app_state_fixture();
    let mut mouse_state = app_state_fixture();

    assert_same_starting_state(&keyboard_state, &palette_state);
    assert_same_starting_state(&keyboard_state, &mouse_state);

    let keyboard: Transition = reduce(&mut keyboard_state, AppEvent::Action(keyboard_action));
    let palette: Transition = reduce(&mut palette_state, AppEvent::Action(palette_action));
    let mouse: Transition = reduce(&mut mouse_state, AppEvent::Action(mouse_action));

    assert_eq!(keyboard, palette);
    assert_eq!(keyboard, mouse);
    assert_eq!(keyboard.effects, palette.effects);
    assert_eq!(keyboard.effects, mouse.effects);
    assert_eq!(observable_workbench_state(&keyboard_state), observable_workbench_state(&palette_state));
    assert_eq!(observable_workbench_state(&keyboard_state), observable_workbench_state(&mouse_state));
}

fn rows_fixture_with_visible_len(len: usize) -> crate::api::types::QueryResult {
    crate::api::types::QueryResult { columns: vec!["id".into()], rows: (0..len).map(|i| vec![i.to_string()]).collect() }
}

fn fixed_instant() -> std::time::Instant { std::time::Instant::now() }

#[test]
fn disabled_availability_blocks_reducer_mutation_and_effects() {
    let mut state = app_state_fixture();
    state.workbench.focused_pane = WorkbenchPane::Workspace;
    state.resources.table_rows = LoadState::Ready { data: rows_fixture_with_visible_len(5), refreshed_at: fixed_instant() };
    let before = observable_workbench_state(&state);
    let transition = reduce(&mut state, AppEvent::Action(Action::Invoke(CommandId::ScrollWorkspaceDown)));
    assert_eq!(transition, Transition { effects: Vec::new() });
    assert_eq!(observable_workbench_state(&state), before);
}

#[test]
fn keychord_migration_binding_and_primary_binding_resolve_to_same_command() {
    let registry = CommandRegistry::default();
    let primary = registry.resolve_key(KeyChord { code: KeyCode::Char('t'), modifiers: KeyModifiers::NONE });
    let migration = registry.resolve_key(KeyChord { code: KeyCode::Char('1'), modifiers: KeyModifiers::NONE });
    assert_eq!(primary, migration);
    assert_eq!(primary, Some(CommandId::GotoTables));
}
```

- [ ] **GREEN: Extend reducer/action/effect handling for new workbench commands**

Add missing Phase 3 commands to Phase 2 reducer handling. Commands must enter as `AppEvent::Action(Action::Invoke(command))`, mutate state only inside `reduce(&mut AppState, AppEvent)`, and return the exact Phase 2 `Transition { effects }` type.

```diff
 fn invoke_command(state: &mut AppState, command: CommandId) -> Transition {
     match command {
+        CommandId::ModeData => { set_workbench_mode(state, WorkbenchMode::Data); Transition::none() }
+        CommandId::ModeObserve => { set_workbench_mode(state, WorkbenchMode::Observe); Transition::none() }
+        CommandId::ModeOperate => { set_workbench_mode(state, WorkbenchMode::Operate); Transition::none() }
+        CommandId::FocusExplorer => { set_workbench_focus(state, WorkbenchPane::Explorer); Transition::none() }
+        CommandId::FocusWorkspace => { set_workbench_focus(state, WorkbenchPane::Workspace); Transition::none() }
+        CommandId::FocusInspector => { state.workbench.inspector_open = true; set_workbench_focus(state, WorkbenchPane::Inspector); Transition::none() }
+        CommandId::FocusActivity => { state.workbench.activity_open = true; set_workbench_focus(state, WorkbenchPane::Activity); Transition::none() }
+        CommandId::ToggleInspector => { state.workbench.inspector_open = !state.workbench.inspector_open; if !state.workbench.inspector_open && state.workbench.focused_pane == WorkbenchPane::Inspector { set_workbench_focus(state, WorkbenchPane::Workspace); } Transition::none() }
+        CommandId::ToggleActivity => { state.workbench.activity_open = !state.workbench.activity_open; if !state.workbench.activity_open && state.workbench.focused_pane == WorkbenchPane::Activity { set_workbench_focus(state, WorkbenchPane::Workspace); } Transition::none() }
+        CommandId::ScrollWorkspaceUp => { scroll_workspace(state, -1); Transition::none() }
+        CommandId::ScrollWorkspaceDown => { scroll_workspace(state, 1); Transition::none() }
         CommandId::RefreshActiveResource => { /* existing Phase 2 arm unchanged */ }
         CommandId::RefreshCurrentView => { /* existing Phase 2 arm unchanged */ }
         _ => Transition::none(),
     }
 }
+
+fn set_workbench_mode(state: &mut AppState, mode: WorkbenchMode) {
+    state.workbench.mode = mode;
+    state.workbench.workspace = match mode { WorkbenchMode::Data => Workspace::Tables, WorkbenchMode::Observe => Workspace::Logs, WorkbenchMode::Operate => Workspace::Module };
+    state.workbench.compact_surface = CompactSurface::Workspace;
+    state.workbench.focused_pane = WorkbenchPane::Workspace;
+    state.workbench.workspace_scroll_offset = 0;
+}
+
+fn set_workbench_focus(state: &mut AppState, pane: WorkbenchPane) {
+    state.workbench.focused_pane = pane;
+    state.workbench.compact_surface = match pane { WorkbenchPane::Explorer => CompactSurface::Explorer, WorkbenchPane::Workspace => CompactSurface::Workspace, WorkbenchPane::Inspector => CompactSurface::Inspector, WorkbenchPane::Activity => CompactSurface::Activity };
+}
+
+fn scroll_workspace(state: &mut AppState, pages: i16) {
+    state.workbench.scroll_workspace_by_pages(pages, workspace_scroll_max_offset(state));
+}
+
+fn workspace_scroll_max_offset(state: &AppState) -> u16 {
+    workspace_row_count(state).saturating_sub(1).try_into().unwrap_or(u16::MAX)
+}
+
+fn workspace_row_count(state: &AppState) -> usize {
+    match state.workbench.workspace {
+        Workspace::Tables => match &state.resources.table_rows { LoadState::Ready { data, .. } | LoadState::Refreshing { data, .. } | LoadState::Stale { data, .. } => data.rows.len(), LoadState::Error { previous: Some(data), .. } => data.rows.len(), _ => 0 },
+        Workspace::Logs => match &state.resources.logs { LoadState::Ready { data, .. } | LoadState::Refreshing { data, .. } | LoadState::Stale { data, .. } => data.len(), LoadState::Error { previous: Some(data), .. } => data.len(), _ => 0 },
+        Workspace::Metrics => match &state.resources.metrics { LoadState::Ready { data, .. } | LoadState::Refreshing { data, .. } | LoadState::Stale { data, .. } => data.series_count(), LoadState::Error { previous: Some(data), .. } => data.series_count(), _ => 0 },
+        Workspace::Live => match &state.resources.live_clients { LoadState::Ready { data, .. } | LoadState::Refreshing { data, .. } | LoadState::Stale { data, .. } => data.len(), LoadState::Error { previous: Some(data), .. } => data.len(), _ => 0 },
+        Workspace::Sql | Workspace::Module => 0,
+    }
+}
```

Add exact scroll availability extensions in `src/app/command.rs`:

```rust
pub enum DisabledReason {
    // existing variants preserved
    WorkspaceNotScrollable,
}

pub enum AvailabilityRule {
    // existing variants preserved
    WorkspaceScrollable,
}

pub struct CommandContext {
    // existing fields preserved
    pub workspace_scrollable: bool,
}

pub fn command_context_from_state(state: &AppState) -> CommandContext {
    CommandContext {
        mode: state.workbench.mode,
        focus: focus_context_from_pane(state.workbench.focused_pane),
        has_active_database: state.navigation.active_database.is_some(),
        has_active_resource: state.navigation.active_resource.is_some(),
        schema_current: matches!(state.resources.schema, LoadState::Ready { .. } | LoadState::Refreshing { .. }),
        connection_online: state.activity.connection == ConnectionState::Connected,
        live_available: false,
        write_plan_available: !state.safety.pending_write_plans.is_empty(),
        workspace_scrollable: workspace_scroll_max_offset(state) > 0,
    }
}

fn focus_context_from_pane(pane: WorkbenchPane) -> FocusContext {
    match pane {
        WorkbenchPane::Explorer => FocusContext::Explorer,
        WorkbenchPane::Workspace => FocusContext::Workspace,
        WorkbenchPane::Inspector => FocusContext::Inspector,
        WorkbenchPane::Activity => FocusContext::Activity,
    }
}

pub fn evaluate_availability(rule: AvailabilityRule, context: &CommandContext) -> Availability {
    match rule {
        AvailabilityRule::Always => Availability::Available,
        AvailabilityRule::RequiresActiveDatabase if !context.has_active_database => Availability::Disabled(DisabledReason::NoActiveDatabase),
        AvailabilityRule::RequiresActiveResource if !context.has_active_resource => Availability::Disabled(DisabledReason::NoActiveResource),
        AvailabilityRule::RequiresOnline if !context.connection_online => Availability::Disabled(DisabledReason::Offline),
        AvailabilityRule::RequiresLiveAvailable if !context.live_available => Availability::Disabled(DisabledReason::LiveUnavailable),
        AvailabilityRule::RequiresWritePlan if !context.write_plan_available => Availability::Disabled(DisabledReason::WritePlanUnavailable),
        AvailabilityRule::WorkspaceScrollable if !context.workspace_scrollable => Availability::Disabled(DisabledReason::WorkspaceNotScrollable),
        _ => Availability::Available,
    }
}

#[test]
fn workspace_scroll_commands_disable_when_workspace_cannot_scroll() {
    let registry = CommandRegistry::default();
    let mut context = CommandContext {
        mode: WorkbenchMode::Data,
        focus: FocusContext::Workspace,
        has_active_database: true,
        has_active_resource: true,
        schema_current: true,
        connection_online: true,
        live_available: false,
        write_plan_available: false,
        workspace_scrollable: false,
    };
    assert_eq!(registry.availability(CommandId::ScrollWorkspaceDown, &context), Availability::Disabled(DisabledReason::WorkspaceNotScrollable));
    context.workspace_scrollable = true;
    assert_eq!(registry.availability(CommandId::ScrollWorkspaceDown, &context), Availability::Available);
}
```

Do not call side-effect methods or directly assign application state from input handlers.

- [ ] **GREEN: Normalize all input routes to `AppEvent::Action(Action::Invoke(command))`**

Update `src/app.rs` input handlers so they only normalize input to a command with `CommandRegistry::default().resolve_key(KeyChord)` and submit it to the Phase 2 event path. Production adapters should return `Action`, not mutate App state directly:

```rust
fn action_from_key(registry: &CommandRegistry, key_chord: KeyChord) -> Option<Action> {
    registry.resolve_key(key_chord).map(Action::Invoke)
}

fn action_from_palette_selection(command: CommandId) -> Action {
    Action::Invoke(command)
}

fn action_from_hit(command: CommandId) -> Action {
    Action::Invoke(command)
}

if let Some(command) = registry.resolve_key(key_chord) {
    self.submit_event(AppEvent::Action(Action::Invoke(command))).await;
    return;
}
```

Palette selection must call the same path:

```rust
self.submit_event(AppEvent::Action(Action::Invoke(command))).await;
```

Mouse hit testing must call the same path:

```rust
self.submit_event(AppEvent::Action(Action::Invoke(command))).await;
```

Help must not execute commands directly. It may display registry-derived bindings and disabled reasons only.

- [ ] **GREEN: Run focused tests**

```bash
cargo test app::command -- --nocapture
cargo test app::event -- --nocapture
cargo test app::reducer -- --nocapture
cargo test app::policy -- --nocapture
cargo test app:: -- --nocapture
```

Expected: command mapping tests pass, adapter normalization tests prove keyboard/palette/mouse produce the same `Action::Invoke(command)` and wrap it in `AppEvent::Action(action)` only at the reducer boundary, independent fixture states produce identical `Transition { effects }`, and disabled availability blocks invocation before reducer mutation or effects.

- [ ] **Commit boundary**

```bash
git add src/app.rs src/app/command.rs src/app/event.rs src/app/reducer.rs src/app/policy.rs src/state/workbench.rs
git commit -m "feat: route workbench commands through reducer"
```

---

### Task 8: Add precise pure mouse hit testing that emits registered commands

**Files:**
- Modify: `src/state/workbench.rs`
- Modify: `src/ui/workbench.rs`
- Modify: `src/app.rs`
- Modify: `src/app/command.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/app/policy.rs`

- [ ] **RED: Add precise control-rect, overlap, and boundary tests**

Place neutral widget bookkeeping in `src/state/workbench.rs`. `HitMap` and `HitRegion` are UI hit-test bookkeeping only; they must not encode domain state transitions and rendering must not mutate domain state. `WorkbenchAreas` must expose exact clickable control rects, not coarse bars that shadow controls.

```rust
#[derive(Debug, Clone, Copy)]
pub struct WorkbenchAreas {
    pub data_button: Option<Rect>,
    pub observe_button: Option<Rect>,
    pub operate_button: Option<Rect>,
    pub explorer_header: Option<Rect>,
    pub explorer_body: Option<Rect>,
    pub workspace_header: Option<Rect>,
    pub workspace_body: Option<Rect>,
    pub inspector_header: Option<Rect>,
    pub inspector_body: Option<Rect>,
    pub inspector_toggle: Option<Rect>,
    pub activity_header: Option<Rect>,
    pub activity_body: Option<Rect>,
    pub activity_toggle: Option<Rect>,
    pub compact_explorer_switch: Option<Rect>,
    pub compact_workspace_switch: Option<Rect>,
    pub compact_inspector_switch: Option<Rect>,
    pub compact_activity_switch: Option<Rect>,
    pub breadcrumb: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitPriority {
    Control,
    Header,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    pub rect: Rect,
    pub command: CommandId,
    pub priority: HitPriority,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HitMap {
    pub regions: Vec<HitRegion>,
}

impl HitMap {
    pub fn try_push(&mut self, region: HitRegion) -> Result<(), HitMapError> {
        if region.rect.width == 0 || region.rect.height == 0 {
            return Ok(());
        }
        if let Some(existing) = self.regions.iter().find(|existing| overlaps(existing.rect, region.rect) && existing.priority == region.priority) {
            return Err(HitMapError::Shadowed { existing: existing.command, new: region.command });
        }
        self.regions.push(region);
        self.regions.sort_by_key(|region| match region.priority { HitPriority::Control => 0, HitPriority::Header => 1, HitPriority::Body => 2 });
        Ok(())
    }

    pub fn command_at(&self, x: u16, y: u16) -> Option<CommandId> {
        self.regions.iter().find(|region| contains(region.rect, x, y)).map(|region| region.command)
    }
}
```

Add tests:

```rust
#[test]
fn mode_buttons_resolve_to_distinct_commands_without_shadowing() {
    let app = app_state_fixture_at(120, 20);
    let areas = compute_workbench_areas(Rect::new(0, 0, 120, 20), &app);
    let hit_map = build_hit_map(areas, &app.workbench).expect("hit map");
    assert_eq!(center_command(&hit_map, areas.data_button), Some(CommandId::ModeData));
    assert_eq!(center_command(&hit_map, areas.observe_button), Some(CommandId::ModeObserve));
    assert_eq!(center_command(&hit_map, areas.operate_button), Some(CommandId::ModeOperate));
}

#[test]
fn inspector_toggle_is_not_shadowed_by_inspector_focus_region() {
    let mut app = app_state_fixture_at(90, 16);
    app.workbench.inspector_open = true;
    let areas = compute_workbench_areas(Rect::new(0, 0, 90, 16), &app);
    let hit_map = build_hit_map(areas, &app.workbench).expect("hit map");
    assert_eq!(center_command(&hit_map, areas.inspector_toggle), Some(CommandId::ToggleInspector));
    assert_eq!(center_command(&hit_map, areas.inspector_body), Some(CommandId::FocusInspector));
}

#[test]
fn all_layout_classes_have_boundary_coordinate_coverage() {
    for (w, h) in [(120, 20), (90, 16), (40, 12), (39, 12), (0, 0)] {
        let app = app_state_fixture_at(w, h);
        let areas = compute_workbench_areas(Rect::new(0, 0, w, h), &app);
        let hit_map = build_hit_map(areas, &app.workbench).expect("hit map");
        assert_no_saturating_rect_panics(&hit_map, w, h);
    }
}
```

- [ ] **GREEN: Build hit map from explicit controls with deterministic overlap handling**

In `src/ui/workbench.rs`, add or update pure functions:

```rust
pub fn build_hit_map(areas: WorkbenchAreas, state: &crate::state::WorkbenchState) -> Result<crate::state::workbench::HitMap, HitMapError> {
    use crate::app::command::CommandId;
    use crate::state::workbench::{HitMap, HitPriority, HitRegion};

    let mut map = HitMap::default();
    push(&mut map, areas.data_button, CommandId::ModeData, HitPriority::Control)?;
    push(&mut map, areas.observe_button, CommandId::ModeObserve, HitPriority::Control)?;
    push(&mut map, areas.operate_button, CommandId::ModeOperate, HitPriority::Control)?;
    push(&mut map, areas.explorer_body, CommandId::FocusExplorer, HitPriority::Body)?;
    push(&mut map, areas.workspace_body, CommandId::FocusWorkspace, HitPriority::Body)?;
    push(&mut map, areas.inspector_body, CommandId::FocusInspector, HitPriority::Body)?;
    push(&mut map, areas.activity_body, CommandId::FocusActivity, HitPriority::Body)?;
    push(&mut map, areas.inspector_toggle, CommandId::ToggleInspector, HitPriority::Control)?;
    push(&mut map, areas.activity_toggle, CommandId::ToggleActivity, HitPriority::Control)?;
    push(&mut map, areas.compact_explorer_switch, CommandId::FocusExplorer, HitPriority::Control)?;
    push(&mut map, areas.compact_workspace_switch, CommandId::FocusWorkspace, HitPriority::Control)?;
    push(&mut map, areas.compact_inspector_switch, CommandId::FocusInspector, HitPriority::Control)?;
    push(&mut map, areas.compact_activity_switch, CommandId::FocusActivity, HitPriority::Control)?;
    let _ = state;
    Ok(map)
}

fn push(map: &mut HitMap, rect: Option<Rect>, command: CommandId, priority: HitPriority) -> Result<(), HitMapError> {
    if let Some(rect) = rect {
        map.try_push(HitRegion { rect, command, priority })?;
    }
    Ok(())
}
```

A control may overlap a body only when its priority is higher and tests prove the intended command wins. Same-priority overlap must return `HitMapError::Shadowed` so no visible control is silently unreachable.

- [ ] **GREEN: Scope mouse wheel to scrollable workspace region and reducer path**

Mouse wheel must emit `ScrollWorkspaceUp` or `ScrollWorkspaceDown` only when the pointer is over `areas.workspace_body` and the command is available. Else it emits no workspace scroll command. It still uses `AppEvent::Action(Action::Invoke(command))` and the reducer/policy path from Task 7.

```rust
let command = match mouse.kind {
    MouseEventKind::Down(MouseButton::Left) => hit_map.command_at(mouse.column, mouse.row),
    MouseEventKind::ScrollUp if rect_contains(areas.workspace_body, mouse.column, mouse.row) => Some(CommandId::ScrollWorkspaceUp),
    MouseEventKind::ScrollDown if rect_contains(areas.workspace_body, mouse.column, mouse.row) => Some(CommandId::ScrollWorkspaceDown),
    _ => None,
};
if let Some(command) = command {
    self.submit_event(AppEvent::Action(Action::Invoke(command))).await;
}
```

- [ ] **GREEN: Run focused tests**

```bash
cargo test state::workbench -- --nocapture
cargo test ui::workbench -- --nocapture
cargo test app::command -- --nocapture
cargo test app::reducer -- --nocapture
cargo test app::policy -- --nocapture
```

Expected: hit-map tests pass for wide, medium, compact, and too-small layouts; every visible control resolves to its intended registered command; no same-priority control is shadowed; mouse wheel over Workspace emits scroll commands; mouse wheel outside Workspace emits no workspace scroll; all invocations use the same reducer path.

- [ ] **Commit boundary**

```bash
git add src/state/workbench.rs src/ui/workbench.rs src/app.rs src/app/command.rs src/app/event.rs src/app/reducer.rs src/app/policy.rs
git commit -m "feat: route mouse actions through registered workbench commands"
```

---

### Task 9: Explicit lifecycle UI from Phase 2 LoadState and retry state

**Files:**
- Modify: `src/ui/tabs/tables.rs`
- Modify: `src/ui/tabs/sql.rs`
- Modify: `src/ui/tabs/logs.rs`
- Modify: `src/ui/tabs/metrics.rs`
- Modify: `src/ui/tabs/live.rs`
- Modify: `src/ui/activity.rs`
- Modify: `src/ui/states.rs`

- [ ] **RED: Add lifecycle render assertions from Phase 2 state model**

Use Phase 2 `LoadState<T>`, connection state, and bounded retry state as the source of truth. If exact Phase 2 field names differ, adapt to the Phase 2 names while preserving these assertions.

```rust
#[test]
fn table_workspace_renders_phase_2_load_states() {
    let mut app = app_state();
    app.resources.table_rows = LoadState::Loading { request: request_context("rows/users") };
    let loading = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(loading.contains("Loading"));

    app.resources.table_rows = LoadState::Refreshing { data: rows_fixture(), request: request_context("rows/users") };
    let refreshing = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(refreshing.contains("Refreshing"));
    assert!(refreshing.contains("users") || refreshing.contains("row"));

    app.resources.table_rows = LoadState::Ready { data: rows_fixture(), refreshed_at: fixed_instant() };
    let ready = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(ready.contains("Ready") || ready.contains("users") || ready.contains("row"));

    app.resources.table_rows = LoadState::Empty { refreshed_at: fixed_instant() };
    let empty = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(empty.contains("Empty"));

    app.resources.table_rows = LoadState::Stale { data: rows_fixture(), reason: stale_reason("offline") };
    let stale = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(stale.contains("Stale"));

    app.resources.table_rows = LoadState::Error { previous: None, error: app_error("boom") };
    let error = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(error.contains("Error"));
}

#[test]
fn activity_renders_offline_and_retrying_from_connection_and_retry_state() {
    let mut app = app_state();
    app.activity.connection = ConnectionState::Disconnected;
    let offline = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(offline.contains("Offline") || offline.contains("Disconnected"));

    app.activity.retry = Some(RetryState { attempt: 2, seconds_remaining: 5, scope: retry_scope("rows/users") });
    let retrying = render_to_string(120, 20, |area, buf| { render_workbench(area, buf, &app); });
    assert!(retrying.contains("Retrying"));
    assert!(retrying.contains("2") || retrying.contains("5s"));
}
```

- [ ] **GREEN: Render lifecycle from authoritative Phase 2 data**

Map Phase 2 state into `LifecyclePresentation` in `src/ui/states.rs` and each workspace renderer:

- `LoadState::Loading { .. }` -> `Loading`
- `LoadState::Refreshing { data, .. }` -> `Refreshing`, while keeping existing data visible
- `LoadState::Ready { data, .. }` -> `Ready` presentation or normal content with a visible fresh/live marker
- `LoadState::Empty { .. }` -> `Empty`
- `LoadState::Stale { data, reason }` -> `Stale`, while keeping stale data visible
- `LoadState::Error { previous, error }` -> `Error`, preserving previous data if present
- disconnected connection plus cached data -> `Offline` and `Stale`
- active retry entry from Phase 2 retry state -> `Retrying` with attempt and countdown

Do not add new duplicate lifecycle booleans. Phase 3 lifecycle presentation must derive from Phase 2 `LoadState`, connection, and retry models.

- [ ] **GREEN: Run render matrix tests**

```bash
cargo test ui::workbench -- --nocapture
cargo test ui::states -- --nocapture
cargo test ui::tabs -- --nocapture
cargo test state::activity -- --nocapture
```

Expected: Loading, Refreshing, Ready, Empty, Stale, Offline, Retrying, and Error labels have render coverage at `120x20`, `90x16`, and `40x12`, with no panics across the TestBackend matrix.

- [ ] **Commit boundary**

```bash
git add src/ui/tabs/tables.rs src/ui/tabs/sql.rs src/ui/tabs/logs.rs src/ui/tabs/metrics.rs src/ui/tabs/live.rs src/ui/activity.rs src/ui/states.rs
git commit -m "feat: render phase 2 lifecycle states across workspaces"
```

---

### Task 10: Focus, navigation, compact quick switch, and migration bindings

**Files:**
- Modify: `src/state/workbench.rs`
- Modify: `src/app/command.rs`
- Modify: `src/app.rs`
- Modify: `src/ui/workbench.rs`
- Modify: `src/ui/components/help.rs`

- [ ] **RED: Add focus semantics tests**

In `src/state/workbench.rs` tests:

```rust
#[test]
fn compact_surface_switch_updates_focus() {
    let mut state = WorkbenchState::default();
    state.set_compact_surface(CompactSurface::Explorer);
    assert_eq!(state.focused_pane, WorkbenchPane::Explorer);
    state.set_compact_surface(CompactSurface::Inspector);
    assert_eq!(state.focused_pane, WorkbenchPane::Inspector);
}

#[test]
fn migration_help_uses_binding_flags() {
    let registry = crate::app::command::CommandRegistry::default();
    let migration_bindings: Vec<_> = registry
        .iter()
        .flat_map(|spec| spec.bindings.iter())
        .filter(|binding| binding.migration_alias)
        .collect();
    assert!(!migration_bindings.is_empty());
    assert!(migration_bindings.iter().any(|binding| binding.display == "1"));
}
```

Render tests:

```rust
#[test]
fn compact_breadcrumb_and_connection_remain_visible() {
    let app = app_state();
    let text = render_to_string(40, 12, |area, buf| { render_workbench(area, buf, &app); });
    assert!(text.contains("DATA"));
    assert!(text.contains("Disconnected") || text.contains("Connected") || text.contains("Connection"));
}
```

- [ ] **GREEN: Implement semantics**

- `Tab` and `Shift+Tab` call `FocusNextPane` and `FocusPrevPane`, using an available-pane list derived from the current layout. Closed Inspector drawers and compact hidden panes are skipped.
- In Medium, `FocusInspector` opens drawer first or returns disabled reason. Pick one policy and test it.
- In Compact, `FocusNextPane` changes `compact_surface` in Explorer -> Workspace -> Inspector -> Activity order.
- `Esc` closes palette, modal, help, and drawer before semantic navigation.
- Migration bindings remain documented in generated help by filtering `Binding { migration_alias: true, .. }` from the Phase 2 registry.

- [ ] **GREEN: Run tests**

```bash
cargo test state::workbench -- --nocapture
cargo test app::command -- --nocapture
cargo test ui::workbench -- --nocapture
cargo test ui::components::help -- --nocapture
```

Expected: focus tests pass and migration help tests use Phase 2 binding flags.

- [ ] **Commit boundary**

```bash
git add src/state/workbench.rs src/app/command.rs src/app.rs src/ui/workbench.rs src/ui/components/help.rs
git commit -m "feat: define workbench focus navigation semantics"
```

---

### Task 11: Unicode, dimension state-space, docs, CHANGELOG, and deterministic SVG screenshots

**Files:**
- Modify: `src/ui/workbench.rs`
- Modify: `src/ui/test_support.rs`
- Modify: `src/ui/components/input.rs` only if unsafe UTF-8 slicing is found
- Create: `images/contextual-workbench-wide.svg`
- Create: `images/contextual-workbench-medium.svg`
- Create: `images/contextual-workbench-compact.svg`
- Modify: `README.md`
- Modify: `CHANGELOG.md`

- [ ] **RED: Add exhaustive dimension and Unicode tests**

In `src/ui/workbench.rs` tests:

```rust
#[test]
fn exhaustive_dimensions_do_not_panic() {
    let mut app = app_state();
    app.search_query = "é 你 🚀 … µs".to_string();
    for width in 0..=200 {
        for height in 0..=80 {
            let _ = render_to_string(width, height, |area, buf| { render_workbench(area, buf, &app); });
        }
    }
}

#[test]
fn unicode_context_strings_do_not_panic_or_byte_slice() {
    let mut app = app_state();
    app.databases = vec!["数据库🚀".to_string()];
    app.selected_database_idx = Some(0);
    let text = render_to_string(40, 12, |area, buf| { render_workbench(area, buf, &app); });
    assert!(text.contains("DATA") || text.contains("terminal too small"));
}
```

- [ ] **GREEN: Fix any unsafe truncation discovered by tests**

Use `.chars().take(n).collect::<String>()` or `unicode-width` based truncation. Do not slice arbitrary `&str` by byte index in render paths.

- [ ] **RED: Add deterministic ignored screenshot generator test inside the binary module tree**

This project has no library target. Do not create an example target or import private modules from outside the binary crate. Put the generator in `src/ui/test_support.rs` under the existing `#[cfg(test)]` test-support module, or in a child test-support module declared by `src/ui/mod.rs`. The ignored test must call the real `render_workbench` path with `TestBackend`, fixed fixture state, and deterministic SVG serialization. It must not require a live terminal, server, clock, or network.

Add this to `src/ui/test_support.rs` tests, using the Phase 2 `AppState::new("http://localhost:3000")` constructor already present in the codebase:

```rust
#[test]
#[ignore = "regenerates deterministic screenshot SVG fixtures"]
fn regenerate_workbench_screenshots() {
    if std::env::var_os("UPDATE_SCREENSHOTS").is_none() {
        eprintln!("set UPDATE_SCREENSHOTS=1 to regenerate workbench SVG screenshots");
        return;
    }

    write_svg("images/contextual-workbench-wide.svg", render_workbench_svg(120, 20, |_| {}));
    write_svg("images/contextual-workbench-medium.svg", render_workbench_svg(90, 16, |app| {
        app.workbench.inspector_open = true;
    }));
    write_svg("images/contextual-workbench-compact.svg", render_workbench_svg(40, 12, |app| {
        app.workbench.set_compact_surface(crate::state::CompactSurface::Workspace);
    }));
}

fn render_workbench_svg(width: u16, height: u16, configure: impl FnOnce(&mut crate::state::AppState)) -> String {
    use ratatui::{backend::TestBackend, Terminal};

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut app = crate::state::AppState::new("http://localhost:3000");
    configure(&mut app);
    terminal
        .draw(|frame| crate::ui::workbench::render_workbench(frame.area(), frame.buffer_mut(), &app))
        .expect("draw workbench screenshot");
    buffer_to_svg(terminal.backend().buffer(), width, height)
}


fn buffer_to_svg(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> String {
    let cell_w = 8u16;
    let cell_h = 16u16;
    let svg_width = width.saturating_mul(cell_w);
    let svg_height = height.saturating_mul(cell_h);
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{svg_width}" height="{svg_height}" viewBox="0 0 {svg_width} {svg_height}"><rect width="100%" height="100%" fill="#0f1624"/><g font-family="monospace" font-size="14" fill="#dcdcdc">"#
    );
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        let y_pos = (y + 1).saturating_mul(cell_h);
        let escaped = escape_xml(line.trim_end());
        svg.push_str(&format!(r#"<text x="0" y="{y_pos}">{escaped}</text>"#));
    }
    svg.push_str("</g></svg>\n");
    svg
}

fn escape_xml(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn write_svg(path: &str, svg: String) {
    let path = std::path::Path::new(path);
    std::fs::create_dir_all(path.parent().expect("image parent")).expect("create image dir");
    std::fs::write(path, svg).expect("write svg");
}
```

- [ ] **GREEN: Generate exact deterministic SVG outputs through the ignored unit test**

```bash
UPDATE_SCREENSHOTS=1 cargo test ui::test_support::tests::regenerate_workbench_screenshots -- --ignored --nocapture
test -f images/contextual-workbench-wide.svg
test -f images/contextual-workbench-medium.svg
test -f images/contextual-workbench-compact.svg
grep -q "DATA" images/contextual-workbench-wide.svg
grep -q "OBSERVE" images/contextual-workbench-wide.svg
grep -q "OPERATE" images/contextual-workbench-wide.svg
grep -q "Inspector" images/contextual-workbench-medium.svg
grep -q "Workspace" images/contextual-workbench-compact.svg
```

Expected: the ignored test writes exactly three deterministic SVG files at the listed paths. Do not create PNG screenshots, example targets, or a new library target for screenshot generation.

- [ ] **GREEN: Update README and CHANGELOG docs**

Add this README section and link the deterministic SVGs:

```markdown
## Contextual Workbench

`spacetimedb-tui` is organized into DATA, OBSERVE, and OPERATE modes.

- DATA: table browsing and SQL workspaces.
- OBSERVE: logs, metrics, and live observation workspaces.
- OPERATE: reducers, module information, and database-level operations.

Responsive terminal contracts:

- Wide: at least 120x20, showing Explorer, Workspace, Inspector, and Activity.
- Medium: at least 90x16, showing Explorer and Workspace with Inspector as a drawer.
- Compact: at least 40x12, showing one primary surface with breadcrumb and connection state.
- Below 40x12, the app shows a terminal-too-small screen instead of panicking.

Screenshots:

- [Wide workbench](images/contextual-workbench-wide.svg)
- [Medium workbench](images/contextual-workbench-medium.svg)
- [Compact workbench](images/contextual-workbench-compact.svg)

Regenerate deterministic screenshots with:

```bash
UPDATE_SCREENSHOTS=1 cargo test ui::test_support::tests::regenerate_workbench_screenshots -- --ignored --nocapture
```

Use the generated help screen for command palette and contextual help bindings. The displayed labels come from `CommandRegistry::default().help_lines()` and each `CommandSpec.bindings[*].display`, not hardcoded shortcut prose. Existing non-conflicting shortcuts such as `1` through `6`, `q`, `e`, `E`, `y`, and `Y` remain as one-release migration bindings and are listed in help during migration.

Mouse support is parity-only: clicking panes, tabs, rows, modal choices, and scroll targets emits the same registered commands available from keyboard and palette. Mouse wheel emits `ScrollWorkspaceUp` and `ScrollWorkspaceDown`, which have `PageUp` and `PageDown` keyboard parity. There are no mouse-only actions.
```

Add a `CHANGELOG.md` entry under the current unreleased heading or create an `Unreleased` heading if absent:

```markdown
## Unreleased

### Added

- Added the Contextual Workbench layout with DATA, OBSERVE, and OPERATE modes.
- Added responsive Explorer, Workspace, Inspector, and Activity surfaces for wide, medium, and compact terminals.
- Added deterministic SVG screenshots for the wide, medium, and compact workbench layouts.
- Added generated command palette and contextual help coverage for workbench commands and one-release migration bindings.

### Changed

- Mouse interactions now route through registered commands with keyboard parity, including workspace wheel scrolling through `ScrollWorkspaceUp` and `ScrollWorkspaceDown`.
```

- [ ] **GREEN: Run final verification**

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release
UPDATE_SCREENSHOTS=1 cargo test ui::test_support::tests::regenerate_workbench_screenshots -- --ignored --nocapture
git diff --check
```

Expected: all commands pass and the three SVG screenshots are deterministic when the ignored test is run twice.

- [ ] **Commit boundary**

```bash
git add src/ui/workbench.rs src/ui/test_support.rs src/ui/components/input.rs images/contextual-workbench-wide.svg images/contextual-workbench-medium.svg images/contextual-workbench-compact.svg README.md CHANGELOG.md
git commit -m "docs: document contextual workbench with screenshots"
```

---

## Final implementation validation checklist

- [ ] `cargo fmt --check`
- [ ] `cargo test`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo build --release`
- [ ] `git diff --check`
- [ ] Wide `120x20` render contains DATA, OBSERVE, OPERATE, Explorer, Workspace, Inspector, Activity.
- [ ] Medium `90x16` render keeps Explorer and Workspace visible and gates Inspector as drawer.
- [ ] Compact `40x12` render keeps breadcrumb and connection state visible.
- [ ] Too-small `39x12` and `40x11` render `terminal too small` and do not panic.
- [ ] Command palette and help are generated from Phase 2 `CommandRegistry` and `COMMAND_SPECS`.
- [ ] Every mouse hit target emits a registered command.
- [ ] Every registered mouse command is available by keyboard or palette, including `ScrollWorkspaceUp` and `ScrollWorkspaceDown` with PageUp/PageDown parity.
- [ ] One-release migration bindings are listed in generated help by filtering `migration_alias == true`.
- [ ] Loading, Refreshing, Ready, Empty, Stale, Offline, Retrying, and Error labels render from Phase 2 LoadState, connection, and retry state.
- [ ] Unicode strings `é`, `你`, `🚀`, `…`, and `µs` do not panic in render tests.
- [ ] README and CHANGELOG explain modes, responsive contracts, command palette, help, mouse parity, migration bindings, and deterministic SVG screenshots.
