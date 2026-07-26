# Contextual Workbench Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deliver the approved Contextual Workbench design as five safety-first, independently releasable phases without regressing existing SpacetimeDB TUI capabilities.

**Architecture:** Keep Ratatui and the existing transport adapters, but move behavior toward a central `AppState`, typed commands, a pure reducer, typed effects, scoped async contexts, and owned runtime tasks. Phase 1 narrows unsafe behavior before structural extraction, Phases 2 and 3 establish the workbench interaction model, Phase 4 enables bounded active-resource Live behavior, and Phase 5 makes product and release claims mechanically verifiable.

**Tech Stack:** Rust 2021, Tokio, Ratatui 0.29, Crossterm 0.28, Reqwest 0.12, Tokio Tungstenite 0.26, Serde, Clap 4, GitHub Actions, Bash, PowerShell.

---

## 1. Execution workspace and baseline

- Repository source: `/home/raziel/projects/spacetimedb-tui`
- Isolated worktree: `/home/raziel/.config/superpowers/worktrees/spacetimedb-tui/contextual-workbench`
- Feature branch: `feature/contextual-workbench`
- Starting commit: `996213d`
- Starting verification: `cargo build` passed and all 127 tests passed.
- User-owned untracked paths in the source checkout, `.claude/` and `exports/`, are outside this worktree and must never be staged or modified.
- Visual brainstorming artifacts remain outside the repository. Product screenshots added in Phase 3 are release documentation, not copies of the brainstorming companion.

Before every implementation session:

```bash
cd /home/raziel/.config/superpowers/worktrees/spacetimedb-tui/contextual-workbench
git status --short
git branch --show-current
```

Expected:

```text
feature/contextual-workbench
```

A non-empty status must be explained by the currently executing task before another worker starts.

## 2. Plan set and strict order

Execute every checkbox in each linked plan in order:

1. [Phase 1: Safety and Crash Resistance](2026-07-26-contextual-workbench-phase-1-safety.md)
2. [Phase 2: State and Interaction Foundation](2026-07-26-contextual-workbench-phase-2-foundation.md)
3. [Phase 3: Contextual Workbench UX](2026-07-26-contextual-workbench-phase-3-workbench.md)
4. [Phase 4: Live Data, Recovery, and Performance](2026-07-26-contextual-workbench-phase-4-live-performance.md)
5. [Phase 5: Product Truth and Release Hardening](2026-07-26-contextual-workbench-phase-5-release.md)

Do not start a later phase until the previous phase gate passes and its documentation describes shipped behavior accurately.

## 3. Locked cross-phase type contracts

Later phases may add variants or fields through reviewed migrations, but must not create competing owners or synonyms for these concepts.

### 3.1 Scoped requests

Location: `src/effects/request.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestScope {
    DatabaseCatalog,
    Schema { database: String },
    TableRows { database: String, table: String, view: String },
    SqlWorkspace { database: String, workspace: String },
    Logs { database: String },
    Metrics { database: String },
    LiveClients { database: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestContext {
    pub id: RequestId,
    pub scope: RequestScope,
    pub generation: u64,
}

impl RequestContext {
    pub fn new(id: RequestId, scope: RequestScope, generation: u64) -> Self {
        Self { id, scope, generation }
    }

    pub fn scope(&self) -> &RequestScope { &self.scope }

    pub fn accepts(&self, delivered: &RequestContext) -> bool {
        self.id == delivered.id
            && self.scope == delivered.scope
            && self.generation == delivered.generation
    }
}
```

A result is applicable only when request ID, scope, and generation match the latest request for that scope. `RequestScope` implements `Display` using deterministic non-secret labels. `RequestContext` keeps the three fields public for the Phase 1 and Phase 2 tests and also exposes `scope()` for borrowing without moving the scope.

### 3.2 Guided writes and mutation outcomes

Location: `src/state/safety.rs`

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WritePlanId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QualifiedTable {
    pub database: String,
    pub schema: Option<String>,
    pub table: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqlValue {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64Bits(u64),
    Text(String),
    Bytes(Vec<u8>),
    Unrepresentable(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryKeyPart {
    pub column_id: u16,
    pub value: SqlValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletePrimaryKey {
    parts: Vec<PrimaryKeyPart>,
}

impl CompletePrimaryKey {
    pub fn parts(&self) -> &[PrimaryKeyPart] { &self.parts }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColumnChange {
    pub column_id: u32,
    pub old_value: Option<SqlValue>,
    pub new_value: SqlValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GuidedMutation {
    Update { changes: Vec<ColumnChange> },
    Delete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WritePlan {
    pub id: WritePlanId,
    pub table: QualifiedTable,
    pub original_primary_key: CompletePrimaryKey,
    pub schema_generation: u64,
    pub row_generation: u64,
    pub server_context_generation: u64,
    pub database_generation: u64,
    pub mutation: GuidedMutation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    DefinitelyNotSent { reason: String },
    SentAndConfirmed { affected_rows: Option<u64> },
    Conflict { reason: String },
    Unknown { reason: String },
    CriticalSafetyError { reason: String },
}
```

`CompletePrimaryKey` is constructed only by validation, stores typed raw values, and keeps `parts` private. Confirmation acceptance revalidates schema generation, row generation, server context generation, database generation, complete original key tuple, changed typed values, and row locks immediately before transport ownership. `Unknown`, `Conflict`, and `CriticalSafetyError` disable guided writes for the affected scope until refresh; `Unknown` is never automatically retried.

### 3.3 Commands, reducer, and effects

Locations: `src/app/command.rs`, `src/app/event.rs`, `src/app/reducer.rs`. `src/app.rs` remains the module root and application shell during migration; the path `src/app/mod.rs` is excluded from this implementation.

The following is the locked public shape and signature inventory. It is a contract summary, not a paste-ready implementation block:

```text
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

#[derive(Debug, Default, Clone, Copy)]
pub struct CommandRegistry;

impl CommandRegistry {
    pub fn by_id(&self, id: CommandId) -> Option<&'static CommandSpec>;
    pub fn iter(&self) -> impl Iterator<Item = &'static CommandSpec>;
    pub fn resolve_key(&self, key: KeyChord) -> Option<CommandId>;
    pub fn resolve_palette(&self, query: &str) -> Option<CommandId>;
    pub fn search_palette(&self, query: &str) -> Vec<CommandId>;
    pub fn palette_score(&self, id: CommandId, query: &str) -> usize;
    pub fn help_lines(&self) -> Vec<HelpLine>;
    pub fn availability(&self, id: CommandId, context: &CommandContext) -> Availability;
    pub fn palette_entries(&self, context: &CommandContext) -> Vec<PaletteEntry>;
    pub fn contextual_help(&self, context: &CommandContext) -> Vec<ContextualHelpLine>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Invoke(CommandId),
    SelectDatabase { database: String },
    SelectResource { database: String, resource: String },
    NavigateUp,
    ConfirmWritePlan { plan_id: WritePlanId },
    RestoreSession,
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    Action(Action),
    ScopedReadCompleted { context: RequestContext, result: ReadResult },
    ScopedReadFailed { context: RequestContext, failure: ReadFailure, retry: Option<ReadOperation> },
    Tick,
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
pub enum ReadFailure {
    Transport(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    LoadCatalog { context: RequestContext },
    LoadSchema { database: String, context: RequestContext },
    LoadTableRows { target: TableTarget, context: RequestContext },
    PersistSession { snapshot: crate::user_config::SessionState },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub effects: Vec<Effect>,
}

pub fn reduce(state: &mut AppState, event: AppEvent) -> Transition;
```

`CommandId`, `Action`, `AppEvent`, `Effect`, `ReadFailure`, and `Transition` are extensible through reviewed additions. `ReadOperation` is the typed identity carried from an effect through transport, failure, retry planning, and retry execution. It must never be reconstructed from `RequestScope::Display` or any formatted string. Phase 2 retains `retry: Some(exact_operation)` only for retryable transport failures and uses `retry: None` for deterministic local validation failures such as rejected SQL identifiers. Phase 4 extends the inherited `ReadFailure` enum and consumes the inherited retry payload without redefining `ScopedReadFailed`. Do not keep earlier draft aliases for refresh, live toggling, action dispatch, read dispatch, or reducer signatures once the Phase 2 reducer boundary lands. Keyboard, mouse, and palette execution all produce the identical `Action::Invoke(CommandId)` path. Palette and help content resolve through the same `CommandRegistry`; Phase 3 migrates `CommandPalette::filter(&self)` to call `CommandRegistry::default().search_palette(&self.query.value)` and return command IDs. Phase 4 extends `AppEvent` and `Effect` without replacing these base contracts.

### 3.4 State ownership

Location: `src/state/app_state.rs` with focused submodules under `src/state/`.

```rust
pub struct AppState {
    pub navigation: NavigationState,
    pub resources: ResourceState,
    pub workbench: WorkbenchState,
    pub activity: ActivityState,
    pub safety: SafetyState,
    pub requests: RequestTracker,
}
```

`NavigationState`, `ResourceState`, `WorkbenchState`, `ActivityState`, `SafetyState`, and `RequestTracker` are the runtime owners. `user_config::SessionState` is an I/O representation only. Widget state migrates into `WorkbenchState`. Render functions receive state and cannot start effects or change domain selection. No later phase may introduce a second mutable owner for navigation, resource rows, live subscriptions, request generations, retry state, or mutation outcomes.

### 3.5 Load lifecycle

Location: `src/state/resources.rs`

```rust
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    Idle,
    Loading { request: RequestContext },
    Ready { data: T, refreshed_at: std::time::Instant },
    Refreshing { data: T, request: RequestContext },
    Empty { refreshed_at: std::time::Instant },
    Stale { data: T, reason: StaleReason },
    Error { previous: Option<T>, error: AppError },
}
```

`Retrying` and `Offline` are derived presentations using `LoadState`, retry state, and connection state. They are not duplicate data stores.

### 3.6 Subscription context and task ownership

Locations: typed generations and ID conversion in `src/effects/ports.rs`, wire-scoped identity in `src/api/ws_decode.rs`, and ownership/reconciliation in `src/effects/subscription.rs`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionGeneration(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubscriptionKey {
    pub database: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionSpec {
    pub key: SubscriptionKey,
    pub query_strings: Vec<String>,
    pub expected_bound: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionContext {
    pub key: SubscriptionKey,
    pub task_generation: TaskGeneration,
    pub connection_generation: ConnectionGeneration,
    pub request_id: u32,
}

pub struct OwnedSubscriptionTask {
    pub spec: SubscriptionSpec,
    pub task_generation: TaskGeneration,
    pub context: Option<SubscriptionContext>,
    pub phase: LivePhase,
    pub handle: Option<crate::api::ws::WsHandle>,
}

pub struct SubscriptionController {
    pub desired: Option<SubscriptionSpec>,
    pub task: Option<OwnedSubscriptionTask>,
}
```

Phase 4 owns one `SubscriptionController` with exactly `desired` and `task`, so there is one desired subscription specification and at most one actual `OwnedSubscriptionTask`. Desired `None` closes and joins the actual task. Desired replacement creates a close/join barrier before a new task starts. Each connection attempt uses the shared Phase 2 `IdSource` to allocate a fresh checked `u32` wire request ID plus fresh typed task/connection generations; reconnect uses `BackoffPolicy` from Phase 2 and reports completion or panic through Phase 1 `TaskOutcome`. Do not create duplicate generation counters, duplicate backoff policy owners, or a second task-outcome enum. `TransactionUpdate` receives the owning local context because the wire event has no subscription request ID.

### 3.7 Runtime ports

Locations: `src/effects/ports.rs`, `src/effects/task_registry.rs`, `src/terminal.rs`.

The stable port names are:

```text
ApiTransport
SubscriptionTransport
SessionStore
Clock
Sleeper
RandomSource
BackoffPolicy
IdSource
TaskRegistry
TerminalOps
```

The locked Phase 2 execution boundary is:

```text
ApiTransport::execute_read(ReadOperation, RequestContext)
    -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>

SubscriptionTransport::connect(Self::Request)
    -> Pin<Box<dyn Future<Output = AppEvent> + Send + '_>>

SessionStore::save(user_config::SessionState)
    -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>>

EffectRunner<A, S, T> {
    api: A,
    subscriptions: S,
    terminal: T,
    tasks: TaskRegistry,
    sessions: Arc<dyn SessionStore>,
    event_sender: tokio::sync::mpsc::Sender<AppEvent>,
}

SpacetimeApiTransport { client: SpacetimeClient }
Phase2SubscriptionTransport
LocalSessionStore
AppEffectRunner = EffectRunner<SpacetimeApiTransport, Phase2SubscriptionTransport, CrosstermTerminalOps>
```

`SubscriptionTransport::Request` is an associated typed request. Phase 2 binds it to `Infallible` while row Live remains unavailable; Phase 4 replaces that associated request with the owned typed subscription request shape rather than a string. `SpacetimeApiTransport` clones the existing `SpacetimeClient`, validates table identifiers before SQL formatting, and emits the exact scoped completion or failure event. `LocalSessionStore` is the production `SessionStore` adapter. `App` owns one concrete `AppEffectRunner` plus the bounded `mpsc::Receiver<AppEvent>` returned at startup, dispatches every `Transition.effects` item, and drains completion events back through the same reducer path.

There is one `EffectRunner<A, S, T>` throughout Phases 2 and 4. It owns the concrete Phase 1 `TaskRegistry`, the typed `SessionStore`, and a bounded Tokio MPSC sender. `dispatch` spawns every read or persistence future through `TaskRegistry` and sends its `AppEvent` completion into that bounded channel. It never returns a bare future that a caller can drop unpolled, and it never sends `PersistSession` through `ApiTransport`. Tests use deterministic fakes. Safety-relevant tasks are cancellable, joined, and panic-reporting through the shared `TaskOutcome` path.

## 4. Target file structure

The phased end state is:

```text
src/
├── app.rs                 # module root and application shell; do not replace with src/app/mod.rs
├── app/
│   ├── command.rs
│   ├── event.rs
│   ├── navigation.rs
│   ├── policy.rs
│   └── reducer.rs
├── effects/
│   ├── mod.rs
│   ├── ports.rs
│   ├── request.rs
│   ├── runner.rs
│   ├── subscription.rs
│   ├── task_registry.rs
│   └── write_ops.rs
├── resource/
│   ├── budget.rs
│   └── estimate.rs
├── state/
│   ├── app_state.rs
│   ├── activity.rs
│   ├── navigation.rs
│   ├── palette.rs
│   ├── resources.rs
│   ├── row_cache.rs
│   ├── safety.rs
│   └── workbench.rs
├── terminal.rs
├── ui/
│   ├── activity.rs
│   ├── explorer.rs
│   ├── hit_test.rs
│   ├── inspector.rs
│   ├── states.rs
│   ├── styles.rs
│   ├── test_support.rs
│   ├── text.rs
│   ├── workbench.rs
│   ├── components/
│   │   ├── grid_index.rs
│   │   ├── help.rs
│   │   └── palette.rs
│   └── existing reusable views and components
└── api/
    ├── client.rs
    ├── http_body.rs
    ├── query_decode.rs
    ├── types.rs
    ├── ws.rs
    └── ws_decode.rs

scripts/
├── install.sh
├── install.ps1
└── release/
    ├── check-msrv.sh
    ├── make-archives.sh
    ├── product-claim-check.sh
    ├── validate-version.sh
    ├── verify-checksums.sh
    └── write-checksum-manifest.sh

images/
├── contextual-workbench-wide.svg
├── contextual-workbench-medium.svg
└── contextual-workbench-compact.svg
```

`src/app.rs` remains the module root and application shell. The path `src/app/mod.rs` is excluded and must never appear as a target file.

The target tree includes `src/resource/{budget,estimate}.rs`, `src/api/{http_body,query_decode,ws_decode}.rs`, `src/state/row_cache.rs`, and `src/ui/components/grid_index.rs`. Do not create stale paths from earlier drafts unless a phase plan explicitly reintroduces them. The final target tree excludes `src/app/mod.rs`, `src/effects/retry.rs`, `src/state/cache.rs`, `src/state/live.rs`, and `src/api/budget.rs`.

## 5. Phase gates

### Phase 1 gate

- No name-based, auto-increment, or first-column primary-key fallback remains.
- Update and delete require every valid declared primary-key component.
- Confirmation-time generation or key mismatch is local `DefinitelyNotSent`.
- Transport-owned non-authoritative mutation failure is `Unknown` and has no automatic retry path.
- Spreadsheet Save aggregates changes for exactly one row.
- Automatic all-table subscription is removed and Live truthfully explains temporary unavailability.
- Late scoped results cannot replace newer scope data.
- Terminal, Unicode, narrow layout, and upward navigation regressions have tests.
- README, existing in-app help, existing palette wording, and `CHANGELOG.md` match the narrowed Phase 1 behavior.

### Phase 2 gate

- `CommandRegistry` is the source for keyboard, palette, generated help, keybinding help, and availability; `reduce(&mut AppState, AppEvent) -> Transition` is the reducer boundary.
- Navigation and async completion pass through a reducer and typed effects.
- `AppState` owns Navigation, Resources, Workbench, Activity, Safety, and RequestTracker with no second mutable owner.
- All deterministic runtime ports have production adapters and fakes.
- Config source provenance and session defaults pass the full source matrix.
- Existing layout and features remain available.

### Phase 3 gate

- DATA, OBSERVE, and OPERATE preserve all migrated capabilities.
- Wide, Medium, Compact, and TooSmall layout contracts pass deterministic render tests.
- Explorer, Workspace, Inspector, Activity, and Breadcrumb remain discoverable at applicable sizes.
- Every supported mouse action resolves to a registered command with keyboard or palette parity.
- Lifecycle views and recovery actions are distinct and non-color-only.
- Visible layout documentation and `images/contextual-workbench-wide.svg`, `images/contextual-workbench-medium.svg`, and `images/contextual-workbench-compact.svg` match the implementation and README references.

### Phase 4 gate

- One desired subscription and at most one actual `OwnedSubscriptionTask` exist for only the desired active resource.
- Active requires a matching initial acknowledgement for the current socket context.
- Scope changes close and join the old task. Reconnect uses typed `TaskGeneration`, typed `ConnectionGeneration`, Phase 2 `IdSource`, Phase 2 `BackoffPolicy`, Phase 1 `TaskOutcome`, a fresh request ID, and no duplicate query sets.
- Frame, message, HTTP body, decoded batch, single row, queue, cache byte, and cache row limits reject before unbounded retention.
- Cache stays within 64 MiB or 10,000 rows by deterministic LRU accounting.
- Rendering consumes precomputed `GridViewIndex` and `ColumnMetrics` and scales with the viewport.
- Live scope, state, budget exhaustion, and retry behavior are visible and documented.

### Phase 5 gate

- Continuous log-tail positive claims are removed. The product truth is on-demand log refresh; Phase 5 does not connect `spawn_log_follow`.
- `rust-version`, the lockfile policy, README, and an MSRV CI job agree after checking candidates from Rust 1.78.0 through the current 1.93 range with explicit failure if none pass.
- Tag, Cargo version, documentation examples, archive names, and binary version agree.
- Release archives publish one combined `SHA256SUMS.txt` manifest, and both installers verify exactly one matching checksum entry before extraction.
- Packaged binaries and installer fixture paths fail the build on smoke-test failure.
- Product-claim validation covers README, `src/app/command.rs`, generated help, palette rendering/indexing, keybinding help, changelog, Phase 3 SVG screenshots, and legacy screenshots.

## 6. Requirement-to-plan coverage

| Approved requirement | Owning plan | Required observable check |
|---|---|---|
| Complete declared primary key and stale confirmation rejection | Phase 1 | Constructor and confirmation tests reject malformed, partial, null, unrepresentable, or stale plans |
| One-row spreadsheet save | Phase 1 | Multiple cells in one row produce one plan and editing another dirty row yields Save, Discard, Stay |
| Mutation uncertainty | Phase 1, Phase 4 | Pre-dispatch rejection is `DefinitelyNotSent`; post-ownership uncertainty is `Unknown`; no retry is emitted |
| Scoped async state | Phase 1, Phase 2 | Out-of-order, cancelled, and old-generation results do not change current state |
| Terminal and Unicode safety | Phase 1 | Fake terminal setup matrix and dimensions/UTF-8 property tests never panic |
| Typed commands and one reducer | Phase 2 | Registry parity and reducer transition tests cover every input source |
| Config provenance and session semantics | Phase 2 | Pairwise source matrix includes explicit default-looking values and custom themes |
| Contextual Workbench layout | Phase 3 | TestBackend snapshots cover specified dimensions, focus states, themes, and Unicode |
| Keyboard-first mouse parity | Phase 2, Phase 3 | Every mouse hit target maps to a command reachable by keyboard or palette |
| Distinct lifecycle feedback | Phase 2, Phase 3 | Loading, refreshing, ready, empty, stale, offline, retrying, and error render assertions |
| Active-only bounded Live | Phase 1, Phase 4 | No all-table call path; one desired task; all transport/decode/queue/cache budgets enforced |
| Duplicate-free recovery | Phase 4 | Deterministic reconnect test proves one query set and fresh local context per connection attempt |
| Bounded render work | Phase 4 | Instrumentation proves render does not rebuild sort, filter, or width data |
| Product and release truth | Every phase, finalized Phase 5 | Documentation gate plus on-demand log truth, MSRV through 1.93 range, version/archive/combined-checksum/installer/package checks |

## 7. Coordinated execution tasks

### Task 1: Lock the approved planning baseline

**Files:**
- Modify: `docs/superpowers/specs/2026-07-26-contextual-workbench-ux-design.md:5`
- Create: `docs/superpowers/plans/2026-07-26-contextual-workbench-implementation.md`
- Create: the five phase plans linked in Section 2

- [ ] **Step 1: Verify the specification records approval**

Run:

```bash
grep -F '**Status:** Approved for implementation' docs/superpowers/specs/2026-07-26-contextual-workbench-ux-design.md
```

Expected: exactly one matching line.

- [ ] **Step 2: Verify every plan has the mandatory worker header and checkboxes**

Run:

```bash
for plan in docs/superpowers/plans/2026-07-26-contextual-workbench-*.md; do
  grep -F '> **For agentic workers:** REQUIRED SUB-SKILL:' "$plan"
  grep -F -- '- [ ]' "$plan" >/dev/null
done
```

Expected: every plan prints its worker header and exits zero.

- [ ] **Step 3: Commit only the approved status and plan documents**

```bash
git add docs/superpowers/specs/2026-07-26-contextual-workbench-ux-design.md docs/superpowers/plans
git diff --cached --check
git commit -m "docs: add contextual workbench implementation plans"
```

### Task 2: Execute Phase 1 and review its safety invariants

**Plan:** `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-1-safety.md`

- [ ] **Step 1: For each Phase 1 task, dispatch exactly one implementation worker**
- [ ] **Step 2: Require the implementation worker to make the planned commit for that task**
- [ ] **Step 3: After the task commit, request one specification-compliance review**
- [ ] **Step 4: After compliance passes, request one code-quality review**
- [ ] **Step 5: Run the full Phase 1 gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
bash -n scripts/install.sh
```

Expected: all commands exit zero and Live no longer starts an all-table subscription.

### Task 3: Execute Phase 2 and review state ownership

**Plan:** `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-2-foundation.md`

- [ ] **Step 1: For each Phase 2 task, dispatch exactly one implementation worker in plan order while preserving Phase 1 safety APIs**
- [ ] **Step 2: Require the implementation worker to make the planned commit for that task**
- [ ] **Step 3: Request one specification-compliance review that rejects direct network calls from render code and second mutable owners**
- [ ] **Step 4: After compliance passes, request one code-quality review**
- [ ] **Step 5: Run the full Phase 2 gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
```

Expected: all commands exit zero and registry, reducer, ports, config matrix, and session tests pass.

### Task 4: Execute Phase 3 and review the workbench UX

**Plan:** `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-3-workbench.md`

- [ ] **Step 1: For each Phase 3 task, dispatch exactly one implementation worker, executing mode and layout tasks before workspace migration**
- [ ] **Step 2: Require the implementation worker to make the planned commit for that task**
- [ ] **Step 3: Request one specification-compliance review that verifies migration coverage and command-based mouse handling**
- [ ] **Step 4: After compliance passes, request one code-quality review**
- [ ] **Step 5: Run the full Phase 3 gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
```

Expected: all commands exit zero and representative TestBackend rendering passes for the required size, theme, focus, modal, and Unicode matrix.

### Task 5: Execute Phase 4 and review resource bounds

**Plan:** `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-4-live-performance.md`

- [ ] **Step 1: For each Phase 4 task, dispatch exactly one implementation worker and land transport/decode budgets before row Live**
- [ ] **Step 2: Require the implementation worker to make the planned commit for that task**
- [ ] **Step 3: Request one specification-compliance review covering cache, queue, subscription reconciliation, and deterministic performance assertions**
- [ ] **Step 4: After compliance passes, request one code-quality review**
- [ ] **Step 5: Run the full Phase 4 gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
```

Expected: all commands exit zero and budget, LRU, reconnect, stale-event, retry, and render instrumentation tests pass.

### Task 6: Execute Phase 5 and run the release rehearsal

**Plan:** `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-5-release.md`

- [ ] **Step 1: For each Phase 5 task, dispatch exactly one implementation worker and resolve product claims before release automation**
- [ ] **Step 2: Require the implementation worker to make the planned commit for that task**
- [ ] **Step 3: Request one specification-compliance review covering MSRV, version, archive, combined checksum, package, installer, and product-claim truth**
- [ ] **Step 4: After compliance passes, request one code-quality review**
- [ ] **Step 5: Run the final local release gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
cargo package --locked
bash -n scripts/install.sh
pwsh -NoProfile -Command "[void][scriptblock]::Create((Get-Content -Raw scripts/install.ps1))"
version=$(awk -F ' = ' '$1 == "version" { gsub(/"/, "", $2); print $2; exit }' Cargo.toml)
tag="v${version}"
host=$(rustc -vV | sed -n 's/^host: //p')
rustup target add "$host"
cargo build --release --locked --target "$host"
bash scripts/release/make-archives.sh "$host" "$tag"
bash scripts/release/write-checksum-manifest.sh "$tag"
bash scripts/release/verify-checksums.sh "target/release-artifacts/spacetimedb-tui-${tag}-SHA256SUMS.txt"
bash scripts/release/check-msrv.sh
bash scripts/release/validate-version.sh
bash scripts/release/product-claim-check.sh
```

Also run the repository release validation scripts created by the Phase 5 plan. Expected: every command exits zero, generated checksum verification succeeds, and packaged binaries report the Cargo version.

### Task 7: Final independent review and branch completion

- [ ] **Step 1: Run the verification-before-completion skill and capture fresh output**
- [ ] **Step 2: Request a full-spec code review against all 22 specification sections**
- [ ] **Step 3: Fix every blocker, high, and medium finding with a failing test first**
- [ ] **Step 4: Re-run the final release gate after the last fix**
- [ ] **Step 5: Confirm the source checkout still contains untouched user paths**

```bash
git -C /home/raziel/projects/spacetimedb-tui status --short
```

Expected: only the pre-existing `.claude/` and `exports/` entries appear there.

- [ ] **Step 6: Use the finishing-a-development-branch skill to present merge, PR, or branch-retention options**

Do not merge, push, publish a release, or delete the worktree without explicit user authorization.
