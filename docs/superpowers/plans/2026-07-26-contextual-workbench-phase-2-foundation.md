# Contextual Workbench Phase 2 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Phase 2 state and interaction foundation for the contextual workbench while preserving the current layout and behavior.

**Architecture:** Introduce typed commands, normalized actions/events/effects, pure reducer transitions, deterministic runtime ports, source-preserving configuration, and a single `AppState` owner in small behavior-preserving slices. Event and effect adapters must not own a second copy of navigation or resource state. Extract responsibilities out of `src/app.rs` incrementally into focused child modules declared from `src/app.rs` without changing the visible Ratatui layout. `src/app.rs` remains the module root throughout Phase 2.

**Tech Stack:** Rust 2021, Cargo tests, Ratatui/Crossterm existing UI stack, Clap `ValueSource`, Tokio channels/tasks, existing SpacetimeDB HTTP and WebSocket adapters.

---

## File structure and responsibilities

- `src/app.rs`: remains the module root and public app shell through every Phase 2 extraction task. It declares submodules with `pub mod command;`, `pub mod event;`, `pub mod reducer;`, `pub mod navigation;`, and `pub mod policy;`, imports extracted modules, and preserves the current layout/event loop. Do not create an app `mod.rs` file while `src/app.rs` exists.
- `src/app/command.rs`: stable command API: `CommandId`, `CommandSpec`, `Binding`, `Availability`, `DisabledReason`, `AvailabilityRule`, and `CommandRegistry::default()` with exactly `by_id(CommandId)`, `iter()`, `resolve_key(KeyChord)`, `resolve_palette(&str)`, `search_palette(&self, query: &str) -> Vec<CommandId>`, `help_lines()`, and `availability(CommandId, &CommandContext)`. `CommandSpec.focus` is `&'static [FocusContext]`. `CommandSpec` carries exactly one `availability: AvailabilityRule`, not a slice. Stable Phase 2 command names include `OpenCommandPalette`, `OpenContextualHelp`, `CloseSurfaceOrNavigateUp`, `FocusNextPane`, `FocusPreviousPane`, `ToggleLive`, and `ConfirmWritePlan`; preserve additional migrated commands as needed. Do not create command state under `src/state`; keyboard, palette, help, and later mouse input all resolve through this registry and the shared policy path, with no competing hard-coded availability checks. The registry includes every migrated user command exactly once, including `OpenCommandPalette`, `OpenContextualHelp`, `CloseSurfaceOrNavigateUp`, `FocusNextPane`, `FocusPreviousPane`, `RefreshActiveResource`, `ToggleLive`, and `ConfirmWritePlan`.
- `src/app/event.rs`: normalized `Action`, `AppEvent`, stable `ReadOperation`, scoped async event payloads, bounded event channel aliases. The runtime event channel is bounded, not any unbounded Tokio MPSC API, so backpressure and overflow are testable.
- `src/app/reducer.rs`: pure `reduce(&mut AppState, AppEvent) -> Transition` transitions. No network, terminal, or time side effects.
- `src/app/navigation.rs`: semantic navigation helpers that update `NavigationState` and emit required load effects.
- `src/app/policy.rs`: shared command availability and safety policy helpers.
- `src/effects/mod.rs`: effect subsystem module root.
- `src/effects/runner.rs`: deterministic ports, production runner boundary, bounded event channel construction, task ownership.
- `src/state/app_state.rs`: the only mutable runtime owner for Phase 2 app state, composed of `NavigationState`, `ResourceState`, `WorkbenchState`, `ActivityState`, and `SafetyState`, plus `LoadState<T>`. Session/config structs remain I/O or startup inputs and must not become a second mutable owner.
- `src/state/navigation.rs`: active database, active resource, selection identity, and session selection mapping only. It must not own mode, workspace, or focused pane.
- `src/state/resources.rs`: scoped resource data and `LoadState<T>` transition helpers.
- `src/state/workbench.rs`: the single owner of `WorkbenchMode`, `Workspace`, `WorkbenchPane`, focused pane, SQL input/history cursor, grid cursor/search, palette/help/modal state, including `Workspace::Live`.
- `src/state/activity.rs`: active work, notices, connection lifecycle, retry countdowns.
- `src/state/safety.rs`: existing Phase 1 write-safety domain. Modify it to add `SafetyState` while preserving `RequestContext`, `WritePlan`, primary-key, mutation uncertainty, and existing write-safety APIs. Do not overwrite or redefine existing write-plan or mutation types.
- `src/state/mod.rs`: exports state modules.
- `src/config.rs`: source-preserving startup resolution using `ConfigSource`, `Resolved<T>`, Clap `ValueSource`, CLI over env over lower sources, `--no-tls`, and custom theme strings.
- `src/user_config.rs`: manual `Default` with `restore_session: true` and deterministic session mapping support.
- `src/ui/components/help.rs`: reads the command registry for help text instead of maintaining independent shortcut descriptions.
- `src/ui/components/palette.rs`: resolves command palette entries from the command registry.
- `src/main.rs`: passes Clap value sources to config resolution.
- `README.md`: Phase 2 truth for command foundation, session defaults, source precedence, and no layout change yet.
- `CHANGELOG.md`: modify the existing Phase 1 changelog with the Phase 2 foundation entry.

## Implementation tasks

### Task 1: State foundation before command registry: AppState, WorkbenchState, ResourceState, ActivityState, and SafetyState

**Files:**
- Create: `src/state/resources.rs`
- Create: `src/state/workbench.rs`
- Create: `src/state/activity.rs`
- Create: `src/state/navigation.rs`
- Modify: `src/state/safety.rs`
- Modify: `src/state/app_state.rs`
- Modify: `src/state/mod.rs`

This task intentionally moves the state foundation before reducer/navigation tasks. Later RED/GREEN snippets must not depend on future fields. Phase 1 already owns `crate::effects::request::{RequestId, RequestScope, RequestContext}` and `src/state/safety.rs`; reuse those types and never redefine them.

- [ ] **Step 1: Write the failing tests**

Append to `src/state/app_state.rs`:

```rust
#[cfg(test)]
mod phase2_state_tests {
    use super::*;
    use crate::effects::request::RequestScope;
    use crate::state::workbench::{WorkbenchMode, WorkbenchPane, Workspace};

    #[test]
    fn app_state_has_one_owner_for_phase2_state_domains() {
        let state = AppState::new("http://localhost:3000".to_string());

        assert_eq!(state.navigation.active_database, None);
        assert_eq!(state.navigation.active_resource, None);
        assert_eq!(state.workbench.mode, WorkbenchMode::Data);
        assert_eq!(state.workbench.workspace, Workspace::Tables);
        assert_eq!(state.workbench.focused_pane, WorkbenchPane::Explorer);
        assert_eq!(state.activity.active_task_count(), 0);
        assert_eq!(state.safety.unresolved_mutation_count(), 0);
    }

    #[test]
    fn request_tracker_generations_increment_per_scope_independently() {
        let mut tracker = crate::state::resources::RequestTracker::default();
        let table = RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() };
        let schema = RequestScope::Schema { database: "inventory".into() };

        let table_first = tracker.next_context(table.clone());
        let table_second = tracker.next_context(table.clone());
        let schema_first = tracker.next_context(schema.clone());

        assert_eq!(table_first.generation, 1);
        assert_eq!(table_second.generation, 2);
        assert_eq!(schema_first.generation, 1);
        assert!(tracker.is_current(&table_second));
        assert!(!tracker.is_current(&table_first));
        assert!(tracker.is_current(&schema_first));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test state::app_state::phase2_state_tests::app_state_has_one_owner_for_phase2_state_domains`

Expected: FAIL with missing state modules or fields. It must not fail because Phase 1 `WritePlan`, primary-key, mutation uncertainty, or safety APIs were overwritten.

- [ ] **Step 3: Write minimal implementation**

Add this migration table to the task notes before editing `AppState` so no current field is silently dropped:

```text
Current AppState field migration table
- active database/resource/session identity -> NavigationState
- table/schema/query/log/metrics/live data and request generations -> ResourceState
- SQL input/history, table cursor/search, palette/help/modal flags, mode/workspace/pane -> WorkbenchState
- connection, retry, active tasks, notices, WebSocket lifecycle -> ActivityState
- WritePlan, WritePlanId, primary-key selection, mutation uncertainty, confirmation state -> SafetyState
- base_url, config, theme, terminal/UI layout caches, clients, and any field not migrated in this task -> explicitly preserved legacy field until its owning task moves it
```

Create `src/state/workbench.rs` as the one owner for Phase 2 and Phase 3 workbench concepts:

```rust
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkbenchMode {
    #[default]
    Data,
    Observe,
    Operate,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WorkbenchPane {
    #[default]
    Explorer,
    Workspace,
    Inspector,
    Activity,
}
```

Create `src/state/navigation.rs` so navigation references workbench state instead of duplicating mode/workspace/pane owners. This file does not exist in the current baseline, so Task 1 creates it and `src/state/mod.rs` exports it in the same task:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NavigationState {
    pub active_database: Option<String>,
    pub active_resource: Option<String>,
    pub selection_identity: Option<String>,
}
```

Create `src/state/resources.rs` using Phase 1 request types and typed payloads. Replace payload aliases with existing project types where available. `CatalogSnapshot = Vec<String>` is acceptable because the current catalog shape is string-based; do not downgrade typed schema/query/log/metrics/live payloads to strings:

```rust
use std::time::Instant;

use crate::effects::request::{RequestContext, RequestId, RequestScope};

pub type CatalogSnapshot = Vec<String>;
pub type SchemaSnapshot = crate::api::types::Schema;
pub type TableRows = crate::api::types::QueryResult;
pub type LogSnapshot = Vec<crate::api::types::LogEntry>;
// Move the existing `MetricsSnapshot` struct definition out of `src/state/app_state.rs` into `src/state/resources.rs`, then re-export or update old call sites. Do not create a self-alias.
use std::collections::HashMap;

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub total_reducer_calls: u64,
    pub total_energy_used: u64,
    pub connected_clients: u64,
    pub memory_bytes: u64,
    pub sampled_at: Option<DateTime<Utc>>,
    pub extra: HashMap<String, serde_json::Value>,
}

pub type MetricSnapshot = MetricsSnapshot;
// Move the existing `LiveClientEntry` definition out of its current owner into this module, remove the old definition from the previous module, and update all old call-site imports to `crate::state::resources::LiveClientEntry`. Do not leave the same type defined in two modules.
pub type LiveClientSnapshot = Vec<LiveClientEntry>;

#[derive(Debug, Clone)]
pub struct LiveClientEntry {
    pub identity: String,
    pub connected_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone)]
pub enum LoadState<T> {
    Idle,
    Loading { request: RequestContext },
    Ready { data: T, refreshed_at: Instant },
    Refreshing { data: T, request: RequestContext },
    Empty { refreshed_at: Instant },
    Stale { data: T, reason: StaleReason },
    Error { previous: Option<T>, error: AppError },
}

impl<T> Default for LoadState<T> {
    fn default() -> Self { Self::Idle }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleReason {
    Offline,
    NewerGeneration,
    BudgetExceeded,
    ManualRefreshRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct ResourceState {
    pub catalog: LoadState<CatalogSnapshot>,
    pub schema: LoadState<SchemaSnapshot>,
    pub table_rows: LoadState<TableRows>,
    pub logs: LoadState<LogSnapshot>,
    pub metrics: LoadState<MetricSnapshot>,
    pub live_clients: LoadState<LiveClientSnapshot>,
}

#[derive(Debug, Clone, Default)]
// RequestTracker has one global unique RequestId counter and one per-scope generation counter.
pub struct RequestTracker {
    next_id: u64,
    next_generation_by_scope: std::collections::HashMap<RequestScope, u64>,
    latest_by_scope: std::collections::HashMap<RequestScope, RequestContext>,
}

impl RequestTracker {
    pub fn next_context(&mut self, scope: RequestScope) -> RequestContext {
        self.next_id += 1;
        let generation = self.next_generation_by_scope.entry(scope.clone()).or_insert(0);
        *generation += 1;
        let context = RequestContext::new(RequestId::from_u64(self.next_id), scope.clone(), *generation);
        self.latest_by_scope.insert(scope, context.clone());
        context
    }

    pub fn is_current(&self, context: &RequestContext) -> bool {
        self.latest_by_scope.get(context.scope()) == Some(context)
    }
}
```

Create `src/state/activity.rs` as the one owner for activity, retry, and connection lifecycle:

```rust
use crate::effects::request::RequestId;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityState {
    pub active_reads: Vec<RequestId>,
    pub active_mutations: Vec<RequestId>,
    pub notices: Vec<String>,
    pub connection: ConnectionState,
    pub retry: RetryState,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RetryState {
    pub attempt: u32,
    pub next_retry_label: Option<String>,
}

impl ActivityState {
    pub fn active_task_count(&self) -> usize {
        self.active_reads.len() + self.active_mutations.len()
    }
}
```

Modify existing `src/state/safety.rs` by extending Phase 1 safety types without redefining them. Add exact `MutationUncertainty { plan_id: WritePlanId, table: QualifiedTable, reason: String }` and record only `MutationOutcome::Unknown`. Preserve existing `WritePlan`, primary-key, mutation uncertainty, and safety APIs:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationUncertainty {
    pub plan_id: WritePlanId,
    pub table: QualifiedTable,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SafetyState {
    pub pending_write_plans: Vec<WritePlan>,
    pub unresolved_mutations: Vec<MutationUncertainty>,
}

impl SafetyState {
    pub fn record_mutation_outcome(&mut self, plan_id: WritePlanId, table: QualifiedTable, outcome: MutationOutcome) {
        if let MutationOutcome::Unknown { reason } = outcome {
            self.unresolved_mutations.push(MutationUncertainty { plan_id, table, reason });
        }
    }

    pub fn unresolved_mutation_count(&self) -> usize {
        self.unresolved_mutations.len()
    }
}

#[cfg(test)]
mod safety_state_tests {
    use super::*;

    #[test]
    fn records_only_unknown_mutation_outcomes_as_uncertainty() {
        let mut state = SafetyState::default();
        let plan_id = WritePlanId(7);
        let table = QualifiedTable { database: "inventory".to_string(), schema: None, table: "items".to_string() };

        state.record_mutation_outcome(plan_id, table.clone(), MutationOutcome::DefinitelyNotSent { reason: "client rejected before send".to_string() });
        state.record_mutation_outcome(plan_id, table.clone(), MutationOutcome::SentAndConfirmed { affected_rows: Some(1) });
        state.record_mutation_outcome(plan_id, table.clone(), MutationOutcome::Unknown { reason: "connection dropped before acknowledgement".to_string() });

        assert_eq!(state.unresolved_mutation_count(), 1);
        assert_eq!(state.unresolved_mutations[0].plan_id, plan_id);
        assert_eq!(state.unresolved_mutations[0].table, table);
    }
}
```

Adapt `src/state/app_state.rs` compile-guided. Do not replace the existing `AppState` with a shortened struct. Preserve every existing field and method, including `AppState::new(base_url)`, and move or mirror fields into the new owners only when call sites are updated. Source baseline tests must still pass at the end of the task:

```rust
use crate::state::activity::ActivityState;
use crate::state::navigation::NavigationState;
use crate::state::resources::{RequestTracker, ResourceState};
use crate::state::safety::SafetyState;
use crate::state::workbench::WorkbenchState;

// In the existing AppState definition, keep the current `#[derive(Debug)] pub struct AppState` and insert these fields without deleting any existing field:
pub navigation: NavigationState,
pub resources: ResourceState,
pub workbench: WorkbenchState,
pub activity: ActivityState,
pub safety: SafetyState,
pub requests: RequestTracker,

// In `AppState::new(base_url)`, keep all existing initialization and add these field initializers:
navigation: Default::default(),
resources: Default::default(),
workbench: Default::default(),
activity: Default::default(),
safety: Default::default(),
requests: Default::default(),
```

Update `src/state/mod.rs` to export every created or modified state module:

```rust
pub mod activity;
pub mod app_state;
pub mod edit_mode;
pub mod modal;
pub mod navigation;
pub mod palette;
pub mod resources;
pub mod safety;
pub mod workbench;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test state::app_state::phase2_state_tests`

Expected: PASS, and baseline source tests still pass so no existing AppState capability field or constructor behavior was lost.

- [ ] **Step 5: Commit**

```bash
git add src/state/app_state.rs src/state/mod.rs src/state/navigation.rs src/state/resources.rs src/state/workbench.rs src/state/activity.rs src/state/safety.rs
git commit -m "feat: add phase2 state foundation"
```

### Task 2: Central command registry and keyboard resolution

**Files:**
- Create: `src/app/command.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Write the failing tests**

Add this test module at the bottom of the new `src/app/command.rs` file. The migration inventory is the single explicit list of every current user-invokable command known at Phase 2 entry, including palette commands and direct key-handler actions, so no legacy command is lost or duplicated:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::workbench::WorkbenchMode;

    const MIGRATED_COMMAND_INVENTORY: &[CommandId] = &[
        CommandId::OpenCommandPalette,
        CommandId::OpenContextualHelp,
        CommandId::CloseSurfaceOrNavigateUp,
        CommandId::FocusNextPane,
        CommandId::FocusPreviousPane,
        CommandId::SelectDatabase,
        CommandId::SelectResource,
        CommandId::RefreshActiveResource,
        CommandId::ToggleLive,
        CommandId::ConfirmWritePlan,
        CommandId::RestoreSession,
        CommandId::GotoTables,
        CommandId::GotoSql,
        CommandId::GotoLogs,
        CommandId::GotoMetrics,
        CommandId::GotoModule,
        CommandId::GotoLive,
        CommandId::RefreshCurrentView,
        CommandId::ReconnectWebSocket,
        CommandId::ToggleHelp,
        CommandId::ExportCsv,
        CommandId::ExportJson,
        CommandId::CopyCell,
        CommandId::CopyRow,
        CommandId::Quit,
    ];

    #[test]
    fn ctrl_p_keyboard_palette_and_help_resolve_same_command() {
        let registry = CommandRegistry::default();
        let command = registry.by_id(CommandId::OpenCommandPalette).unwrap();

        assert_eq!(command.id, CommandId::OpenCommandPalette);
        assert_eq!(registry.resolve_key(KeyChord::ctrl('p')), Some(CommandId::OpenCommandPalette));
        assert_eq!(registry.resolve_palette("open command palette"), Some(CommandId::OpenCommandPalette));
        assert!(registry.help_lines().iter().any(|line| {
            line.command_id == CommandId::OpenCommandPalette
                && line.primary_binding.as_deref() == Some("Ctrl+P")
        }));
    }

    #[test]
    fn disabled_commands_report_stable_reason() {
        let registry = CommandRegistry::default();
        let context = CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: false,
            has_active_resource: false,
            schema_current: false,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        };

        assert_eq!(
            registry.availability(CommandId::RefreshActiveResource, &context),
            Availability::Disabled(DisabledReason::NoActiveResource)
        );
    }

    #[test]
    fn migration_inventory_matches_registry_once() {
        let mut inventory = MIGRATED_COMMAND_INVENTORY.to_vec();
        inventory.sort_by_key(|id| *id as u16);
        inventory.dedup();
        assert_eq!(inventory.len(), MIGRATED_COMMAND_INVENTORY.len(), "migration inventory has duplicates");

        let mut registry_ids = COMMAND_SPECS.iter().map(|spec| spec.id).collect::<Vec<_>>();
        registry_ids.sort_by_key(|id| *id as u16);
        registry_ids.dedup();

        let mut expected = MIGRATED_COMMAND_INVENTORY.to_vec();
        expected.sort_by_key(|id| *id as u16);

        assert_eq!(registry_ids, expected, "registry must contain exactly the migrated command inventory");
    }

    #[test]
    fn key_bindings_have_one_canonical_owner() {
        let mut owners = std::collections::HashMap::new();
        for spec in COMMAND_SPECS {
            for binding in spec.bindings {
                assert_eq!(owners.insert(binding.key, spec.id), None, "binding {binding:?} has multiple owners");
            }
        }
        assert_eq!(owners.get(&KeyChord::plain('?')), Some(&CommandId::OpenContextualHelp));
        assert_eq!(owners.get(&KeyChord::ctrl('r')), Some(&CommandId::RefreshActiveResource));
    }

    #[test]
    fn palette_search_preserves_fuzzy_subsequence_contract() {
        let registry = CommandRegistry::default();
        let all = registry.iter().map(|spec| spec.id).collect::<Vec<_>>();

        assert_eq!(registry.search_palette(""), all);
        assert_eq!(registry.search_palette("sql").first(), Some(&CommandId::GotoSql));
        assert!(registry.search_palette("zzz").is_empty());
        assert_eq!(registry.search_palette("go"), registry.search_palette("GO"));
    }

    #[test]
    fn palette_search_is_deterministic_by_score_then_registry_order() {
        let registry = CommandRegistry::default();
        let first = registry.search_palette("go");
        let second = registry.search_palette("go");

        assert_eq!(first, second);
        assert!(first.windows(2).all(|window| {
            registry.palette_score(window[0], "go") <= registry.palette_score(window[1], "go")
        }));
    }

    #[test]
    fn every_command_has_one_spec_and_one_typed_availability_rule() {
        let registry = CommandRegistry::default();
        let context = CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: true,
            schema_current: true,
            connection_online: true,
            live_available: true,
            write_plan_available: true,
        };

        for id in MIGRATED_COMMAND_INVENTORY {
            let specs = COMMAND_SPECS.iter().filter(|spec| spec.id == *id).collect::<Vec<_>>();
            assert_eq!(specs.len(), 1, "{id:?} must have exactly one CommandSpec");
            assert_eq!(registry.availability(*id, &context), evaluate_availability(specs[0].availability, &context));
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::command::tests`

Expected: FAIL with compile errors like `file not found for module command`, `cannot find type CommandRegistry`, `cannot find type CommandId`, or missing `AvailabilityRule`.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/command.rs` with this exact code. Phase 3 must use this exact registry API and must not introduce constructor synonyms beyond `CommandRegistry::default()`. Keyboard, palette, help, and later mouse input adapters map to `CommandId` only in Tasks 2 and 3; the shared `invoke(id)` action bridge is added in Task 4 after `app/event.rs` exists. This snippet must stay duplicate-free: each binding const appears once, each `CommandId` appears once in `COMMAND_SPECS`, and there is exactly one `#[cfg(test)] mod tests` in the module.

```rust
use crate::state::workbench::WorkbenchMode;
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u16)]
pub enum CommandId {
    OpenCommandPalette,
    OpenContextualHelp,
    CloseSurfaceOrNavigateUp,
    FocusNextPane,
    FocusPreviousPane,
    SelectDatabase,
    SelectResource,
    RefreshActiveResource,
    ToggleLive,
    ConfirmWritePlan,
    RestoreSession,
    GotoTables,
    GotoSql,
    GotoLogs,
    GotoMetrics,
    GotoModule,
    GotoLive,
    RefreshCurrentView,
    ReconnectWebSocket,
    ToggleHelp,
    ExportCsv,
    ExportJson,
    CopyCell,
    CopyRow,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusContext {
    Explorer,
    Workspace,
    Inspector,
    Activity,
    Palette,
    Help,
    Modal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    NoActiveDatabase,
    NoActiveResource,
    SchemaNotCurrent,
    Offline,
    LiveUnavailable,
    WritePlanUnavailable,
    WrongMode,
    WrongFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Disabled(DisabledReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityRule {
    Always,
    RequiresActiveDatabase,
    RequiresActiveResource,
    RequiresOnline,
    RequiresLiveAvailable,
    RequiresWritePlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn ctrl(c: char) -> Self {
        Self { code: KeyCode::Char(c), modifiers: KeyModifiers::CONTROL }
    }

    pub fn plain(c: char) -> Self {
        Self { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub key: KeyChord,
    pub display: &'static str,
    pub migration_alias: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
    pub bindings: &'static [Binding],
    pub modes: &'static [WorkbenchMode],
    pub focus: &'static [FocusContext],
    pub availability: AvailabilityRule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub mode: WorkbenchMode,
    pub focus: FocusContext,
    pub has_active_database: bool,
    pub has_active_resource: bool,
    pub schema_current: bool,
    pub connection_online: bool,
    pub live_available: bool,
    pub write_plan_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLine {
    pub command_id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
    pub primary_binding: Option<String>,
}

const ALL_MODES: &[WorkbenchMode] = &[WorkbenchMode::Data, WorkbenchMode::Observe, WorkbenchMode::Operate];
const ALL_FOCUS: &[FocusContext] = &[
    FocusContext::Explorer,
    FocusContext::Workspace,
    FocusContext::Inspector,
    FocusContext::Activity,
    FocusContext::Palette,
    FocusContext::Help,
    FocusContext::Modal,
];
const WORKBENCH_FOCUS: &[FocusContext] = &[
    FocusContext::Explorer,
    FocusContext::Workspace,
    FocusContext::Inspector,
    FocusContext::Activity,
];
const MODAL_OR_INSPECTOR_FOCUS: &[FocusContext] = &[FocusContext::Modal, FocusContext::Inspector];


const NO_BINDINGS: &[Binding] = &[];
const OPEN_PALETTE_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('p'), modifiers: KeyModifiers::CONTROL }, display: "Ctrl+P", migration_alias: false }];
const HELP_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('?'), modifiers: KeyModifiers::NONE }, display: "?", migration_alias: false }];
const ESC_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Esc, modifiers: KeyModifiers::NONE }, display: "Esc", migration_alias: false }];
const TAB_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Tab, modifiers: KeyModifiers::NONE }, display: "Tab", migration_alias: false }];
const BACKTAB_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::BackTab, modifiers: KeyModifiers::SHIFT }, display: "Shift+Tab", migration_alias: false }];
const REFRESH_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('r'), modifiers: KeyModifiers::CONTROL }, display: "Ctrl+R", migration_alias: false }];
const TOGGLE_LIVE_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('l'), modifiers: KeyModifiers::CONTROL }, display: "Ctrl+L", migration_alias: false }];
const CONFIRM_WRITE_PLAN_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Enter, modifiers: KeyModifiers::CONTROL }, display: "Ctrl+Enter", migration_alias: false }];
const QUIT_BINDINGS: &[Binding] = &[Binding { key: KeyChord { code: KeyCode::Char('q'), modifiers: KeyModifiers::NONE }, display: "q", migration_alias: true }];

pub const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec { id: CommandId::OpenCommandPalette, label: "Open command palette", description: "Search and run available commands for the current context.", bindings: OPEN_PALETTE_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::OpenContextualHelp, label: "Open contextual help", description: "Show shortcuts and actions available in the current context.", bindings: HELP_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::CloseSurfaceOrNavigateUp, label: "Close surface or navigate up", description: "Close the top modal surface or move one semantic navigation level up.", bindings: ESC_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::FocusNextPane, label: "Focus next pane", description: "Move focus to the next available workbench pane.", bindings: TAB_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::FocusPreviousPane, label: "Focus previous pane", description: "Move focus to the previous available workbench pane.", bindings: BACKTAB_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::SelectDatabase, label: "Select database", description: "Select a database from the catalog.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::SelectResource, label: "Select resource", description: "Select a table, reducer, or module resource.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::RefreshActiveResource, label: "Refresh active resource", description: "Reload the current database resource.", bindings: REFRESH_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::ToggleLive, label: "Toggle live updates", description: "Toggle live updates for the scoped active resource only; do not enable all-table Live behavior.", bindings: TOGGLE_LIVE_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresLiveAvailable },
    CommandSpec { id: CommandId::ConfirmWritePlan, label: "Confirm write plan", description: "Confirm the reviewed Phase 1 WritePlan for an explicit mutation with unresolved mutation uncertainty preserved.", bindings: CONFIRM_WRITE_PLAN_BINDINGS, modes: ALL_MODES, focus: MODAL_OR_INSPECTOR_FOCUS, availability: AvailabilityRule::RequiresWritePlan },
    CommandSpec { id: CommandId::RestoreSession, label: "Restore session", description: "Restore the configured previous database session.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::GotoTables, label: "Go to tables", description: "Show table resources.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoSql, label: "Go to SQL", description: "Show the SQL workspace.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoLogs, label: "Go to logs", description: "Show log output.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::GotoMetrics, label: "Go to metrics", description: "Show metrics output.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::GotoModule, label: "Go to module", description: "Show module details.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoLive, label: "Go to live", description: "Show the scoped live resource workspace.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresLiveAvailable },
    CommandSpec { id: CommandId::RefreshCurrentView, label: "Refresh current view", description: "Palette-only legacy alias for refreshing the current view through the canonical refresh path.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::ReconnectWebSocket, label: "Reconnect WebSocket", description: "Reconnect the WebSocket adapter.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::ToggleHelp, label: "Toggle help", description: "Palette-only legacy alias for contextual help; the ? key belongs to OpenContextualHelp.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::ExportCsv, label: "Export CSV", description: "Export the active grid as CSV.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::ExportJson, label: "Export JSON", description: "Export the active grid as JSON.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::CopyCell, label: "Copy cell", description: "Copy the focused grid cell.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::CopyRow, label: "Copy row", description: "Copy the focused grid row.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::Quit, label: "Quit", description: "Quit the application.", bindings: QUIT_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
];

pub fn evaluate_availability(rule: AvailabilityRule, context: &CommandContext) -> Availability {
    match rule {
        AvailabilityRule::Always => Availability::Available,
        AvailabilityRule::RequiresActiveDatabase if !context.has_active_database => Availability::Disabled(DisabledReason::NoActiveDatabase),
        AvailabilityRule::RequiresActiveResource if !context.has_active_resource => Availability::Disabled(DisabledReason::NoActiveResource),
        AvailabilityRule::RequiresOnline if !context.connection_online => Availability::Disabled(DisabledReason::Offline),
        AvailabilityRule::RequiresLiveAvailable if !context.live_available => Availability::Disabled(DisabledReason::LiveUnavailable),
        AvailabilityRule::RequiresWritePlan if !context.write_plan_available => Availability::Disabled(DisabledReason::WritePlanUnavailable),
        _ => Availability::Available,
    }
}

fn subsequence_score(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut total_gap = 0usize;
    let mut last_match = None;
    let mut chars = needle.chars();
    let mut wanted = chars.next()?;
    for (index, actual) in haystack.chars().enumerate() {
        if actual == wanted {
            if let Some(previous) = last_match {
                total_gap += index.saturating_sub(previous + 1);
            } else {
                total_gap += index;
            }
            last_match = Some(index);
            if let Some(next) = chars.next() {
                wanted = next;
            } else {
                return Some(total_gap);
            }
        }
    }
    None
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CommandRegistry;

impl CommandRegistry {
    pub fn by_id(&self, id: CommandId) -> Option<&'static CommandSpec> {
        COMMAND_SPECS.iter().find(|spec| spec.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static CommandSpec> {
        COMMAND_SPECS.iter()
    }

    pub fn resolve_key(&self, key: KeyChord) -> Option<CommandId> {
        COMMAND_SPECS.iter().find_map(|spec| {
            spec.bindings.iter().any(|binding| binding.key == key).then_some(spec.id)
        })
    }

    pub fn resolve_palette(&self, query: &str) -> Option<CommandId> {
        let normalized = query.trim().to_ascii_lowercase();
        COMMAND_SPECS.iter().find_map(|spec| {
            (spec.label.to_ascii_lowercase() == normalized).then_some(spec.id)
        })
    }

    pub fn search_palette(&self, query: &str) -> Vec<CommandId> {
        let normalized = query.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return self.iter().map(|spec| spec.id).collect();
        }

        let mut scored = self.iter().enumerate().filter_map(|(index, spec)| {
            let haystack = format!("{} {}", spec.label, spec.description).to_ascii_lowercase();
            subsequence_score(&haystack, &normalized).map(|score| (score, index, spec.id))
        }).collect::<Vec<_>>();
        scored.sort_by_key(|(score, index, _)| (*score, *index));
        scored.into_iter().map(|(_, _, id)| id).collect()
    }

    pub fn palette_score(&self, id: CommandId, query: &str) -> usize {
        let Some(spec) = self.by_id(id) else { return usize::MAX; };
        let haystack = format!("{} {}", spec.label, spec.description).to_ascii_lowercase();
        subsequence_score(&haystack, &query.trim().to_ascii_lowercase()).unwrap_or(usize::MAX)
    }

    pub fn help_lines(&self) -> Vec<HelpLine> {
        COMMAND_SPECS.iter().map(|spec| HelpLine {
            command_id: spec.id,
            label: spec.label,
            description: spec.description,
            primary_binding: spec.bindings.first().map(|binding| binding.display.to_string()),
        }).collect()
    }

    pub fn availability(&self, id: CommandId, context: &CommandContext) -> Availability {
        let Some(spec) = self.by_id(id) else { return Availability::Disabled(DisabledReason::WrongMode); };
        if !spec.modes.contains(&context.mode) {
            return Availability::Disabled(DisabledReason::WrongMode);
        }
        if !spec.focus.contains(&context.focus) {
            return Availability::Disabled(DisabledReason::WrongFocus);
        }
        evaluate_availability(spec.availability, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::workbench::WorkbenchMode;

    const MIGRATED_COMMAND_INVENTORY: &[CommandId] = &[
        CommandId::OpenCommandPalette,
        CommandId::OpenContextualHelp,
        CommandId::CloseSurfaceOrNavigateUp,
        CommandId::FocusNextPane,
        CommandId::FocusPreviousPane,
        CommandId::SelectDatabase,
        CommandId::SelectResource,
        CommandId::RefreshActiveResource,
        CommandId::ToggleLive,
        CommandId::ConfirmWritePlan,
        CommandId::RestoreSession,
        CommandId::GotoTables,
        CommandId::GotoSql,
        CommandId::GotoLogs,
        CommandId::GotoMetrics,
        CommandId::GotoModule,
        CommandId::GotoLive,
        CommandId::RefreshCurrentView,
        CommandId::ReconnectWebSocket,
        CommandId::ToggleHelp,
        CommandId::ExportCsv,
        CommandId::ExportJson,
        CommandId::CopyCell,
        CommandId::CopyRow,
        CommandId::Quit,
    ];

    #[test]
    fn ctrl_p_keyboard_palette_and_help_resolve_same_command() {
        let registry = CommandRegistry::default();
        let command = registry.by_id(CommandId::OpenCommandPalette).unwrap();

        assert_eq!(command.id, CommandId::OpenCommandPalette);
        assert_eq!(registry.resolve_key(KeyChord::ctrl('p')), Some(CommandId::OpenCommandPalette));
        assert_eq!(registry.resolve_palette("open command palette"), Some(CommandId::OpenCommandPalette));
        assert!(registry.help_lines().iter().any(|line| {
            line.command_id == CommandId::OpenCommandPalette
                && line.primary_binding.as_deref() == Some("Ctrl+P")
        }));
    }

    #[test]
    fn disabled_commands_report_stable_reason() {
        let registry = CommandRegistry::default();
        let context = CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: false,
            has_active_resource: false,
            schema_current: false,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        };

        assert_eq!(
            registry.availability(CommandId::RefreshActiveResource, &context),
            Availability::Disabled(DisabledReason::NoActiveResource)
        );
    }

    #[test]
    fn migration_inventory_matches_registry_once() {
        let mut inventory = MIGRATED_COMMAND_INVENTORY.to_vec();
        inventory.sort_by_key(|id| *id as u16);
        inventory.dedup();
        assert_eq!(inventory.len(), MIGRATED_COMMAND_INVENTORY.len(), "migration inventory has duplicates");

        let mut registry_ids = COMMAND_SPECS.iter().map(|spec| spec.id).collect::<Vec<_>>();
        registry_ids.sort_by_key(|id| *id as u16);
        registry_ids.dedup();

        let mut expected = MIGRATED_COMMAND_INVENTORY.to_vec();
        expected.sort_by_key(|id| *id as u16);

        assert_eq!(registry_ids, expected, "registry must contain exactly the migrated command inventory");
    }

    #[test]
    fn key_bindings_have_one_canonical_owner() {
        let mut owners = std::collections::HashMap::new();
        for spec in COMMAND_SPECS {
            for binding in spec.bindings {
                assert_eq!(owners.insert(binding.key, spec.id), None, "binding {binding:?} has multiple owners");
            }
        }
        assert_eq!(owners.get(&KeyChord::plain('?')), Some(&CommandId::OpenContextualHelp));
        assert_eq!(owners.get(&KeyChord::ctrl('r')), Some(&CommandId::RefreshActiveResource));
    }

    #[test]
    fn palette_search_preserves_fuzzy_subsequence_contract() {
        let registry = CommandRegistry::default();
        let all = registry.iter().map(|spec| spec.id).collect::<Vec<_>>();

        assert_eq!(registry.search_palette(""), all);
        assert_eq!(registry.search_palette("sql").first(), Some(&CommandId::GotoSql));
        assert!(registry.search_palette("zzz").is_empty());
        assert_eq!(registry.search_palette("go"), registry.search_palette("GO"));
    }

    #[test]
    fn palette_search_is_deterministic_by_score_then_registry_order() {
        let registry = CommandRegistry::default();
        let first = registry.search_palette("go");
        let second = registry.search_palette("go");

        assert_eq!(first, second);
        assert!(first.windows(2).all(|window| {
            registry.palette_score(window[0], "go") <= registry.palette_score(window[1], "go")
        }));
    }

    #[test]
    fn every_command_has_one_spec_and_one_typed_availability_rule() {
        let registry = CommandRegistry::default();
        let context = CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: true,
            schema_current: true,
            connection_online: true,
            live_available: true,
            write_plan_available: true,
        };

        for id in MIGRATED_COMMAND_INVENTORY {
            let specs = COMMAND_SPECS.iter().filter(|spec| spec.id == *id).collect::<Vec<_>>();
            assert_eq!(specs.len(), 1, "{id:?} must have exactly one CommandSpec");
            assert_eq!(registry.availability(*id, &context), evaluate_availability(specs[0].availability, &context));
        }
    }
}
```

Add this line near the top of `src/app.rs`, before other item definitions:

```rust
pub mod command;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::command::tests`

Expected: PASS, with all command registry tests passing, including coverage that the explicit migration inventory exactly matches registry IDs and every command has one `CommandSpec` plus exactly one typed `availability: AvailabilityRule` evaluated through the shared policy.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/app/command.rs
git commit -m "feat: add typed command registry"
```

### Task 3: Palette and help read from the same command registry

**Files:**
- Modify: `src/ui/components/help.rs`
- Modify: `src/ui/components/palette.rs`
- Modify: `src/app/command.rs`

- [ ] **Step 1: Write the failing tests**

Append these tests to `src/app/command.rs` inside the existing `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn palette_entries_are_generated_from_registered_commands() {
        let registry = CommandRegistry::default();
        let entries = registry.palette_entries(&CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: true,
            schema_current: true,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        });

        assert!(entries.iter().any(|entry| {
            entry.command_id == CommandId::RefreshActiveResource
                && entry.enabled
                && entry.label == "Refresh active resource"
        }));
    }

    #[test]
    fn help_marks_disabled_commands_with_registry_reason() {
        let registry = CommandRegistry::default();
        let lines = registry.contextual_help(&CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: false,
            schema_current: true,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        });

        assert!(lines.iter().any(|line| {
            line.command_id == CommandId::RefreshActiveResource
                && line.disabled_reason == Some(DisabledReason::NoActiveResource)
        }));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::command::tests`

Expected: FAIL with compile errors `no method named palette_entries found for struct CommandRegistry`, `no method named contextual_help`, and missing `disabled_reason` field.

- [ ] **Step 3: Write minimal implementation**

Add these structs and methods to `src/app/command.rs` after `HelpLine`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub command_id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    pub disabled_reason: Option<DisabledReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextualHelpLine {
    pub command_id: CommandId,
    pub label: &'static str,
    pub primary_binding: Option<String>,
    pub disabled_reason: Option<DisabledReason>,
}
```

Add these methods inside `impl CommandRegistry`:

```rust
    pub fn palette_entries(&self, context: &CommandContext) -> Vec<PaletteEntry> {
        COMMAND_SPECS.iter().map(|spec| {
            let availability = self.availability(spec.id, context);
            PaletteEntry {
                command_id: spec.id,
                label: spec.label,
                description: spec.description,
                enabled: availability == Availability::Available,
                disabled_reason: match availability {
                    Availability::Available => None,
                    Availability::Disabled(reason) => Some(reason),
                },
            }
        }).collect()
    }

    pub fn contextual_help(&self, context: &CommandContext) -> Vec<ContextualHelpLine> {
        COMMAND_SPECS.iter().map(|spec| {
            let availability = self.availability(spec.id, context);
            ContextualHelpLine {
                command_id: spec.id,
                label: spec.label,
                primary_binding: spec.bindings.first().map(|binding| binding.display.to_string()),
                disabled_reason: match availability {
                    Availability::Available => None,
                    Availability::Disabled(reason) => Some(reason),
                },
            }
        }).collect()
    }
```

In `src/ui/components/help.rs`, replace any hard-coded global shortcut list with this registry-driven helper:

```rust
use crate::app::command::{CommandContext, CommandRegistry, ContextualHelpLine};

pub fn contextual_help_lines(context: &CommandContext) -> Vec<ContextualHelpLine> {
    CommandRegistry::default().contextual_help(context)
}
```

Retire or bridge the legacy `crate::state::palette::Command::ALL` list through `src/app/command.rs` in this task. Phase 3 palette UI must call `search_palette` for case-insensitive subsequence fuzzy search and deterministic ordering. Do not keep a competing palette command inventory. If callers still need the old type during migration, provide a temporary conversion from registry `CommandId` to the legacy palette command shape and derive the old entries from `CommandRegistry::default().palette_entries(context)`.

In `src/ui/components/palette.rs`, add this registry-driven helper next to the existing palette state code:

```rust
use crate::app::command::{CommandContext, CommandRegistry, PaletteEntry};

pub fn palette_entries_for_context(context: &CommandContext) -> Vec<PaletteEntry> {
    CommandRegistry::default().palette_entries(context)
}
```

If either file already imports a type named `CommandRegistry`, use a single combined `use crate::app::command::{...};` line rather than duplicating imports.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::command::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app/command.rs src/ui/components/help.rs src/ui/components/palette.rs
git commit -m "feat: route palette and help through command registry"
```

### Task 4: Normalized Action, AppEvent, Effect, Transition, and pure reducer shell

**Files:**
- Create: `src/app/event.rs`
- Create: `src/app/reducer.rs`
- Create: `src/app/policy.rs`
- Modify: `src/app/command.rs`
- Modify: `src/app.rs`
- Modify: `src/user_config.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/app/reducer.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::command::CommandId;
    use crate::app::event::{Action, AppEvent, Effect, Transition};
    use crate::effects::request::RequestScope;
    use crate::state::app_state::AppState;
    use crate::state::workbench::WorkbenchMode;

    #[test]
    fn reducer_evaluates_policy_before_emitting_refresh_effect() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());
        state.workbench.mode = WorkbenchMode::Data;

        let transition = reduce(&mut state, AppEvent::Action(Action::Invoke(CommandId::RefreshActiveResource)));

        assert_eq!(state.navigation.active_database.as_deref(), Some("inventory"));
        assert_eq!(state.navigation.active_resource.as_deref(), Some("items"));
        assert!(matches!(transition, Transition { effects } if matches!(effects.as_slice(), [Effect::LoadTableRows { target, context }] if target.database == "inventory" && target.table == "items" && context.scope() == &RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() })));
    }

    #[test]
    fn reducer_blocks_unavailable_command_without_effects() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.workbench.mode = WorkbenchMode::Data;

        let transition = reduce(&mut state, AppEvent::Action(Action::Invoke(CommandId::RefreshActiveResource)));

        assert_eq!(transition.effects, Vec::new());
    }

    #[test]
    fn command_invoke_helper_creates_the_shared_action() {
        assert_eq!(crate::app::command::invoke(CommandId::GotoSql), Action::Invoke(CommandId::GotoSql));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::reducer::tests`

Expected: FAIL with missing `event`, `reducer`, `Transition`, typed effects, or policy evaluation.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/event.rs`:

```rust
use crate::app::command::CommandId;
use crate::api::types::{LogEntry, QueryResult, Schema};
use crate::effects::request::RequestContext;
use crate::state::resources::MetricsSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Invoke(CommandId),
    SelectDatabase { database: String },
    SelectResource { database: String, resource: String },
    NavigateUp,
    ConfirmWritePlan { plan_id: crate::state::safety::WritePlanId },
    RestoreSession,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    Action(Action),
    ScopedReadCompleted { context: RequestContext, result: ReadResult },
    ScopedReadFailed { context: RequestContext, failure: ReadFailure, retry: Option<ReadOperation> },
    Tick,
}

#[derive(Debug, Clone)]
pub enum ReadResult {
    Catalog(Vec<String>),
    Schema(Schema),
    TableRows(QueryResult),
    Logs(Vec<LogEntry>),
    Metrics(MetricsSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    Transport(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub effects: Vec<Effect>,
}

impl Transition {
    pub fn none() -> Self { Self { effects: Vec::new() } }
    pub fn effects(effects: Vec<Effect>) -> Self { Self { effects } }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTarget {
    pub database: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOperation {
    Catalog,
    Schema { database: String },
    TableRows { target: TableTarget },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    LoadCatalog { context: RequestContext },
    LoadSchema { database: String, context: RequestContext },
    LoadTableRows { target: TableTarget, context: RequestContext },
    PersistSession { snapshot: crate::user_config::SessionState },
}
```

After `src/app/event.rs` exists, add the shared action bridge to `src/app/command.rs`:

```rust
pub fn invoke(id: CommandId) -> crate::app::event::Action {
    crate::app::event::Action::Invoke(id)
}
```

Keyboard, palette, help, and later mouse input adapters must call `invoke(id)` or directly produce the identical `Action::Invoke(id)` value, with no competing mappings.

Create `src/app/policy.rs` in this task, before compiling the reducer:

```rust
use crate::app::command::{Availability, CommandContext, CommandId, CommandRegistry, FocusContext};
use crate::state::app_state::AppState;

pub const PHASE2_LAYOUT_CHANGE_ALLOWED: bool = false;

pub fn focus_context_from_pane(pane: crate::state::workbench::WorkbenchPane) -> FocusContext {
    match pane {
        crate::state::workbench::WorkbenchPane::Explorer => FocusContext::Explorer,
        crate::state::workbench::WorkbenchPane::Workspace => FocusContext::Workspace,
        crate::state::workbench::WorkbenchPane::Inspector => FocusContext::Inspector,
        crate::state::workbench::WorkbenchPane::Activity => FocusContext::Activity,
    }
}

pub fn command_context_from_state(state: &AppState) -> CommandContext {
    let schema_current = matches!(
        state.resources.schema,
        crate::state::resources::LoadState::Ready { .. }
            | crate::state::resources::LoadState::Refreshing { .. }
    );
    CommandContext {
        mode: state.workbench.mode,
        focus: focus_context_from_pane(state.workbench.focused_pane),
        has_active_database: state.navigation.active_database.is_some(),
        has_active_resource: state.navigation.active_resource.is_some(),
        schema_current,
        connection_online: matches!(state.activity.connection, crate::state::activity::ConnectionState::Connected),
        live_available: false,
        write_plan_available: !state.safety.pending_write_plans.is_empty(),
    }
}

pub fn command_availability(id: CommandId, context: &CommandContext) -> Availability {
    CommandRegistry::default().availability(id, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::command::{DisabledReason, FocusContext};

    #[test]
    fn command_context_maps_from_single_phase2_state_owners() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.workbench.focused_pane = crate::state::workbench::WorkbenchPane::Workspace;

        let context = command_context_from_state(&state);

        assert!(context.has_active_database);
        assert_eq!(context.mode, state.workbench.mode);
        assert_eq!(context.focus, FocusContext::Workspace);
        assert!(!context.live_available);
    }

    #[test]
    fn toggle_live_remains_disabled_until_phase4_capability_probe() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());
        let context = command_context_from_state(&state);

        assert_eq!(command_availability(CommandId::ToggleLive, &context), Availability::Disabled(DisabledReason::LiveUnavailable));
    }

    #[test]
    fn policy_and_registry_agree_on_refresh_availability() {
        let state = AppState::new("http://localhost:3000".to_string());
        let context = command_context_from_state(&state);

        assert_eq!(command_availability(CommandId::RefreshActiveResource, &context), Availability::Disabled(DisabledReason::NoActiveResource));
    }
}
```

Typed failure contract: `ReadFailure::Transport(String)` and `ScopedReadFailed { context, failure, retry: Option<ReadOperation> }` preserve the exact retry operation for Phase 4 without making `AppEvent` derive `PartialEq`.

If `crate::user_config::SessionState` does not already derive `PartialEq, Eq`, add those derives in Task 4 so `Effect::PersistSession { snapshot }` can be asserted exactly in navigation and runner tests.

`pub fn reduce(state: &mut AppState, event: AppEvent) -> Transition` is the locked reducer contract for Phase 2 and Phase 3. `Transition` and `Effect` derive `PartialEq, Eq` for Phase 3 normalized-action reducer assertions; `AppEvent` and `ReadResult` do not derive equality because their real payloads do not guarantee it.

Create `src/app/reducer.rs`:

```rust
use crate::app::command::CommandId;
use crate::app::event::{Action, AppEvent, Effect, TableTarget, Transition};
use crate::app::policy::{command_availability, command_context_from_state};
use crate::effects::request::RequestScope;
use crate::state::app_state::AppState;

pub fn reduce(state: &mut AppState, event: AppEvent) -> Transition {
    match event {
        AppEvent::Action(Action::Invoke(command)) => invoke_command(state, command),
        _ => Transition::none(),
    }
}

fn invoke_command(state: &mut AppState, command: CommandId) -> Transition {
    let context = command_context_from_state(state);
    if command_availability(command, &context) != crate::app::command::Availability::Available {
        return Transition::none();
    }
    match command {
        CommandId::RefreshActiveResource | CommandId::RefreshCurrentView => {
            match (&state.navigation.active_database, &state.navigation.active_resource) {
                (Some(database), Some(resource)) => Transition::effects(vec![Effect::LoadTableRows {
                    target: TableTarget { database: database.clone(), table: resource.clone() },
                    context: state.requests.next_context(RequestScope::TableRows { database: database.clone(), table: resource.clone(), view: "browse".into() }),
                }]),
                _ => Transition::none(),
            }
        }
        _ => Transition::none(),
    }
}
```

Add these module lines to `src/app.rs` near `pub mod command;`:

```rust
pub mod event;
pub mod policy;
pub mod reducer;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::reducer::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/app/command.rs src/app/event.rs src/app/policy.rs src/app/reducer.rs src/user_config.rs
git commit -m "feat: add normalized events and reducer transition"
```

### Task 5: Semantic navigation emits scoped RequestContext effects

**Files:**
- Create: `src/app/navigation.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/app/navigation.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::Effect;
    use crate::effects::request::RequestScope;
    use crate::state::app_state::AppState;

    #[test]
    fn select_resource_updates_navigation_and_emits_scoped_load_once() {
        let mut state = AppState::new("http://localhost:3000".to_string());

        let transition = select_resource(&mut state, "inventory".to_string(), "items".to_string());

        assert_eq!(state.navigation.active_database.as_deref(), Some("inventory"));
        assert_eq!(state.navigation.active_resource.as_deref(), Some("items"));
        assert!(matches!(transition.effects.as_slice(), [Effect::LoadTableRows { target, context }, Effect::PersistSession { snapshot }] if target.database == "inventory" && target.table == "items" && context.scope() == &RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() } && snapshot.last_database.as_deref() == Some("inventory") && snapshot.last_table.as_deref() == Some("items") && snapshot.last_tab == Some(0)));
    }

    #[test]
    fn navigate_up_from_resource_clears_resource_and_emits_schema_reload() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());

        let transition = navigate_up(&mut state);

        assert_eq!(state.navigation.active_database.as_deref(), Some("inventory"));
        assert_eq!(state.navigation.active_resource, None);
        assert!(matches!(transition.effects.as_slice(), [Effect::LoadSchema { database, context }, Effect::PersistSession { snapshot }] if database == "inventory" && context.scope() == &RequestScope::Schema { database: "inventory".into() } && snapshot.last_database.as_deref() == Some("inventory") && snapshot.last_table.is_none() && snapshot.last_tab == Some(0)));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::navigation::tests`

Expected: FAIL with missing module `navigation` or missing functions `select_resource` and `navigate_up`.

- [ ] **Step 3: Write minimal implementation**

Create `src/app/navigation.rs`:

```rust
use crate::app::event::{Effect, TableTarget, Transition};
use crate::effects::request::RequestScope;
use crate::state::app_state::AppState;

fn session_snapshot(state: &AppState) -> crate::user_config::SessionState {
    crate::user_config::SessionState {
        last_database: state.navigation.active_database.clone(),
        last_table: state.navigation.active_resource.clone(),
        last_tab: Some(match state.workbench.workspace {
            crate::state::workbench::Workspace::Tables => 0,
            crate::state::workbench::Workspace::Sql => 1,
            crate::state::workbench::Workspace::Logs => 2,
            crate::state::workbench::Workspace::Metrics => 3,
            crate::state::workbench::Workspace::Module => 4,
            crate::state::workbench::Workspace::Live => 5,
        }),
    }
}

pub fn select_database(state: &mut AppState, database: String) -> Transition {
    state.navigation.active_database = Some(database.clone());
    state.navigation.active_resource = None;
    Transition::effects(vec![
        Effect::LoadSchema { database: database.clone(), context: state.requests.next_context(RequestScope::Schema { database }) },
        Effect::PersistSession { snapshot: session_snapshot(state) },
    ])
}

pub fn select_resource(state: &mut AppState, database: String, resource: String) -> Transition {
    state.navigation.active_database = Some(database.clone());
    state.navigation.active_resource = Some(resource.clone());
    Transition::effects(vec![
        Effect::LoadTableRows {
            target: TableTarget { database: database.clone(), table: resource.clone() },
            context: state.requests.next_context(RequestScope::TableRows { database, table: resource, view: "browse".into() }),
        },
        Effect::PersistSession { snapshot: session_snapshot(state) },
    ])
}

pub fn navigate_up(state: &mut AppState) -> Transition {
    if state.navigation.active_resource.take().is_some() {
        if let Some(database) = state.navigation.active_database.clone() {
            return Transition::effects(vec![
                Effect::LoadSchema { database: database.clone(), context: state.requests.next_context(RequestScope::Schema { database }) },
                Effect::PersistSession { snapshot: session_snapshot(state) },
            ]);
        }
    }
    state.navigation.active_database = None;
    Transition::effects(vec![Effect::LoadCatalog { context: state.requests.next_context(RequestScope::DatabaseCatalog) }, Effect::PersistSession { snapshot: session_snapshot(state) }])
}
```

Task 5 adds the reducer arms for typed navigation actions. Update `src/app/reducer.rs` to call these helpers for typed selection actions with this exact diff:

```rust
        AppEvent::Action(Action::SelectDatabase { database }) => crate::app::navigation::select_database(state, database),
        AppEvent::Action(Action::SelectResource { database, resource }) => crate::app::navigation::select_resource(state, database, resource),
        AppEvent::Action(Action::NavigateUp) => crate::app::navigation::navigate_up(state),
```

Add this module line to `src/app.rs` alongside the Task 4 `event`, `policy`, and `reducer` declarations; do not add it twice:

```rust
pub mod navigation;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::navigation::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/app/navigation.rs src/app/reducer.rs
git commit -m "feat: route semantic navigation through scoped effects"
```

### Task 6: LoadState<T> scoped transitions and stale-result rejection

**Files:**
- Modify: `src/state/resources.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/app/event.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/state/resources.rs`:

```rust
#[cfg(test)]
mod load_state_tests {
    use super::*;
    use crate::effects::request::{RequestContext, RequestId, RequestScope};

    #[test]
    fn refresh_keeps_existing_typed_query_result_visible() {
        let request = RequestContext::new(RequestId::from_u64(1), RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() }, 1);
        let state = LoadState::Ready { data: QueryResult { schema: vec![], rows: vec![], total_duration_micros: 0 }, refreshed_at: Instant::now() };

        let next = state.begin_read(request.clone());

        assert!(matches!(next, LoadState::Refreshing { request: actual, .. } if actual == request));
    }

    #[test]
    fn stale_completion_does_not_replace_newer_request_context() {
        let current = RequestContext::new(RequestId::from_u64(8), RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() }, 3);
        let stale = RequestContext::new(RequestId::from_u64(7), RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() }, 2);
        let state = LoadState::<TableRows>::Loading { request: current.clone() };

        let next = state.apply_success(&stale, QueryResult { schema: vec![], rows: vec![], total_duration_micros: 0 }, Instant::now());

        assert!(matches!(next, LoadState::Loading { request } if request == current));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test state::resources::load_state_tests`

Expected: FAIL with missing methods `begin_read` and `apply_success` or missing typed resource wrappers.

- [ ] **Step 3: Write minimal implementation**

Add this impl to `src/state/resources.rs`:

```rust
impl<T: Clone> LoadState<T> {
    pub fn begin_read(self, request: RequestContext) -> Self {
        match self {
            LoadState::Ready { data, .. } | LoadState::Refreshing { data, .. } | LoadState::Stale { data, .. } => {
                LoadState::Refreshing { data, request }
            }
            LoadState::Error { previous: Some(data), .. } => LoadState::Refreshing { data, request },
            _ => LoadState::Loading { request },
        }
    }

    pub fn apply_success(self, completed: &RequestContext, data: T, refreshed_at: Instant) -> Self {
        match &self {
            LoadState::Loading { request } | LoadState::Refreshing { request, .. }
                if request == completed => LoadState::Ready { data, refreshed_at },
            _ => self,
        }
    }

    pub fn apply_empty(self, completed: &RequestContext, refreshed_at: Instant) -> Self {
        match &self {
            LoadState::Loading { request } | LoadState::Refreshing { request, .. }
                if request == completed => LoadState::Empty { refreshed_at },
            _ => self,
        }
    }

    pub fn apply_error(self, completed: &RequestContext, error: AppError) -> Self {
        match self {
            LoadState::Loading { request } if &request == completed => LoadState::Error { previous: None, error },
            LoadState::Refreshing { data, request } if &request == completed => LoadState::Error { previous: Some(data), error },
            other => other,
        }
    }
}
```

Update `src/app/reducer.rs` to apply scoped completions only when `state.requests.is_current(&context)` and the Phase 1 `RequestContext` matches the currently loading resource:

```rust
        AppEvent::ScopedReadCompleted { context, result } => {
            if state.requests.is_current(&context) {
                apply_scoped_read_completion(state, context, result);
            }
            Transition::none()
        }
        AppEvent::ScopedReadFailed { context, failure, retry: _ } => {
            if state.requests.is_current(&context) {
                apply_scoped_read_failure(state, context, failure);
            }
            // Phase 2 retains the typed retry identity on the event but does
            // not schedule retries until Phase 4 installs retry policy.
            Transition::none()
        }
```

Add the failure helper in `src/app/reducer.rs` in this task, after `LoadState::apply_error` exists. It consumes and replaces exactly one resource state and never mutates a stale scope:

```rust
fn apply_scoped_read_failure(
    state: &mut AppState,
    context: RequestContext,
    failure: crate::app::event::ReadFailure,
) {
    use crate::effects::request::RequestScope;
    use crate::state::resources::AppError;

    let scope = context.scope.clone();
    let error = AppError {
        message: match failure {
            crate::app::event::ReadFailure::Transport(message) => message,
        },
    };
    match scope {
        RequestScope::DatabaseCatalog => {
            state.resources.catalog = std::mem::take(&mut state.resources.catalog)
                .apply_error(&context, error);
        }
        RequestScope::Schema { .. } => {
            state.resources.schema = std::mem::take(&mut state.resources.schema)
                .apply_error(&context, error);
        }
        RequestScope::TableRows { .. } | RequestScope::SqlWorkspace { .. } => {
            state.resources.table_rows = std::mem::take(&mut state.resources.table_rows)
                .apply_error(&context, error);
        }
        RequestScope::Logs { .. } => {
            state.resources.logs = std::mem::take(&mut state.resources.logs)
                .apply_error(&context, error);
        }
        RequestScope::Metrics { .. } => {
            state.resources.metrics = std::mem::take(&mut state.resources.metrics)
                .apply_error(&context, error);
        }
        RequestScope::LiveClients { .. } => {
            state.resources.live_clients = std::mem::take(&mut state.resources.live_clients)
                .apply_error(&context, error);
        }
    }
}
```

Add reducer tests in Task 6 proving a current `ScopedReadFailed` moves the matching `Loading` state to `Error`, a failure for an older `RequestContext` leaves the newer load untouched, and the retained `retry: Some(ReadOperation)` emits no retry effect in Phase 2.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test state::resources::load_state_tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/state/resources.rs src/app/event.rs src/app/reducer.rs
git commit -m "feat: add scoped load state transitions"
```

### Task 7: Deterministic runtime ports and effect runner boundary

**Files:**
- Modify: `src/effects/mod.rs`
- Create: `src/effects/ports.rs`
- Create: `src/effects/runner.rs`
- Modify: `src/main.rs` to declare the `effects` module for this binary crate if not already declared

Phase 1 already creates `src/effects/mod.rs`, `src/effects/task_registry.rs`, concrete `TaskRegistry` with `TaskRegistry::new()`, and `src/terminal.rs::TerminalOps`. Reuse those definitions. Do not redefine competing `TaskRegistry` or `TerminalOps` traits in `src/effects/runner.rs`; `TaskRegistry` is a struct, not a trait.

- [ ] **Step 1: Write the failing tests**

Create `src/effects/ports.rs` with these deterministic port traits and tests first:

```rust
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::app::event::{AppEvent, ReadOperation};
use crate::effects::request::RequestContext;

pub trait ApiTransport: Send + Sync {
    fn execute_read(&self, operation: ReadOperation, context: RequestContext) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>;
}

pub trait SubscriptionTransport: Send + Sync {
    type Request: Send;

    fn connect(&self, request: Self::Request) -> Pin<Box<dyn Future<Output = AppEvent> + Send + '_>>;
}

pub trait SessionStore: Send + Sync {
    fn save(&self, snapshot: crate::user_config::SessionState) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>;
}

pub trait Clock { fn now(&self) -> Instant; }
pub trait Sleeper { fn sleep(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>; }
pub trait RandomSource { fn next_u64(&mut self) -> u64; }
pub trait IdSource { fn next_id(&mut self) -> u64; }
pub trait BackoffPolicy { fn delay(&self, attempt: u32, random: &mut dyn RandomSource) -> Duration; }

#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FakeClock { now: Instant }

impl FakeClock { pub fn new(now: Instant) -> Self { Self { now } } }

impl Clock for FakeClock { fn now(&self) -> Instant { self.now } }

#[cfg(test)]
pub struct FakeRandomSource { values: Vec<u64> }

impl FakeRandomSource { pub fn new(values: Vec<u64>) -> Self { Self { values } } }

impl RandomSource for FakeRandomSource {
    fn next_u64(&mut self) -> u64 { self.values.remove(0) }
}

#[cfg(test)]
pub struct FakeIdSource { value: u64 }

impl FakeIdSource { pub fn new(value: u64) -> Self { Self { value } } }

impl IdSource for FakeIdSource {
    fn next_id(&mut self) -> u64 { self.value += 1; self.value }
}

#[cfg(test)]
pub struct FixedBackoffPolicy { pub base: Duration }

impl BackoffPolicy for FixedBackoffPolicy {
    fn delay(&self, attempt: u32, random: &mut dyn RandomSource) -> Duration {
        self.base * attempt + Duration::from_millis(random.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ports_work_without_real_time_or_randomness() {
        let mut ids = FakeIdSource::new(40);
        let mut random = FakeRandomSource::new(vec![3]);
        let backoff = FixedBackoffPolicy { base: Duration::from_millis(100) };
        let clock = FakeClock::new(Instant::now());

        assert_eq!(ids.next_id(), 41);
        assert_eq!(random.next_u64(), 3);
        let mut random = FakeRandomSource::new(vec![3]);
        assert_eq!(backoff.delay(2, &mut random), Duration::from_millis(203));
        assert_eq!(clock.now(), clock.now());
    }
}
```

Create `src/effects/runner.rs` with this test module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::AppEvent;

    #[test]
    fn bounded_event_channel_reports_capacity_and_refuses_overflow() {
        let channel = bounded_event_channel(2);

        assert_eq!(channel.capacity(), 2);
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test effects::ports::tests`

Expected: FAIL with missing `ports` module until `src/effects/mod.rs` exports it.

Run: `cargo test effects::runner::tests`

Expected: FAIL with missing `runner` module or missing `bounded_event_channel`.

- [ ] **Step 3: Write minimal implementation**

Modify existing `src/effects/mod.rs` to export all Phase 1 and Phase 2 effects modules:

```rust
pub mod ports;
pub mod runner;
pub mod task_registry;
```

Keep `src/effects/ports.rs` as above. `ApiTransport::execute_read` takes a stable typed `ReadOperation` and the exact `RequestContext`; it must never accept `scope: String`, derive a dispatch key from `RequestScope::Display`, or lower domain identity to a formatted string. `RequestScope::Display` remains user-facing text only. TableRows adapter uses `crate::effects::write_ops::encode_identifier` before formatting `SELECT * FROM ... LIMIT 200`; raw table names must never be interpolated. Identifier encoding failures become `ScopedReadFailed` with `retry: None` because deterministic local validation failures must not enter the read retry loop. Phase 4 retry reuses the exact `ReadOperation` and `RequestContext` pair retained by retryable transport failures rather than reconstructing a string scope. Create `src/effects/runner.rs` with bounded AppEvent channel construction and an owned effect boundary. `dispatch` must not return a bare future because app integration can drop it unpolled; every read future is spawned through `TaskRegistry` and completion is delivered through the bounded `AppEvent` sender:

```rust
use crate::app::event::{AppEvent, Effect, ReadFailure, ReadOperation, ReadResult};
use crate::effects::ports::{ApiTransport, SessionStore, SubscriptionTransport};
use std::sync::Arc;
use crate::api::SpacetimeClient;
use crate::effects::task_registry::TaskRegistry;
use crate::effects::write_ops::encode_identifier;
use crate::terminal::CrosstermTerminalOps;
use crate::terminal::TerminalOps;
use tokio::sync::mpsc;

pub struct BoundedEventChannel {
    sender: mpsc::Sender<AppEvent>,
    receiver: mpsc::Receiver<AppEvent>,
    capacity: usize,
}

impl BoundedEventChannel {
    pub fn capacity(&self) -> usize { self.capacity }
    pub fn sender(&self) -> mpsc::Sender<AppEvent> { self.sender.clone() }
    pub fn split(self) -> (mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>) { (self.sender, self.receiver) }
    pub fn try_send(&self, event: AppEvent) -> Result<(), mpsc::error::TrySendError<AppEvent>> { self.sender.try_send(event) }
    pub async fn recv(&mut self) -> Option<AppEvent> { self.receiver.recv().await }
}

pub fn bounded_event_channel(capacity: usize) -> BoundedEventChannel {
    // Explicitly use the bounded Tokio MPSC channel for AppEvent delivery. Do not introduce any unbounded AppEvent channel.
    let (sender, receiver) = mpsc::channel(capacity);
    BoundedEventChannel { sender, receiver, capacity }
}


#[derive(Clone)]
pub struct SpacetimeApiTransport {
    client: SpacetimeClient,
}

impl SpacetimeApiTransport {
    pub fn new(client: SpacetimeClient) -> Self { Self { client } }
}

impl ApiTransport for SpacetimeApiTransport {
    fn execute_read(&self, operation: ReadOperation, context: crate::effects::request::RequestContext) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + 'static>> {
        let client = self.client.clone();
        let retry = operation.clone();
        Box::pin(async move {
            let result = match operation.clone() {
                ReadOperation::Catalog => client.list_databases().await.map(ReadResult::Catalog),
                ReadOperation::Schema { database } => client.get_schema(&database).await.map(ReadResult::Schema),
                ReadOperation::TableRows { target } => {
                    match encode_identifier(&target.table) {
                        Ok(table) => client.query_sql(&target.database, &format!("SELECT * FROM {table} LIMIT 200")).await.map(ReadResult::TableRows),
                        Err(error) => {
                            return AppEvent::ScopedReadFailed {
                                context,
                                failure: ReadFailure::Transport(error.to_string()),
                                retry: None,
                            };
                        }
                    }
                }
            };
            match result {
                Ok(result) => AppEvent::ScopedReadCompleted { context, result },
                Err(error) => AppEvent::ScopedReadFailed { context, failure: ReadFailure::Transport(error.to_string()), retry: Some(retry) },
            }
        })
    }
}

pub struct Phase2SubscriptionTransport;

impl SubscriptionTransport for Phase2SubscriptionTransport {
    type Request = std::convert::Infallible;

    fn connect(&self, request: Self::Request) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + '_>> {
        match request {}
    }
}

pub struct LocalSessionStore;

impl SessionStore for LocalSessionStore {
    fn save(&self, snapshot: crate::user_config::SessionState) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + 'static>> {
        Box::pin(async move {
            snapshot.save();
            AppEvent::Tick
        })
    }
}

pub type AppEffectRunner = EffectRunner<SpacetimeApiTransport, Phase2SubscriptionTransport, CrosstermTerminalOps>;

pub struct EffectRunner<A, S, T> {
    pub api: A,
    pub subscriptions: S,
    pub terminal: T,
    pub tasks: TaskRegistry,
    pub sessions: Arc<dyn SessionStore>,
    event_sender: mpsc::Sender<AppEvent>,
}

impl<A, S, T> EffectRunner<A, S, T>
where
    A: ApiTransport,
    S: SubscriptionTransport,
    T: TerminalOps,
{
    pub fn new(api: A, subscriptions: S, terminal: T, tasks: TaskRegistry, sessions: Arc<dyn SessionStore>, event_sender: mpsc::Sender<AppEvent>) -> Self {
        Self { api, subscriptions, terminal, tasks, sessions, event_sender }
    }

    pub fn dispatch(&mut self, effect: Effect) -> Option<crate::effects::task_registry::TaskId> {
        let (name, future) = match effect {
            Effect::LoadCatalog { context } => ("read catalog", self.api.execute_read(ReadOperation::Catalog, context)),
            Effect::LoadSchema { database, context } => ("read schema", self.api.execute_read(ReadOperation::Schema { database }, context)),
            Effect::LoadTableRows { target, context } => ("read table rows", self.api.execute_read(ReadOperation::TableRows { target }, context)),
            Effect::PersistSession { snapshot } => {
                // PersistSession is local session I/O and must not be sent to the network ApiTransport.
                ("persist session", self.sessions.save(snapshot))
            }
        };
        let sender = self.event_sender.clone();
        Some(self.tasks.spawn(name, async move {
            let event = future.await;
            let _ = sender.send(event).await;
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::AppEvent;

    #[test]
    fn bounded_event_channel_reports_capacity_and_refuses_overflow() {
        let channel = bounded_event_channel(2);

        assert_eq!(channel.capacity(), 2);
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_err());
    }
}
```

The bounded channel is split at app startup: the receiver stays in the app event loop, and `EffectRunner::new` receives a cloned sender. The runner owns all safety-relevant async read work through `TaskRegistry`; no read future may be returned to callers or detached.

Add this module line near other root modules in the binary crate root `src/main.rs` if missing:

```rust
mod effects;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test effects::ports::tests`

Run: `cargo test effects::runner::tests`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/effects/mod.rs src/effects/ports.rs src/effects/runner.rs src/main.rs
git commit -m "feat: add deterministic effect runner ports"
```

### Task 8: Migrate widget and input state into central WorkbenchState

**Files:**
- Modify: `src/state/workbench.rs`
- Modify: `src/app.rs`
- Modify: `src/ui/components/input.rs`
- Modify: `src/ui/components/table_grid.rs`
- Modify: `src/ui/components/palette.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/state/workbench.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test state::workbench::workbench_state_tests::sql_input_grid_cursor_palette_and_help_are_owned_by_workbench_state`

Expected: FAIL with missing methods `set_sql_input`, `set_grid_cursor`, `open_palette`, and `open_help`.

- [ ] **Step 3: Write minimal implementation**

Add this impl to `src/state/workbench.rs`:

```rust
impl WorkbenchState {
    pub fn set_sql_input(&mut self, sql: String) {
        self.sql_input = sql;
    }

    pub fn set_grid_cursor(&mut self, row: usize, column: usize) {
        self.table_grid_cursor = Some((row, column));
    }

    pub fn open_palette(&mut self) {
        self.palette_open = true;
        self.help_open = false;
    }

    pub fn open_help(&mut self) {
        self.help_open = true;
        self.palette_open = false;
    }
}
```

In `src/app.rs`, replace app-level fields that duplicate SQL input, table cursor, palette open, help open, modal open, or focused pane with reads and writes through `self.state.workbench`. Use these exact accessor methods if direct field migration is too large for one commit:

```rust
fn workbench_state(&self) -> &crate::state::workbench::WorkbenchState {
    &self.state.workbench
}

fn workbench_state_mut(&mut self) -> &mut crate::state::workbench::WorkbenchState {
    &mut self.state.workbench
}
```

In `src/ui/components/input.rs`, pass the SQL text as `&state.workbench.sql_input` from call sites instead of reading an app-level duplicate.

In `src/ui/components/table_grid.rs`, pass `state.workbench.table_grid_cursor` from call sites instead of reading an app-level duplicate.

In `src/ui/components/palette.rs`, pass `state.workbench.palette_open` from call sites instead of reading an app-level duplicate.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test state::workbench::workbench_state_tests::sql_input_grid_cursor_palette_and_help_are_owned_by_workbench_state && cargo test`

Expected: PASS for the focused test and the full test suite.

- [ ] **Step 5: Commit**

```bash
git add src/state/workbench.rs src/app.rs src/ui/components/input.rs src/ui/components/table_grid.rs src/ui/components/palette.rs
git commit -m "refactor: centralize widget and input state"
```

### Task 9: ConfigSource, Resolved<T>, Clap ValueSource, CLI/env/lower precedence, --no-tls, and custom theme strings

**Files:**
- Modify: `src/config.rs`
- Modify: `src/main.rs`
- Modify: `src/user_config.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/config.rs`:

```rust
#[cfg(test)]
mod phase2_config_tests {
    use super::*;
    use clap::parser::ValueSource;

    #[test]
    fn explicit_cli_default_looking_values_win_over_environment_and_detected_config() {
        let input = ConfigResolutionInput {
            cli_host: Some(Resolved::new("localhost".to_string(), ConfigSource::CommandLine)),
            cli_port: Some(Resolved::new(3000, ConfigSource::CommandLine)),
            cli_tls: Some(Resolved::new(false, ConfigSource::CommandLine)),
            cli_token: None,
            cli_database: None,
            cli_theme: None,
            env_host: Some("remote.example".to_string()),
            env_port: Some(443),
            env_tls: Some(true),
            env_token: Some("env-token".to_string()),
            env_database: None,
            env_theme: None,
            restored_database: None,
            user_default_database: None,
            user_theme: Some("custom-user-theme".to_string()),
            detected_host: Some("detected.example".to_string()),
            detected_port: Some(80),
            detected_tls: Some(true),
            detected_token: Some("detected-token".to_string()),
            restore_session: true,
        };

        let resolved = resolve_config(input);

        assert_eq!(resolved.host, Resolved::new("localhost".to_string(), ConfigSource::CommandLine));
        assert_eq!(resolved.port, Resolved::new(3000, ConfigSource::CommandLine));
        assert_eq!(resolved.tls, Resolved::new(false, ConfigSource::CommandLine));
        assert_eq!(resolved.token, Resolved::new(Some("env-token".to_string()), ConfigSource::Environment));
        assert_eq!(resolved.theme, Resolved::new("custom-user-theme".to_string(), ConfigSource::UserConfig));
    }

    #[test]
    fn clap_value_source_maps_explicit_no_tls_to_command_line() {
        assert_eq!(config_source_from_clap(ValueSource::CommandLine), ConfigSource::CommandLine);
        assert_eq!(config_source_from_clap(ValueSource::DefaultValue), ConfigSource::BuiltIn);
    }

    #[test]
    fn cli_theme_accepts_custom_theme_string() {
        let input = ConfigResolutionInput {
            cli_theme: Some(Resolved::new("solarized-company".to_string(), ConfigSource::CommandLine)),
            ..ConfigResolutionInput::empty()
        };

        let resolved = resolve_config(input);

        assert_eq!(resolved.theme, Resolved::new("solarized-company".to_string(), ConfigSource::CommandLine));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test config::phase2_config_tests`

Expected: FAIL with missing `ConfigSource`, `Resolved`, `ConfigResolutionInput`, `resolve_config`, or `config_source_from_clap`.

- [ ] **Step 3: Write minimal implementation**

Add these types and functions to `src/config.rs` without removing existing public config types:

```rust
use clap::parser::ValueSource;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSource {
    CommandLine,
    Environment,
    RestoredSession,
    UserConfig,
    SpacetimeCliConfig,
    BuiltIn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: ConfigSource,
}

impl<T> Resolved<T> {
    pub fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfig {
    pub host: Resolved<String>,
    pub port: Resolved<u16>,
    pub tls: Resolved<bool>,
    pub token: Resolved<Option<String>>,
    pub database: Resolved<Option<String>>,
    pub theme: Resolved<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigResolutionInput {
    pub cli_host: Option<Resolved<String>>,
    pub cli_port: Option<Resolved<u16>>,
    pub cli_tls: Option<Resolved<bool>>,
    pub cli_token: Option<Resolved<String>>,
    pub cli_database: Option<Resolved<String>>,
    pub cli_theme: Option<Resolved<String>>,
    pub env_host: Option<String>,
    pub env_port: Option<u16>,
    pub env_tls: Option<bool>,
    pub env_token: Option<String>,
    pub env_database: Option<String>,
    pub env_theme: Option<String>,
    pub restored_database: Option<String>,
    pub user_default_database: Option<String>,
    pub user_theme: Option<String>,
    pub detected_host: Option<String>,
    pub detected_port: Option<u16>,
    pub detected_tls: Option<bool>,
    pub detected_token: Option<String>,
    pub restore_session: bool,
}

impl ConfigResolutionInput {
    pub fn empty() -> Self {
        Self {
            cli_host: None,
            cli_port: None,
            cli_tls: None,
            cli_token: None,
            cli_database: None,
            cli_theme: None,
            env_host: None,
            env_port: None,
            env_tls: None,
            env_token: None,
            env_database: None,
            env_theme: None,
            restored_database: None,
            user_default_database: None,
            user_theme: None,
            detected_host: None,
            detected_port: None,
            detected_tls: None,
            detected_token: None,
            restore_session: true,
        }
    }
}

pub fn config_source_from_clap(source: ValueSource) -> ConfigSource {
    match source {
        ValueSource::CommandLine => ConfigSource::CommandLine,
        ValueSource::EnvVariable => ConfigSource::Environment,
        ValueSource::DefaultValue => ConfigSource::BuiltIn,
    }
}

pub fn resolve_config(input: ConfigResolutionInput) -> ResolvedConfig {
    let host = input.cli_host
        .or_else(|| input.env_host.map(|value| Resolved::new(value, ConfigSource::Environment)))
        .or_else(|| input.detected_host.map(|value| Resolved::new(value, ConfigSource::SpacetimeCliConfig)))
        .unwrap_or_else(|| Resolved::new("localhost".to_string(), ConfigSource::BuiltIn));

    let port = input.cli_port
        .or_else(|| input.env_port.map(|value| Resolved::new(value, ConfigSource::Environment)))
        .or_else(|| input.detected_port.map(|value| Resolved::new(value, ConfigSource::SpacetimeCliConfig)))
        .unwrap_or_else(|| Resolved::new(3000, ConfigSource::BuiltIn));

    let tls = input.cli_tls
        .or_else(|| input.env_tls.map(|value| Resolved::new(value, ConfigSource::Environment)))
        .or_else(|| input.detected_tls.map(|value| Resolved::new(value, ConfigSource::SpacetimeCliConfig)))
        .unwrap_or_else(|| Resolved::new(false, ConfigSource::BuiltIn));

    let token = input.cli_token.map(|resolved| Resolved::new(Some(resolved.value), resolved.source))
        .or_else(|| input.env_token.map(|value| Resolved::new(Some(value), ConfigSource::Environment)))
        .or_else(|| input.detected_token.map(|value| Resolved::new(Some(value), ConfigSource::SpacetimeCliConfig)))
        .unwrap_or_else(|| Resolved::new(None, ConfigSource::BuiltIn));

    let database = input.cli_database.map(|resolved| Resolved::new(Some(resolved.value), resolved.source))
        .or_else(|| input.env_database.map(|value| Resolved::new(Some(value), ConfigSource::Environment)))
        .or_else(|| input.restore_session.then(|| input.restored_database.map(|value| Resolved::new(Some(value), ConfigSource::RestoredSession))).flatten())
        .or_else(|| input.user_default_database.map(|value| Resolved::new(Some(value), ConfigSource::UserConfig)))
        .unwrap_or_else(|| Resolved::new(None, ConfigSource::BuiltIn));

    let theme = input.cli_theme
        .or_else(|| input.env_theme.map(|value| Resolved::new(value, ConfigSource::Environment)))
        .or_else(|| input.user_theme.map(|value| Resolved::new(value, ConfigSource::UserConfig)))
        .unwrap_or_else(|| Resolved::new("dark".to_string(), ConfigSource::BuiltIn));

    ResolvedConfig { host, port, tls, token, database, theme }
}
```

In the Clap args type in `src/main.rs` or `src/config.rs`, ensure TLS has explicit true and false options:

```rust
#[arg(long = "tls", action = clap::ArgAction::SetTrue)]
pub tls: bool,

#[arg(long = "no-tls", action = clap::ArgAction::SetTrue)]
pub no_tls: bool,
```

Change theme CLI field from an enum-only type to a string:

```rust
#[arg(long)]
pub theme: Option<String>,
```

At startup, obtain source with Clap after parsing:

```rust
let matches = Cli::command().get_matches();
let theme_source = matches.value_source("theme").map(config_source_from_clap);
let tls_source = matches.value_source("tls").or_else(|| matches.value_source("no-tls")).map(config_source_from_clap);
```

Use `tls_source == Some(ConfigSource::CommandLine)` to preserve explicit `--tls` and `--no-tls` rather than comparing parsed TLS to the default.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test config::phase2_config_tests && cargo test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/main.rs src/user_config.rs
git commit -m "feat: preserve configuration source precedence"
```

### Task 10: UserConfig default restore_session=true and deterministic session mapping

**Files:**
- Modify: `src/user_config.rs`
- Modify: `src/state/navigation.rs`
- Modify: `src/state/workbench.rs`
- Modify: `src/state/app_state.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/user_config.rs`:

```rust
#[cfg(test)]
mod phase2_user_config_tests {
    use super::*;
    use crate::state::app_state::AppState;
    use crate::state::workbench::{WorkbenchMode, Workspace};

    #[test]
    fn default_user_config_restores_session() {
        assert!(UserConfig::default().restore_session);
    }

    #[test]
    fn legacy_session_tabs_map_deterministically_to_single_navigation_and_workbench_owners() {
        let session = SessionState {
            last_database: Some("inventory".to_string()),
            last_tab: Some(1),
            ..SessionState::default()
        };

        let state = AppState::from_session_state("http://localhost:3000".to_string(), &session);

        assert_eq!(state.navigation.active_database.as_deref(), Some("inventory"));
        assert_eq!(state.workbench.mode, WorkbenchMode::Data);
        assert_eq!(state.workbench.workspace, Workspace::Sql);
    }

    #[test]
    fn invalid_legacy_session_tab_falls_back_to_safe_data_tables_workspace() {
        let session = SessionState {
            last_database: Some("inventory".to_string()),
            last_tab: Some(99),
            ..SessionState::default()
        };

        let state = AppState::from_session_state("http://localhost:3000".to_string(), &session);

        assert_eq!(state.navigation.active_database.as_deref(), Some("inventory"));
        assert_eq!(state.workbench.mode, WorkbenchMode::Data);
        assert_eq!(state.workbench.workspace, Workspace::Tables);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test user_config::phase2_user_config_tests`

Expected: FAIL if `restore_session` defaults false or `AppState::from_session_state` is missing.

- [ ] **Step 3: Write minimal implementation**

In `src/user_config.rs`, remove `Default` derive from `UserConfig` if present and implement it manually:

```rust
impl Default for UserConfig {
    fn default() -> Self {
        Self {
            restore_session: true,
            ..Self::empty_defaults_except_restore_session()
        }
    }
}
```

Add the helper on `UserConfig` with every existing field set explicitly to its previous semantic default. If `UserConfig` has more fields, include every field explicitly.

In `src/state/app_state.rs`, map legacy session tabs into the single owners established in Task 1. Navigation owns only selected database/resource identity; workbench owns mode and workspace:

```rust
impl AppState {
    pub fn from_session_state(base_url: String, session: &crate::user_config::SessionState) -> Self {
        let (mode, workspace) = match session.last_tab {
            Some(1) => (crate::state::workbench::WorkbenchMode::Data, crate::state::workbench::Workspace::Sql),
            Some(2) => (crate::state::workbench::WorkbenchMode::Observe, crate::state::workbench::Workspace::Logs),
            Some(3) => (crate::state::workbench::WorkbenchMode::Observe, crate::state::workbench::Workspace::Metrics),
            Some(4) => (crate::state::workbench::WorkbenchMode::Operate, crate::state::workbench::Workspace::Module),
            Some(5) => (crate::state::workbench::WorkbenchMode::Observe, crate::state::workbench::Workspace::Live),
            Some(0) | None => (crate::state::workbench::WorkbenchMode::Data, crate::state::workbench::Workspace::Tables),
            Some(_) => (crate::state::workbench::WorkbenchMode::Data, crate::state::workbench::Workspace::Tables),
        };

        let mut state = Self::new(base_url);
        state.navigation.active_database = session.last_database.clone();
        state.navigation.active_resource = session.last_table.clone();
        state.workbench.mode = mode;
        state.workbench.workspace = workspace;
        state
    }
}
```

Do not add duplicate mode/workspace fields or legacy workspace/pane synonym types to `NavigationState`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test user_config::phase2_user_config_tests && cargo test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/user_config.rs src/state/navigation.rs src/state/workbench.rs src/state/app_state.rs
git commit -m "fix: align user config defaults and session mapping"
```

### Task 11: Incremental extraction cleanup from module-root `src/app.rs` into child files `src/app/{command,event,reducer,navigation,policy}.rs` without layout change

**Files:**
- Modify: `src/app/policy.rs`
- Modify: `src/app.rs`
- Modify: `src/app/command.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/app/navigation.rs`

- [ ] **Step 1: Write the failing tests**

Do not add a new policy test module in this task. Keep using the existing Task 4 `src/app/policy.rs` test module and add only cleanup assertions inside that existing module if needed. Add this plan-level assertion to the task notes so review can catch accidental duplicate policy definitions:

```text
Task 11 must not introduce another policy tests module, another command_context_from_state function, another focus_context_from_pane function, or another command_availability function.
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::policy::tests`

Expected: PASS if Task 4 policy was created correctly, or FAIL only for extraction cleanup issues introduced in this task.

- [ ] **Step 3: Write minimal implementation**

Keep the existing `src/app/policy.rs` from Task 4 and do not redefine competing policy functions or add a second `#[cfg(test)] mod tests`. In Task 11, only move remaining call sites to the existing `command_context_from_state`, `focus_context_from_pane`, and `command_availability` APIs, and add focused cleanup assertions if needed in the existing test module.

```text
No duplicate policy module, no duplicate command_availability function, and no second policy tests module are introduced in Task 11.
```

Ensure this module line from Task 4 remains in `src/app.rs`; do not add it twice:

```rust
pub mod policy;
```

`src/app.rs` remains the module root for this task and for all Phase 2 extraction tasks. Move only pure command, event, reducer, navigation, and policy code out of `src/app.rs`. Do not move any drawing functions, Ratatui layout code, or pane geometry code in this task. Where `src/app.rs` still needs a type, import it from the extracted modules:

```rust
use crate::app::command::{CommandId, CommandRegistry};
use crate::app::event::{Action, AppEvent, Effect, Transition};
use crate::app::reducer::reduce;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::policy::tests && cargo test`

Expected: PASS and no UI snapshot or layout test changes.

- [ ] **Step 5: Commit**

```bash
git add src/app.rs src/app/command.rs src/app/event.rs src/app/reducer.rs src/app/navigation.rs src/app/policy.rs
git commit -m "refactor: extract app policy foundation"
```

### Task 12: Effect runner dispatch integration with bounded channel

**Files:**
- Modify: `src/effects/runner.rs`
- Modify: `src/effects/ports.rs`
- Modify: `src/effects/mod.rs`
- Modify: `src/app.rs`
- Modify: `src/app/reducer.rs`

- [ ] **Step 1: Write the failing tests**

Append to `src/effects/runner.rs` tests:

```rust
    use crate::app::event::{Effect, ReadOperation, TableTarget};
    use crate::effects::request::{RequestContext, RequestId, RequestScope};
    use crate::effects::task_registry::TaskRegistry;
    use crate::terminal::TerminalOps;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn runner_dispatches_load_active_resource_through_typed_api_transport_port() {
        let api = RecordingApiTransport::default();
        let mut channel = bounded_event_channel(1);
        let mut runner = EffectRunner::new(api, NoopSubscriptionTransport, NoopTerminalOps, TaskRegistry::new(), Arc::new(RecordingSessionStore::default()), channel.sender());
        let target = TableTarget { database: "inventory".to_string(), table: "items".to_string() };
        let context = RequestContext::new(
            RequestId::from_u64(1),
            RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() },
            7,
        );

        let task_id = runner.dispatch(Effect::LoadTableRows { target: target.clone(), context: context.clone() }).unwrap();
        let event = channel.recv().await.unwrap();
        let report = runner.tasks.join_next().await.unwrap();
        let requests = runner.api.requests.lock().unwrap();

        assert_eq!(report.id, task_id);
        assert_eq!(report.outcome, crate::effects::task_registry::TaskOutcome::Completed);
        assert!(matches!(event, AppEvent::Tick));
        assert_eq!(*requests, vec![(ReadOperation::TableRows { target }, context)]);
        assert_eq!(requests[0].1.scope(), &RequestScope::TableRows { database: "inventory".into(), table: "items".into(), view: "browse".into() });
        assert_eq!(requests[0].1.generation, 7);
    }

    #[tokio::test]
    async fn runner_maps_catalog_schema_and_persist_session_without_string_dispatch() {
        let api = RecordingApiTransport::default();
        let mut channel = bounded_event_channel(4);
        let sessions = Arc::new(RecordingSessionStore::default());
        let mut runner = EffectRunner::new(api, NoopSubscriptionTransport, NoopTerminalOps, TaskRegistry::new(), sessions.clone(), channel.sender());
        let catalog_context = RequestContext::new(RequestId::from_u64(2), RequestScope::DatabaseCatalog, 1);
        let schema_context = RequestContext::new(RequestId::from_u64(3), RequestScope::Schema { database: "inventory".into() }, 4);

        let catalog_task = runner.dispatch(Effect::LoadCatalog { context: catalog_context.clone() }).unwrap();
        let schema_task = runner.dispatch(Effect::LoadSchema { database: "inventory".into(), context: schema_context.clone() }).unwrap();
        let snapshot = crate::user_config::SessionState { last_database: Some("inventory".into()), last_table: Some("items".into()), last_tab: Some(0) };
        let persist_task = runner.dispatch(Effect::PersistSession { snapshot: snapshot.clone() }).unwrap();
        let first_event = channel.recv().await.unwrap();
        let second_event = channel.recv().await.unwrap();
        let third_event = channel.recv().await.unwrap();
        let first_report = runner.tasks.join_next().await.unwrap();
        let second_report = runner.tasks.join_next().await.unwrap();
        let third_report = runner.tasks.join_next().await.unwrap();
        for event in [first_event, second_event, third_event] {
            assert!(matches!(event, AppEvent::Tick));
        }
        assert_eq!([first_report.outcome, second_report.outcome, third_report.outcome], [crate::effects::task_registry::TaskOutcome::Completed, crate::effects::task_registry::TaskOutcome::Completed, crate::effects::task_registry::TaskOutcome::Completed]);
        assert_ne!(catalog_task, schema_task);
        assert_ne!(persist_task, catalog_task);
        assert_eq!(sessions.snapshots.lock().unwrap().as_slice(), &[snapshot]);

        let requests = runner.api.requests.lock().unwrap();
        assert_eq!(*requests, vec![
            (ReadOperation::Catalog, catalog_context),
            (ReadOperation::Schema { database: "inventory".into() }, schema_context),
        ]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test effects::runner::tests::runner_dispatches_load_active_resource_through_typed_api_transport_port`

Expected: FAIL with missing `RecordingApiTransport`, `EffectRunner`, typed `ReadOperation` storage, or production adapter failure mapping.

- [ ] **Step 3: Write minimal implementation**

Extend the existing generic `EffectRunner<A, S, T>` from Task 7 only. Do not define another public `EffectRunner` struct, another runner owner, or a string formatter dispatch path in Task 12. Add test fixtures under `#[cfg(test)]` that implement the real Task 7 traits and record typed requests with interior mutability:

```rust
#[cfg(test)]
#[derive(Default)]
pub struct RecordingApiTransport {
    pub requests: std::sync::Mutex<Vec<(ReadOperation, RequestContext)>>,
}

#[cfg(test)]
impl ApiTransport for RecordingApiTransport {
    fn execute_read(&self, operation: ReadOperation, context: RequestContext) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>> {
        self.requests.lock().unwrap().push((operation, context));
        Box::pin(async { AppEvent::Tick })
    }
}

#[cfg(test)]
pub struct NoopSubscriptionTransport;

#[cfg(test)]
impl SubscriptionTransport for NoopSubscriptionTransport {
    type Request = ();

    fn connect(&self, _request: Self::Request) -> Pin<Box<dyn Future<Output = AppEvent> + Send + '_>> {
        Box::pin(async { AppEvent::Tick })
    }
}


#[cfg(test)]
#[derive(Default)]
pub struct RecordingSessionStore {
    pub snapshots: Mutex<Vec<crate::user_config::SessionState>>,
}

#[cfg(test)]
impl SessionStore for RecordingSessionStore {
    fn save(&self, snapshot: crate::user_config::SessionState) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>> {
        self.snapshots.lock().unwrap().push(snapshot);
        Box::pin(async { AppEvent::Tick })
    }
}

#[cfg(test)]
pub struct NoopTerminalOps;

#[cfg(test)]
impl TerminalOps for NoopTerminalOps {
    type Error = std::convert::Infallible;

    fn enable_raw_mode(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn hide_cursor(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn show_cursor(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> { Ok(()) }
    fn disable_raw_mode(&mut self) -> Result<(), Self::Error> { Ok(()) }
}
```

If a constructor is needed, add it only to the Task 7 generic impl as `EffectRunner::new(api, subscriptions, terminal, tasks: TaskRegistry, sessions: Arc<dyn SessionStore>, event_sender) -> Self`. Task 12 tests call the existing generic runner with local terminal fixtures, `TaskRegistry::new()`, a typed session store, and a bounded channel sender, then assert `dispatch` spawns and joins tasks, delivers `AppEvent::Tick` through the bounded receiver, and maps `Effect::LoadCatalog`, `Effect::LoadSchema`, and `Effect::LoadTableRows` to `ApiTransport::execute_read(ReadOperation, RequestContext)` without lowering the domain target to a string. `Effect::PersistSession { snapshot }` must not add any API request; it must call the `SessionStore` port, spawn the save future through `TaskRegistry`, emit bounded completion, and join like read tasks.

Add this forbidden scan to Task 12 review notes so Task 7 and Task 12 remain one compilable API:

```bash
python3 - <<'PY2'
from pathlib import Path
text = Path('src/effects/runner.rs').read_text()
assert text.count('pub struct ' + 'EffectRunner') == 1
PY2
! rg 'connect\(&self, scope: String\)|Option<Pin<Box<dyn[[:space:]]+Future|scopes: Vec<String>|format!\("rows:|format!\("schema:' src/effects/ports.rs src/effects/runner.rs
```

In `src/app.rs`, after reducer integration, process emitted effects by sending them to the runner boundary rather than executing network work in key handlers:

```rust
let transition = reduce(&mut self.state, event);
for effect in transition.effects {
    self.effect_runner.dispatch(effect);
}
while let Ok(event) = self.effect_events.try_recv() {
    self.handle_app_event(event);
}
```

Make `App` own `effect_runner: crate::effects::runner::AppEffectRunner` plus `effect_events: mpsc::Receiver<crate::app::event::AppEvent>`. In the existing `App::new(config, client)` constructor, clone the already-owned `SpacetimeClient` for `SpacetimeApiTransport::new(client.clone())`, create `bounded_event_channel`, pass its sender to `AppEffectRunner::new(SpacetimeApiTransport::new(client.clone()), Phase2SubscriptionTransport, CrosstermTerminalOps::new(), TaskRegistry::new(), Arc::new(LocalSessionStore), effect_sender)`, and keep the bounded receiver in `self.effect_events`. Task 12 must use the Task 7 `ApiTransport::execute_read(ReadOperation, RequestContext)` port contract directly; do not add another string `scopes` recorder or formatter-based dispatch path. Dropping the returned `TaskId` is safe because `dispatch` has already registered the owned read or persist task in `TaskRegistry`; the plan tests this future-drop risk explicitly.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test effects::runner::tests::runner_dispatches_load_active_resource_through_typed_api_transport_port && cargo test`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/effects/mod.rs src/effects/ports.rs src/effects/runner.rs src/app.rs src/app/reducer.rs
git commit -m "feat: run reducer effects through bounded runner"
```

### Task 13: Documentation phase truth for README, in-app help, command descriptions, and CHANGELOG

**Files:**
- Modify: `README.md`
- Modify: `src/ui/components/help.rs`
- Modify: `src/app/command.rs`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Write the failing tests**

Append to `src/app/command.rs` tests:

```rust
    #[test]
    fn phase2_command_descriptions_do_not_claim_new_layout() {
        let registry = CommandRegistry::default();
        let forbidden = ["Explorer", "Inspector drawer", "DATA OBSERVE OPERATE layout is live"];

        for line in registry.help_lines() {
            for phrase in forbidden {
                assert!(!line.description.contains(phrase), "{phrase} appeared in {:?}", line);
            }
        }
    }
```

Add this shell check to the task execution notes:

```bash
python3 - <<'PY'
from pathlib import Path
readme = Path('README.md').read_text()
changelog = Path('CHANGELOG.md').read_text()
assert 'Phase 2 foundation' in readme
assert 'typed command registry' in readme
assert 'layout is unchanged in Phase 2' in readme
assert 'Phase 2 foundation' in changelog
assert 'restore_session defaults to true' in changelog
PY
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test app::command::tests::phase2_command_descriptions_do_not_claim_new_layout && python3 - <<'PY'
from pathlib import Path
readme = Path('README.md').read_text()
changelog = Path('CHANGELOG.md').read_text()
assert 'Phase 2 foundation' in readme
assert 'typed command registry' in readme
assert 'layout is unchanged in Phase 2' in readme
assert 'Phase 2 foundation' in changelog
assert 'restore_session defaults to true' in changelog
PY`

Expected: Rust test PASS or FAIL depending on current descriptions. Python check FAIL if `CHANGELOG.md` is absent or the Phase 2 truth text is missing.

- [ ] **Step 3: Write minimal implementation**

Add this section to `README.md` near the feature overview:

```markdown
### Phase 2 foundation

The Phase 2 foundation adds a typed command registry, shared command availability policy, normalized reducer/effect boundaries, bounded AppEvent delivery, source-preserving configuration resolution, and central ownership for navigation, resource, workbench, activity, and safety state. The visible layout is unchanged in Phase 2. DATA, OBSERVE, OPERATE workbench surfaces are prepared in state and command metadata, but the full contextual workbench layout ships in a later phase.
```

Modify existing `CHANGELOG.md` by adding this entry at the top:

```markdown
# Changelog

## Unreleased

### Phase 2 foundation

- Added the typed command registry used by keyboard shortcuts, palette entries, contextual help, and future mouse actions.
- Added reducer and effect boundaries with deterministic runtime ports for tests.
- Centralized navigation, resource, workbench, activity, and safety state under one `AppState` owner with no second mutable state owner.
- Normalized configuration provenance so explicit CLI values win over environment and lower sources, including `--no-tls` and custom theme strings.
- `restore_session` defaults to true for missing and partial user configuration.
- The layout is unchanged in Phase 2. Full contextual workbench panes remain a later phase.
```

If `CHANGELOG.md` already has `# Changelog` and `## Unreleased`, insert only the `### Phase 2 foundation` block under `## Unreleased`.

In `src/ui/components/help.rs`, ensure Phase 2 help text is registry-driven and does not claim that the new layout is already visible. Use this explicit note string in the help footer or equivalent existing help text location:

```rust
pub const PHASE2_HELP_NOTE: &str = "Phase 2: shortcuts, palette, and help share one command registry. The layout is unchanged until the workbench UX phase.";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test app::command::tests::phase2_command_descriptions_do_not_claim_new_layout && python3 - <<'PY'
from pathlib import Path
readme = Path('README.md').read_text()
changelog = Path('CHANGELOG.md').read_text()
assert 'Phase 2 foundation' in readme
assert 'typed command registry' in readme
assert 'layout is unchanged in Phase 2' in readme
assert 'Phase 2 foundation' in changelog
assert 'restore_session defaults to true' in changelog
PY`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add README.md CHANGELOG.md src/ui/components/help.rs src/app/command.rs
git commit -m "docs: document phase2 foundation truth"
```

### Task 14: Final Phase 2 gates

**Files:**
- Modify only files already changed in Tasks 1 through 13 if a gate fails: `src/app.rs`, `src/app/command.rs`, `src/app/event.rs`, `src/app/reducer.rs`, `src/app/navigation.rs`, `src/app/policy.rs`, `src/effects/runner.rs`, `src/config.rs`, `src/user_config.rs`, `src/state/*.rs`, `src/ui/components/help.rs`, `src/ui/components/palette.rs`, `README.md`, `CHANGELOG.md`

- [ ] **Step 1: Run format gate**

Run: `cargo fmt --all -- --check`

This is the exact CI-parity format gate. The following lint, test, and build gates use `--locked` for dependency parity with CI.

Expected: PASS. If it fails, run `cargo fmt`, inspect the diff, and commit formatting only with the final gate commit.

- [ ] **Step 2: Run warning-denied Clippy gate**

Run: `cargo clippy --all-targets --all-features --locked -- -D warnings`

Expected: PASS. If it fails, fix only the reported Phase 2 issues. Do not suppress lints unless the lint is demonstrably wrong and the suppression is local with a reason.

- [ ] **Step 3: Run full tests**

Run: `cargo test --all-features --locked`

Expected: PASS. Required focused coverage includes:

```text
app::command::tests::ctrl_p_keyboard_palette_and_help_resolve_same_command
app::command::tests::disabled_commands_report_stable_reason
app::command::tests::palette_entries_are_generated_from_registered_commands
app::command::tests::help_marks_disabled_commands_with_registry_reason
app::navigation::tests::select_resource_updates_navigation_and_emits_load_effect_once
app::navigation::tests::navigate_up_from_resource_clears_resource_and_emits_schema_reload
app::reducer::tests::reducer_is_pure_and_emits_effect_for_refresh_command
state::app_state::phase2_state_tests::app_state_has_one_owner_for_phase2_state_domains
state::resources::load_state_tests::refresh_keeps_existing_data_visible
state::resources::load_state_tests::stale_completion_does_not_replace_newer_generation
effects::runner::tests::bounded_event_channel_reports_capacity_and_refuses_overflow
effects::runner::tests::deterministic_ports_can_drive_runner_without_real_time_or_randomness
config::phase2_config_tests::explicit_cli_default_looking_values_win_over_environment_and_detected_config
config::phase2_config_tests::clap_value_source_maps_explicit_no_tls_to_command_line
config::phase2_config_tests::cli_theme_accepts_custom_theme_string
user_config::phase2_user_config_tests::default_user_config_restores_session
user_config::phase2_user_config_tests::legacy_session_tabs_map_deterministically_to_navigation
```

- [ ] **Step 4: Run release build gate**

Run: `cargo build --release --locked`

Expected: PASS.

- [ ] **Step 5: Run phase truth gate**

Run:

```bash
python3 - <<'PY'
from pathlib import Path
required = {
    'README.md': [
        'Phase 2 foundation',
        'typed command registry',
        'layout is unchanged in Phase 2',
    ],
    'CHANGELOG.md': [
        'Phase 2 foundation',
        'restore_session defaults to true',
        'layout is unchanged in Phase 2',
    ],
    'src/ui/components/help.rs': [
        'Phase 2: shortcuts, palette, and help share one command registry',
    ],
}
for file, phrases in required.items():
    text = Path(file).read_text()
    for phrase in phrases:
        assert phrase in text, f'{phrase!r} missing from {file}'
PY
```

Expected: PASS.

- [ ] **Step 6: Commit final gate fixes**

```bash
git status --short
git add src/app.rs src/app/command.rs src/app/event.rs src/app/reducer.rs src/app/navigation.rs src/app/policy.rs src/effects/mod.rs src/effects/ports.rs src/effects/runner.rs src/config.rs src/user_config.rs src/state/app_state.rs src/state/navigation.rs src/state/resources.rs src/state/workbench.rs src/state/activity.rs src/state/safety.rs src/state/mod.rs src/ui/components/help.rs src/ui/components/palette.rs src/ui/components/input.rs src/ui/components/table_grid.rs README.md CHANGELOG.md
git commit -m "chore: pass phase2 foundation gates"
```

Expected: a final commit exists only if formatting, lint, test, build, or phase-truth fixes were needed after Task 13.

## Self-review checklist

- Task 1 includes a Current AppState field migration table and requires existing AppState fields preserved/moved or explicitly preserved as legacy fields until migrated.
- Final task order after dependency correction: Task 1 state foundation; Task 2 command registry; Task 3 palette/help registry integration; Task 4 normalized event/reducer transition; Task 5 semantic navigation; Task 6 scoped LoadState; Task 7 deterministic ports/runner; Task 8 widget/input state migration; Task 9 config provenance; Task 10 session mapping; Task 11 extraction cleanup; Task 12 runner integration; Task 13 docs; Task 14 gates.
- Shared `invoke(id)` action bridge is introduced in Task 4 after `app/event.rs` exists; Task 2/3 compile with CommandId/registry only.
- Final CommandSpec/Registry API: `CommandSpec { id, label, description, bindings, modes, focus: &'static [FocusContext], availability: AvailabilityRule }`; `CommandRegistry::default()` only, with `by_id(CommandId)`, `iter()`, `resolve_key(KeyChord)`, `resolve_palette(&str)`, `search_palette(&self, query: &str) -> Vec<CommandId>`, `help_lines()`, and `availability(CommandId, &CommandContext)`.


- Phase 2 snippets must not define `RequestContext`, `RequestId`, or `RequestScope`; they reuse `crate::effects::request::{RequestId, RequestScope, RequestContext}` from Phase 1.
- No production API is named for tests only; tests use local fixtures, `Default`, or real constructors.
- Reducer returns `Transition { effects }`, uses `Action::Invoke(CommandId)`, evaluates command availability through `app/policy`, and never defines a no-op effect variant.


- Phase 2 command registry at `src/app/command.rs` is covered by Tasks 2, 3, 11, and 13, including stable names `OpenCommandPalette`, `OpenContextualHelp`, `CloseSurfaceOrNavigateUp`, `FocusNextPane`, `FocusPreviousPane`, `ToggleLive`, and `ConfirmWritePlan`.
- Keyboard, palette, help, and later mouse input resolve the same commands in Tasks 2 and 3 through the registry and policy, with no competing hard-coded availability.
- `Action`, `AppEvent`, `Effect`, and pure reducer are covered by Tasks 4 and 12.
- Semantic navigation effect emission is covered by Task 5.
- State split and one `AppState` owner are covered by Task 1; event/effect adapters cannot own a second navigation or resource state copy.
- `LoadState<T>` transitions and stale-result rejection are covered by Task 6.
- Deterministic ports, effect runner, bounded AppEvent channel, Phase 1 task registry reuse, and task ownership boundary are covered by Tasks 7 and 12. The plan explicitly rejects any unbounded AppEvent channel.
- Widget/input state migration is covered by Task 8.
- `ConfigSource`, `Resolved<T>`, Clap `ValueSource`, CLI over env over lower sources, `--no-tls`, and custom theme strings are covered by Task 9.
- `UserConfig` default `restore_session=true` and deterministic session mapping are covered by Task 10.
- Incremental extraction from module-root `src/app.rs` into child files `src/app/{command,event,reducer,navigation,policy}.rs` and `src/effects/runner.rs` without layout change is covered by Tasks 2, 4, 5, 7, 11, and 12. An app `mod.rs` file is never created while `src/app.rs` exists.
- README, help, command descriptions, CHANGELOG truth, and final gates are covered by Tasks 13 and 14.


## Plan validation checklist for workers

Before implementation starts and before final handoff, verify this plan still satisfies these constraints:

```bash
python3 - <<'PY'
from pathlib import Path
text = Path('docs/superpowers/plans/2026-07-26-contextual-workbench-phase-2-foundation.md').read_text()
required = [
    'src/app/command.rs',
    'OpenCommandPalette',
    'OpenContextualHelp',
    'CloseSurfaceOrNavigateUp',
    'FocusNextPane',
    'FocusPreviousPane',
    'ToggleLive',
    'ConfirmWritePlan',
    'bounded AppEvent channel',
    'mpsc::channel(capacity)',
    'RequestContext',
    'WritePlan',
    'mutation uncertainty',
    'do not enable all-table Live behavior',
    'src/effects/ports.rs',
    'crate::effects::task_registry::TaskRegistry',
    'crate::terminal::TerminalOps',
    'RequestId::from_u64',
    'view: "browse".into()',
    'Action::ConfirmWritePlan { plan_id',
    'toggle_live_remains_disabled_until_phase4_capability_probe',
    'focus_context_from_pane',
    'pub enum ReadOperation',
    'fn execute_read(&self, operation: ReadOperation, context: RequestContext)',
    'pub type AppEffectRunner = EffectRunner<SpacetimeApiTransport, Phase2SubscriptionTransport, CrosstermTerminalOps>',
    'ReadFailure::Transport(String)',
    'ScopedReadFailed { context, failure, retry: Option<ReadOperation> }',
    'cargo clippy --all-targets --all-features --locked' + ' -- -D warnings',
    'cargo test --all-features' + ' --locked',
    'cargo build --release' + ' --locked',
]
assert '_for' + '_test' not in text
for phrase in required:
    assert phrase in text, phrase
assert text.count('```') % 2 == 0
assert text.count('### Task ') == 15
assert text.count('pub struct ' + 'EffectRunner') == 1
for forbidden in ['RequestScope::' + 'Catalog', 'connect(&self, scope: ' + 'String)']:
    assert forbidden not in text, forbidden
PY
git diff --check
```

```text
All focused Cargo test invocations in this plan use zero or one positional test filter.
```
