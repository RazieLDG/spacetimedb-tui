# Contextual Workbench Phase 4 Live Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement bounded active-resource Live data, deterministic recovery, atomic cache admission, and precomputed grid performance without restoring unbounded all-table subscriptions.

**Architecture:** HTTP and WebSocket transports enforce encoded limits before decode. Custom `serde::de::DeserializeSeed` visitors keep each encoded row array as borrowed `&RawValue`, materialize one row at a time, estimate it without cloning, and reject it before push when a row or batch limit is exceeded. The Phase 2 effect runner owns the only subscription controller, the controller owns exactly one desired specification and at most one real `WsHandle`, and the reducer owns presentation state only.

**Tech Stack:** Rust 2021, Tokio, Reqwest streaming bodies, tokio-tungstenite 0.26 connect-time `WebSocketConfig`, Serde, `serde_json` with `raw_value`, Ratatui `TestBackend`, existing Phase 1 through Phase 3 state/effect/task contracts.

---

## Fixed contracts inherited from earlier phases and current source

Use these exact owners and shapes. Do not create aliases with competing behavior.

- Phase 1 `RequestContext { pub id: RequestId, pub scope: RequestScope, pub generation: u64 }` and `RequestContext::accepts` own stale read rejection.
- Phase 1 `TaskOutcome` is exactly `Completed`, `Cancelled`, and `Panicked { message: String }`.
- Phase 1 `MutationOutcome` includes `DefinitelyNotSent { reason }`, `SentAndConfirmed { affected_rows }`, `Conflict { reason }`, and `Unknown { reason }`. Phase 4 never schedules an automatic mutation retry.
- Phase 2 `IdSource` is exactly `fn next_id(&mut self) -> u64`.
- Phase 2 `BackoffPolicy` is exactly `fn delay(&self, attempt: u32, random: &mut dyn RandomSource) -> Duration`.
- Phase 2 `ActivityState` owns `connection: ConnectionState` and singular `retry: RetryState`.
- Phase 2 `ConnectionState::Connected` is a unit variant.
- Phase 2 `RetryState` has `attempt: u32` and `next_retry_label: Option<String>`.
- Phase 2 `LoadState<T>` remains the resource lifecycle authority.
- Phase 2 `ReadOperation` is exactly `Catalog`, `Schema { database: String }`, and `TableRows { target: TableTarget }`; Phase 4 extends this existing enum with methods and never defines a competing read descriptor.
- Phase 2 `ReadFailure` already owns `Transport(String)`, and Phase 2 `AppEvent::ScopedReadFailed` is exactly `{ context: RequestContext, failure: ReadFailure, retry: Option<ReadOperation> }`. Phase 4 adds variants to that existing failure enum and consumes the inherited event unchanged.
- Phase 2 runtime ownership remains the single `EffectRunner<A, S, T>` with concrete `TaskRegistry` and bounded `mpsc::Sender<AppEvent>`; Phase 4 adds fields and dispatch arms to that runner without adding generic parameters or a second runner.
- Phase 2 reducer shape remains `reduce(&mut AppState, AppEvent) -> Transition`, where `Transition { effects: Vec<Effect> }`.
- Phase 3 activity history uses `ActivityState::push_recent(ActivityItem { label, outcome, detail })` and `ActivityOutcome::{Started, Succeeded, Failed, Cancelled}`.
- Current `QueryResult` is exactly `schema: Vec<SchemaElement>`, `rows: Vec<Vec<Value>>`, and `total_duration_micros: u64`.
- Current `SchemaElement.name` is `String`.
- Current WebSocket protocol is the externally tagged `WsServerMessage::{InitialSubscription, TransactionUpdate, IdentityToken}`.
- Current `TableUpdate` is exactly `{ table_id: u32, table_name: String, num_rows: u64, inserts: Vec<Value>, deletes: Vec<Value> }`.
- Current `InitialSubscriptionPayload` preserves `total_host_execution_duration: Option<Value>`.
- Current `TransactionUpdatePayload` preserves `status`, `database_update`, and flattened `extra`.
- Current `IdentityTokenPayload` preserves `identity`, `token`, and flattened `extra`.

## Dependency order and memory contract

Tasks are independently compilable in order. A task must not refer to a type first introduced by a later task.

Default budgets are deliberately below the 64 MiB global row-cache limit:

```rust
pub const DEFAULT_GLOBAL_ROW_LIMIT: usize = 10_000;
pub const DEFAULT_GLOBAL_BYTE_LIMIT: usize = 64 * 1024 * 1024;
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 128;
pub const DEFAULT_HTTP_BODY_BYTE_LIMIT: usize = 16 * 1024 * 1024;
pub const DEFAULT_WS_FRAME_BYTE_LIMIT: usize = 2 * 1024 * 1024;
pub const DEFAULT_WS_MESSAGE_BYTE_LIMIT: usize = 8 * 1024 * 1024;
pub const DEFAULT_DECODED_BATCH_BYTE_LIMIT: usize = 16 * 1024 * 1024;
pub const DEFAULT_DECODED_BATCH_ROW_LIMIT: usize = 2_000;
pub const DEFAULT_SINGLE_ROW_BYTE_LIMIT: usize = 512 * 1024;
```

The HTTP peak check is `body + decoded batch + one temporary row < global bytes`. The WebSocket peak check is `message + decoded batch + one temporary row < global bytes`. All additions, subtractions, conversions, and LRU sequence increments use checked arithmetic.

---

### Task 1: Extend the exact Phase 2 ID port with typed subscription IDs

**Files:**
- Modify: `src/effects/ports.rs`
- Test: `src/effects/ports.rs`
- Test only, no production change required: `src/effects/task_registry.rs`

- [ ] **Step 1 RED: Add exact-owner tests**

Append to the existing `src/effects/ports.rs` test module. `FakeIdSource` is the exact Phase 2 fake whose constructor is `FakeIdSource::new(value)` and whose `next_id` increments before returning.

```rust
#[test]
fn phase4_id_extension_uses_next_id_and_checked_u32_conversion() {
    let mut ids = FakeIdSource::new(9);

    assert_eq!(ids.next_subscription_request_id().unwrap(), 10);
    assert_eq!(ids.next_task_generation(), TaskGeneration(11));
    assert_eq!(ids.next_connection_generation(), ConnectionGeneration(12));
}

#[test]
fn subscription_request_id_reports_the_rejected_u64() {
    let mut ids = FakeIdSource::new(u64::from(u32::MAX));

    assert_eq!(
        ids.next_subscription_request_id(),
        Err(SubscriptionRequestIdOverflow {
            value: u64::from(u32::MAX) + 1,
        })
    );
}

#[test]
fn phase2_backoff_signature_remains_the_only_backoff_owner() {
    let backoff = FixedBackoffPolicy {
        base: std::time::Duration::from_millis(100),
    };
    let mut random = FakeRandomSource::new(vec![3]);

    assert_eq!(
        backoff.delay(2, &mut random),
        std::time::Duration::from_millis(203)
    );
}
```

Append this shape assertion to the existing `src/effects/task_registry.rs` tests:

```rust
#[test]
fn phase4_reuses_the_exact_phase1_task_outcome_variants() {
    let outcomes = [
        TaskOutcome::Completed,
        TaskOutcome::Cancelled,
        TaskOutcome::Panicked {
            message: "boom".to_string(),
        },
    ];

    assert!(matches!(&outcomes[0], TaskOutcome::Completed));
    assert!(matches!(&outcomes[1], TaskOutcome::Cancelled));
    assert!(matches!(&outcomes[2], TaskOutcome::Panicked { message } if message == "boom"));
}
```

Run: `cargo test phase4_id_extension_uses_next_id_and_checked_u32_conversion -- --nocapture`

Expected: FAIL because the typed extension methods do not exist.

- [ ] **Step 2 GREEN: Add newtypes, a defined conversion error, and a blanket extension impl**

Add to `src/effects/ports.rs` without changing the existing `IdSource`, `RandomSource`, or `BackoffPolicy` traits:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnectionGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionRequestIdOverflow {
    pub value: u64,
}

impl std::fmt::Display for SubscriptionRequestIdOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "subscription request id {} does not fit in u32", self.value)
    }
}

impl std::error::Error for SubscriptionRequestIdOverflow {}

pub trait IdSourcePhase4Ext: IdSource {
    fn next_subscription_request_id(
        &mut self,
    ) -> Result<u32, SubscriptionRequestIdOverflow> {
        let value = self.next_id();
        u32::try_from(value).map_err(|_| SubscriptionRequestIdOverflow { value })
    }

    fn next_task_generation(&mut self) -> TaskGeneration {
        TaskGeneration(self.next_id())
    }

    fn next_connection_generation(&mut self) -> ConnectionGeneration {
        ConnectionGeneration(self.next_id())
    }
}

impl<T: IdSource + ?Sized> IdSourcePhase4Ext for T {}
```

Stable connection does not reset a policy object. Task 7 resets only `state.activity.retry.attempt` and `state.activity.retry.next_retry_label`.

- [ ] **Step 3 GREEN: Run exact tests and duplicate-owner scan**

```bash
cargo test phase4_id_extension_uses_next_id_and_checked_u32_conversion -- --nocapture
cargo test subscription_request_id_reports_the_rejected_u64 -- --nocapture
cargo test phase4_reuses_the_exact_phase1_task_outcome_variants -- --nocapture
rg -n "trait IdSource|trait BackoffPolicy|enum TaskOutcome" src/effects
```

Expected: tests PASS. The scan shows one `IdSource`, one `BackoffPolicy`, and one `TaskOutcome` definition.

- [ ] **Step 4 Commit**

```bash
git add src/effects/ports.rs src/effects/task_registry.rs
git commit -m "feat: add typed subscription id extensions"
```

---

### Task 2: Define configurable budgets and checked no-clone JSON estimation

**Files:**
- Create: `src/resource/mod.rs`
- Create: `src/resource/budget.rs`
- Create: `src/resource/estimate.rs`
- Modify: `src/main.rs`
- Modify: `src/config.rs`
- Modify: `src/user_config.rs`

- [ ] **Step 1 RED: Add budget, headroom, override, and estimator tests**

Create `src/resource/budget.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_leave_parser_and_state_headroom_below_64_mib() {
        let budgets = ResourceBudgets::default();
        assert_eq!(budgets.global_byte_limit, 64 * 1024 * 1024);
        assert_eq!(budgets.global_row_limit, 10_000);
        assert_eq!(budgets.decoded_batch_row_limit, 2_000);

        let http_peak = budgets
            .http_body_byte_limit
            .checked_add(budgets.decoded_batch_byte_limit)
            .and_then(|value| value.checked_add(budgets.single_row_byte_limit))
            .unwrap();
        let ws_peak = budgets
            .ws_message_byte_limit
            .checked_add(budgets.decoded_batch_byte_limit)
            .and_then(|value| value.checked_add(budgets.single_row_byte_limit))
            .unwrap();

        assert!(http_peak < budgets.global_byte_limit);
        assert!(ws_peak < budgets.global_byte_limit);
        assert!(budgets.ws_frame_byte_limit <= budgets.ws_message_byte_limit);
        budgets.validate().unwrap();
    }

    #[test]
    fn user_overrides_are_applied_and_revalidated() {
        let overrides = ResourceBudgetOverrides {
            decoded_batch_row_limit: Some(25),
            event_queue_capacity: Some(8),
            ..ResourceBudgetOverrides::default()
        };

        let budgets = overrides.apply(ResourceBudgets::default()).unwrap();

        assert_eq!(budgets.decoded_batch_row_limit, 25);
        assert_eq!(budgets.event_queue_capacity, 8);
    }

    #[test]
    fn invalid_override_is_rejected_with_a_named_field() {
        let overrides = ResourceBudgetOverrides {
            ws_frame_byte_limit: Some(9),
            ws_message_byte_limit: Some(8),
            ..ResourceBudgetOverrides::default()
        };

        assert_eq!(
            overrides.apply(ResourceBudgets::default()),
            Err(BudgetConfigError::FrameExceedsMessage {
                frame: 9,
                message: 8,
            })
        );
    }
}
```

Create `src/resource/estimate.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn estimate_json_row_bytes_is_checked_deterministic_and_borrowed() {
        let row = vec![json!(1), json!("alice"), json!({"active": true})];
        let before = row.clone();

        let first = estimate_json_row_bytes(&row).unwrap();
        let second = estimate_json_row_bytes(&row).unwrap();

        assert_eq!(first, second);
        assert_eq!(row, before);
    }

    #[test]
    fn checked_addition_reports_arithmetic_overflow() {
        assert_eq!(
            checked_estimate_add(usize::MAX, 1),
            Err(BudgetExceeded::BudgetArithmeticOverflow)
        );
    }
}
```

Run: `cargo test defaults_leave_parser_and_state_headroom_below_64_mib -- --nocapture`

Expected: FAIL because the resource modules do not exist.

- [ ] **Step 2 GREEN: Add budget types, validation, and persisted overrides**

Create `src/resource/mod.rs`:

```rust
pub mod budget;
pub mod estimate;
```

Create `src/resource/budget.rs`:

```rust
pub const DEFAULT_GLOBAL_ROW_LIMIT: usize = 10_000;
pub const DEFAULT_GLOBAL_BYTE_LIMIT: usize = 64 * 1024 * 1024;
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 128;
pub const DEFAULT_HTTP_BODY_BYTE_LIMIT: usize = 16 * 1024 * 1024;
pub const DEFAULT_WS_FRAME_BYTE_LIMIT: usize = 2 * 1024 * 1024;
pub const DEFAULT_WS_MESSAGE_BYTE_LIMIT: usize = 8 * 1024 * 1024;
pub const DEFAULT_DECODED_BATCH_BYTE_LIMIT: usize = 16 * 1024 * 1024;
pub const DEFAULT_DECODED_BATCH_ROW_LIMIT: usize = 2_000;
pub const DEFAULT_SINGLE_ROW_BYTE_LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceBudgets {
    pub global_row_limit: usize,
    pub global_byte_limit: usize,
    pub http_body_byte_limit: usize,
    pub ws_frame_byte_limit: usize,
    pub ws_message_byte_limit: usize,
    pub decoded_batch_byte_limit: usize,
    pub decoded_batch_row_limit: usize,
    pub single_row_byte_limit: usize,
    pub event_queue_capacity: usize,
}

impl Default for ResourceBudgets {
    fn default() -> Self {
        Self {
            global_row_limit: DEFAULT_GLOBAL_ROW_LIMIT,
            global_byte_limit: DEFAULT_GLOBAL_BYTE_LIMIT,
            http_body_byte_limit: DEFAULT_HTTP_BODY_BYTE_LIMIT,
            ws_frame_byte_limit: DEFAULT_WS_FRAME_BYTE_LIMIT,
            ws_message_byte_limit: DEFAULT_WS_MESSAGE_BYTE_LIMIT,
            decoded_batch_byte_limit: DEFAULT_DECODED_BATCH_BYTE_LIMIT,
            decoded_batch_row_limit: DEFAULT_DECODED_BATCH_ROW_LIMIT,
            single_row_byte_limit: DEFAULT_SINGLE_ROW_BYTE_LIMIT,
            event_queue_capacity: DEFAULT_EVENT_QUEUE_CAPACITY,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetExceeded {
    HttpBody { limit: usize, actual: usize },
    HttpBodyLengthOverflow { advertised: u64 },
    WsFrame { limit: usize, actual: usize },
    WsMessage { limit: usize, actual: usize },
    DecodedBatchBytes { limit: usize, estimated: usize },
    DecodedBatchRows { limit: usize, actual: usize },
    SingleRow { limit: usize, estimated: usize },
    CacheRows { limit: usize, requested: usize },
    CacheBytes { limit: usize, requested: usize },
    EventQueue { capacity: usize },
    BudgetArithmeticOverflow,
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for BudgetExceeded {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetConfigError {
    Zero { field: &'static str },
    FrameExceedsMessage { frame: usize, message: usize },
    DecodedRowsExceedGlobal { decoded: usize, global: usize },
    PeakArithmeticOverflow,
    HttpPeakExceedsGlobal { peak: usize, global: usize },
    WsPeakExceedsGlobal { peak: usize, global: usize },
}

impl ResourceBudgets {
    pub fn validate(self) -> Result<Self, BudgetConfigError> {
        for (field, value) in [
            ("global_row_limit", self.global_row_limit),
            ("global_byte_limit", self.global_byte_limit),
            ("http_body_byte_limit", self.http_body_byte_limit),
            ("ws_frame_byte_limit", self.ws_frame_byte_limit),
            ("ws_message_byte_limit", self.ws_message_byte_limit),
            ("decoded_batch_byte_limit", self.decoded_batch_byte_limit),
            ("decoded_batch_row_limit", self.decoded_batch_row_limit),
            ("single_row_byte_limit", self.single_row_byte_limit),
            ("event_queue_capacity", self.event_queue_capacity),
        ] {
            if value == 0 {
                return Err(BudgetConfigError::Zero { field });
            }
        }
        if self.ws_frame_byte_limit > self.ws_message_byte_limit {
            return Err(BudgetConfigError::FrameExceedsMessage {
                frame: self.ws_frame_byte_limit,
                message: self.ws_message_byte_limit,
            });
        }
        if self.decoded_batch_row_limit > self.global_row_limit {
            return Err(BudgetConfigError::DecodedRowsExceedGlobal {
                decoded: self.decoded_batch_row_limit,
                global: self.global_row_limit,
            });
        }
        let http_peak = self
            .http_body_byte_limit
            .checked_add(self.decoded_batch_byte_limit)
            .and_then(|value| value.checked_add(self.single_row_byte_limit))
            .ok_or(BudgetConfigError::PeakArithmeticOverflow)?;
        let ws_peak = self
            .ws_message_byte_limit
            .checked_add(self.decoded_batch_byte_limit)
            .and_then(|value| value.checked_add(self.single_row_byte_limit))
            .ok_or(BudgetConfigError::PeakArithmeticOverflow)?;
        if http_peak >= self.global_byte_limit {
            return Err(BudgetConfigError::HttpPeakExceedsGlobal {
                peak: http_peak,
                global: self.global_byte_limit,
            });
        }
        if ws_peak >= self.global_byte_limit {
            return Err(BudgetConfigError::WsPeakExceedsGlobal {
                peak: ws_peak,
                global: self.global_byte_limit,
            });
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ResourceBudgetOverrides {
    #[serde(default)]
    pub global_row_limit: Option<usize>,
    #[serde(default)]
    pub global_byte_limit: Option<usize>,
    #[serde(default)]
    pub http_body_byte_limit: Option<usize>,
    #[serde(default)]
    pub ws_frame_byte_limit: Option<usize>,
    #[serde(default)]
    pub ws_message_byte_limit: Option<usize>,
    #[serde(default)]
    pub decoded_batch_byte_limit: Option<usize>,
    #[serde(default)]
    pub decoded_batch_row_limit: Option<usize>,
    #[serde(default)]
    pub single_row_byte_limit: Option<usize>,
    #[serde(default)]
    pub event_queue_capacity: Option<usize>,
}

impl ResourceBudgetOverrides {
    pub fn apply(
        &self,
        mut budgets: ResourceBudgets,
    ) -> Result<ResourceBudgets, BudgetConfigError> {
        macro_rules! apply {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    budgets.$field = value;
                }
            };
        }
        apply!(global_row_limit);
        apply!(global_byte_limit);
        apply!(http_body_byte_limit);
        apply!(ws_frame_byte_limit);
        apply!(ws_message_byte_limit);
        apply!(decoded_batch_byte_limit);
        apply!(decoded_batch_row_limit);
        apply!(single_row_byte_limit);
        apply!(event_queue_capacity);
        budgets.validate()
    }
}
```

Add this exact field to the existing `UserConfig` in `src/user_config.rs`:

```rust
#[serde(default)]
pub resource_budgets: crate::resource::budget::ResourceBudgetOverrides,
```

Add this exact field to the existing resolved `Config` in `src/config.rs`:

```rust
pub resource_budgets: crate::resource::budget::ResourceBudgets,
```

In `Config::from_cli`, resolve it before moving `user_cfg` into `Config`:

```rust
let resource_budgets = user_cfg
    .resource_budgets
    .apply(crate::resource::budget::ResourceBudgets::default())
    .map_err(|error| anyhow::anyhow!("invalid resource budgets: {error:?}"))?;
```

Add `resource_budgets` to the final `Config` initializer. Add `mod resource;` to `src/main.rs`.

- [ ] **Step 3 GREEN: Add checked estimators that never clone a row**

Create `src/resource/estimate.rs`:

```rust
use crate::resource::budget::BudgetExceeded;
use serde_json::Value;

pub fn checked_estimate_add(
    left: usize,
    right: usize,
) -> Result<usize, BudgetExceeded> {
    left.checked_add(right)
        .ok_or(BudgetExceeded::BudgetArithmeticOverflow)
}

pub fn estimate_json_value_bytes(value: &Value) -> Result<usize, BudgetExceeded> {
    match value {
        Value::Null | Value::Bool(_) => Ok(8),
        Value::Number(_) => Ok(16),
        Value::String(value) => checked_estimate_add(24, value.len()),
        Value::Array(values) => {
            let mut total = 24;
            for value in values {
                total = checked_estimate_add(total, estimate_json_value_bytes(value)?)?;
            }
            Ok(total)
        }
        Value::Object(values) => {
            let mut total = 48;
            for (key, value) in values {
                total = checked_estimate_add(total, 24)?;
                total = checked_estimate_add(total, key.len())?;
                total = checked_estimate_add(total, estimate_json_value_bytes(value)?)?;
            }
            Ok(total)
        }
    }
}

pub fn estimate_json_row_bytes(row: &[Value]) -> Result<usize, BudgetExceeded> {
    let mut total = 24;
    for value in row {
        total = checked_estimate_add(total, estimate_json_value_bytes(value)?)?;
    }
    Ok(total)
}

pub fn estimate_query_result_bytes(
    result: &crate::api::types::QueryResult,
) -> Result<usize, BudgetExceeded> {
    let mut total = 128;
    for element in &result.schema {
        total = checked_estimate_add(total, 24)?;
        total = checked_estimate_add(total, element.name.len())?;
        total = checked_estimate_add(
            total,
            estimate_json_value_bytes(&element.algebraic_type)?,
        )?;
    }
    for row in &result.rows {
        total = checked_estimate_add(total, estimate_json_row_bytes(row)?)?;
    }
    Ok(total)
}
```

- [ ] **Step 4 GREEN: Run focused tests**

```bash
cargo test defaults_leave_parser_and_state_headroom_below_64_mib -- --nocapture
cargo test user_overrides_are_applied_and_revalidated -- --nocapture
cargo test estimate_json_row_bytes_is_checked_deterministic_and_borrowed -- --nocapture
```

Expected: PASS.

- [ ] **Step 5 Commit**

```bash
git add src/main.rs src/config.rs src/user_config.rs src/resource/mod.rs src/resource/budget.rs src/resource/estimate.rs
git commit -m "feat: add configurable resource budgets"
```

---

### Task 3: Enforce bounded HTTP bodies and stream SQL rows with a custom seed

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `src/api/decode_error.rs`
- Create: `src/api/http_body.rs`
- Create: `src/api/query_decode.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/api/client.rs`
- Modify: `src/effects/runner.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/state/resources.rs`
- Modify: `src/main.rs`
- Modify: `src/app.rs`

- [ ] **Step 1 RED: Add body-cap, protocol-shape, budget, and malformed tests**

Add these tests to the modules named by each test path:

```rust
// src/api/http_body.rs
#[test]
fn advertised_and_streamed_http_limits_are_both_enforced() {
    assert_eq!(
        check_advertised_length(Some(9), 8),
        Err(BudgetExceeded::HttpBody { limit: 8, actual: 9 })
    );
    let mut body = BoundedBodyAccumulator::new(4);
    body.push(b"123").unwrap();
    assert_eq!(
        body.push(b"45"),
        Err(BudgetExceeded::HttpBody { limit: 4, actual: 5 })
    );
}

// src/api/query_decode.rs
#[test]
fn sql_decoder_preserves_array_object_null_mutation_v1_and_v9_shapes() {
    let budgets = ResourceBudgets::default();
    let v1 = decode_query_result_bounded(
        br#"[{"schema":[{"name":"id","algebraic_type":{"U64":null}}],"rows":[[1]],"total_duration_micros":7}]"#,
        budgets,
    ).unwrap();
    assert_eq!(v1.schema[0].name, "id");
    assert_eq!(v1.rows, vec![vec![serde_json::json!(1)]]);
    assert_eq!(v1.total_duration_micros, 7);

    let v9 = decode_query_result_bounded(
        br#"{"schema":{"elements":[{"name":{"some":"name"},"algebraic_type":{"String":null}}]},"rows":[["Ada"]]}"#,
        budgets,
    ).unwrap();
    assert_eq!(v9.schema[0].name, "name");

    let mutation = decode_query_result_bounded(
        br#"[{"total_duration_micros":9,"rows":[["ignored"]]}]"#,
        budgets,
    ).unwrap();
    assert!(mutation.schema.is_empty());
    assert!(mutation.rows.is_empty());
    assert_eq!(mutation.total_duration_micros, 9);

    assert!(decode_query_result_bounded(b"[]", budgets).unwrap().rows.is_empty());
    assert!(decode_query_result_bounded(b"null", budgets).unwrap().rows.is_empty());
}

#[test]
fn sql_row_limit_is_budget_error_and_rows_are_not_returned() {
    let budgets = ResourceBudgets {
        decoded_batch_row_limit: 1,
        ..ResourceBudgets::default()
    };
    let error = decode_query_result_bounded(
        br#"[{"schema":[],"rows":[[1],[2]]}]"#,
        budgets,
    ).unwrap_err();

    assert!(matches!(
        error,
        DecodeError::Budget(BudgetExceeded::DecodedBatchRows { limit: 1, actual: 2 })
    ));
}

#[test]
fn malformed_sql_payload_is_protocol_error_not_arithmetic_overflow() {
    let error = decode_query_result_bounded(
        br#"[{"schema":[],"rows":[[1],]}]"#,
        ResourceBudgets::default(),
    ).unwrap_err();

    assert!(matches!(error, DecodeError::Protocol(_)));
}

#[test]
fn decode_meter_budget_state_is_consumed_through_typed_accessor() {
    let mut meter = DecodeMeter::default();
    let rejected: Result<(), serde_json::Error> = meter.accept_estimate(
        2,
        ResourceBudgets {
            single_row_byte_limit: 1,
            ..ResourceBudgets::default()
        },
    );

    assert!(rejected.is_err());
    assert!(matches!(
        meter.take_exceeded(),
        Some(BudgetExceeded::SingleRow { limit: 1, estimated: 2 })
    ));
    assert_eq!(meter.take_exceeded(), None);
}

// src/effects/runner.rs
#[test]
fn typed_http_status_survives_client_and_read_failure_adapter() {
    let error = anyhow::Error::new(crate::api::decode_error::HttpStatusError::new(
        503,
        "schema temporarily unavailable",
    ));

    assert_eq!(
        read_failure_from_anyhow(error),
        ReadFailure::HttpStatus(503),
    );
}
```

Add this direct reducer test to `src/app/reducer.rs`. Every fixture is local and uses real Phase 2 types:

```rust
fn phase4_query_result(rows: usize) -> crate::api::types::QueryResult {
    crate::api::types::QueryResult {
        schema: vec![crate::api::types::SchemaElement {
            name: "id".to_string(),
            algebraic_type: serde_json::json!({"U64": null}),
        }],
        rows: (0..rows).map(|value| vec![serde_json::json!(value)]).collect(),
        total_duration_micros: 0,
    }
}

#[test]
fn http_budget_failure_preserves_previous_rows_as_stale() {
    let context = RequestContext::new(
        RequestId::from_u64(7),
        RequestScope::TableRows {
            database: "db".into(),
            table: "users".into(),
            view: "browse".into(),
        },
        3,
    );
    let mut state = AppState::new("http://localhost:3000".to_string());
    state.resources.table_rows = LoadState::Refreshing {
        data: phase4_query_result(2),
        request: context.clone(),
    };

    let transition = reduce(
        &mut state,
        AppEvent::ScopedReadFailed {
            context,
            failure: ReadFailure::BudgetExceeded(BudgetExceeded::HttpBody {
                limit: 8,
                actual: 9,
            }),
            retry: None,
        },
    );

    assert!(transition.effects.is_empty());
    assert!(matches!(
        state.resources.table_rows,
        LoadState::Stale {
            reason: StaleReason::BudgetExceeded,
            ..
        }
    ));
}
```

Run: `cargo test malformed_sql_payload_is_protocol_error_not_arithmetic_overflow -- --nocapture`

Expected: FAIL because the typed decoder does not exist.

- [ ] **Step 2 GREEN: Enable borrowed raw JSON and add typed decode errors**

Change the existing dependency line in `Cargo.toml`:

```toml
serde_json = { version = "1", features = ["raw_value"] }
```

Run `cargo check` once so Cargo updates `Cargo.lock`. Keep both files in this task and commit.

Create `src/api/decode_error.rs`:

```rust
use crate::resource::budget::BudgetExceeded;

#[derive(Debug)]
pub enum DecodeError {
    Protocol(serde_json::Error),
    Budget(BudgetExceeded),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(error) => write!(f, "malformed protocol payload: {error}"),
            Self::Budget(error) => write!(f, "resource budget exceeded: {error}"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Protocol(error) => Some(error),
            Self::Budget(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpStatusError {
    status: u16,
    detail: String,
}

impl HttpStatusError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }

    pub fn status(&self) -> u16 {
        self.status
    }
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}: {}", self.status, self.detail)
    }
}

impl std::error::Error for HttpStatusError {}
```

Export all Task 3 modules from `src/api/mod.rs`:

```rust
pub mod decode_error;
pub mod http_body;
pub mod query_decode;
```

- [ ] **Step 3 GREEN: Collect decompressed HTTP bytes under advertised and actual limits**

Create `src/api/http_body.rs`:

```rust
use crate::resource::budget::BudgetExceeded;
use futures_util::StreamExt;

pub fn check_advertised_length(
    content_length: Option<u64>,
    limit: usize,
) -> Result<(), BudgetExceeded> {
    if let Some(advertised) = content_length {
        let actual = usize::try_from(advertised)
            .map_err(|_| BudgetExceeded::HttpBodyLengthOverflow { advertised })?;
        if actual > limit {
            return Err(BudgetExceeded::HttpBody { limit, actual });
        }
    }
    Ok(())
}

pub struct BoundedBodyAccumulator {
    limit: usize,
    bytes: Vec<u8>,
}

impl BoundedBodyAccumulator {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            bytes: Vec::new(),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), BudgetExceeded> {
        let actual = self
            .bytes
            .len()
            .checked_add(chunk.len())
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
        if actual > self.limit {
            return Err(BudgetExceeded::HttpBody {
                limit: self.limit,
                actual,
            });
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[derive(Debug)]
pub enum BodyReadError {
    Transport(reqwest::Error),
    Budget(BudgetExceeded),
}

impl std::fmt::Display for BodyReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "HTTP body transport error: {error}"),
            Self::Budget(error) => write!(f, "HTTP body budget exceeded: {error}"),
        }
    }
}

impl std::error::Error for BodyReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Budget(error) => Some(error),
        }
    }
}

pub async fn collect_bounded_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, BodyReadError> {
    check_advertised_length(response.content_length(), limit)
        .map_err(BodyReadError::Budget)?;
    let mut body = BoundedBodyAccumulator::new(limit);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(BodyReadError::Transport)?;
        body.push(&chunk).map_err(BodyReadError::Budget)?;
    }
    Ok(body.finish())
}
```

Reqwest exposes bytes after its enabled transport decompression, so the streamed limit applies to the decoded HTTP entity, including responses without `Content-Length`.

- [ ] **Step 4 GREEN: Add the complete custom SQL envelope and row-array visitors**

Create `src/api/query_decode.rs` with the following production structure. The visitor deliberately stores only the encoded `rows` array as `&RawValue`; it never creates a pointer vector for all rows.

```rust
use crate::api::decode_error::DecodeError;
use crate::api::types::{QueryResult, SchemaElement};
use crate::resource::budget::{BudgetExceeded, ResourceBudgets};
use crate::resource::estimate::{estimate_json_row_bytes, estimate_json_value_bytes};
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{value::RawValue, Value};
use std::fmt;

#[derive(Default)]
pub(crate) struct DecodeMeter {
    rows: usize,
    estimated_bytes: usize,
    exceeded: Option<BudgetExceeded>,
}

impl DecodeMeter {
    pub(crate) fn accept_estimate<E: de::Error>(
        &mut self,
        estimate: usize,
        budgets: ResourceBudgets,
    ) -> Result<(), E> {
        if estimate > budgets.single_row_byte_limit {
            return Err(self.reject(BudgetExceeded::SingleRow {
                limit: budgets.single_row_byte_limit,
                estimated: estimate,
            }));
        }
        let rows = self.rows.checked_add(1).ok_or_else(|| {
            self.reject(BudgetExceeded::BudgetArithmeticOverflow)
        })?;
        if rows > budgets.decoded_batch_row_limit {
            return Err(self.reject(BudgetExceeded::DecodedBatchRows {
                limit: budgets.decoded_batch_row_limit,
                actual: rows,
            }));
        }
        let estimated_bytes = self.estimated_bytes.checked_add(estimate).ok_or_else(|| {
            self.reject(BudgetExceeded::BudgetArithmeticOverflow)
        })?;
        if estimated_bytes > budgets.decoded_batch_byte_limit {
            return Err(self.reject(BudgetExceeded::DecodedBatchBytes {
                limit: budgets.decoded_batch_byte_limit,
                estimated: estimated_bytes,
            }));
        }
        self.rows = rows;
        self.estimated_bytes = estimated_bytes;
        Ok(())
    }

    pub(crate) fn accept_query_row<E: de::Error>(
        &mut self,
        row: &[Value],
        budgets: ResourceBudgets,
    ) -> Result<(), E> {
        let estimate = estimate_json_row_bytes(row).map_err(|error| self.reject(error))?;
        self.accept_estimate(estimate, budgets)
    }

    pub(crate) fn accept_json_value<E: de::Error>(
        &mut self,
        value: &Value,
        budgets: ResourceBudgets,
    ) -> Result<(), E> {
        let estimate = estimate_json_value_bytes(value).map_err(|error| self.reject(error))?;
        self.accept_estimate(estimate, budgets)
    }

    fn reject<E: de::Error>(&mut self, error: BudgetExceeded) -> E {
        self.exceeded = Some(error);
        E::custom("phase4 decode budget exceeded")
    }

    pub(crate) fn take_exceeded(&mut self) -> Option<BudgetExceeded> {
        self.exceeded.take()
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SchemaNameWire {
    Plain(String),
    Some { some: String },
}

#[derive(Deserialize)]
struct SchemaColumnWire {
    #[serde(default)]
    name: Option<SchemaNameWire>,
    #[serde(default)]
    algebraic_type: Value,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SchemaWire {
    V1(Vec<SchemaColumnWire>),
    V9 { elements: Vec<SchemaColumnWire> },
}

impl SchemaWire {
    fn into_schema(self) -> Vec<SchemaElement> {
        let columns = match self {
            Self::V1(columns) => columns,
            Self::V9 { elements } => elements,
        };
        columns
            .into_iter()
            .map(|column| SchemaElement {
                name: match column.name {
                    Some(SchemaNameWire::Plain(name)) => name,
                    Some(SchemaNameWire::Some { some }) => some,
                    None => String::new(),
                },
                algebraic_type: column.algebraic_type,
            })
            .collect()
    }
}

struct RowsSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for RowsSeed<'_> {
    type Value = Vec<Vec<Value>>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(RowsVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct RowsVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> Visitor<'de> for RowsVisitor<'_> {
    type Value = Vec<Vec<Value>>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SQL rows array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut rows = Vec::new();
        while let Some(row) = sequence.next_element::<Vec<Value>>()? {
            self.meter
                .accept_query_row::<A::Error>(&row, self.budgets)?;
            rows.push(row);
        }
        Ok(rows)
    }
}

fn decode_rows(
    raw: &RawValue,
    meter: &mut DecodeMeter,
    budgets: ResourceBudgets,
) -> Result<Vec<Vec<Value>>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let rows = RowsSeed { meter, budgets }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(rows)
}

fn decode_schema(raw: &RawValue) -> Result<Vec<SchemaElement>, serde_json::Error> {
    serde_json::from_str::<SchemaWire>(raw.get()).map(SchemaWire::into_schema)
}

struct ResultSetSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for ResultSetSeed<'_> {
    type Value = QueryResult;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(ResultSetVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct ResultSetVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> Visitor<'de> for ResultSetVisitor<'_> {
    type Value = QueryResult;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SQL result object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut schema_raw: Option<&'de RawValue> = None;
        let mut rows_raw: Option<&'de RawValue> = None;
        let mut total_duration_micros = 0;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "schema" => schema_raw = Some(map.next_value()?),
                "rows" => rows_raw = Some(map.next_value()?),
                "total_duration_micros" => {
                    total_duration_micros = map.next_value()?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let Some(schema_raw) = schema_raw else {
            return Ok(QueryResult {
                schema: Vec::new(),
                rows: Vec::new(),
                total_duration_micros,
            });
        };
        let schema = decode_schema(schema_raw).map_err(de::Error::custom)?;
        let rows = match rows_raw {
            Some(raw) => decode_rows(raw, self.meter, self.budgets)
                .map_err(de::Error::custom)?,
            None => Vec::new(),
        };
        Ok(QueryResult {
            schema,
            rows,
            total_duration_micros,
        })
    }
}

struct SqlEnvelopeSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for SqlEnvelopeSeed<'_> {
    type Value = QueryResult;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(SqlEnvelopeVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct SqlEnvelopeVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

fn empty_query_result() -> QueryResult {
    QueryResult {
        schema: Vec::new(),
        rows: Vec::new(),
        total_duration_micros: 0,
    }
}

impl<'de> Visitor<'de> for SqlEnvelopeVisitor<'_> {
    type Value = QueryResult;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SQL result object, result array, or null")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(empty_query_result())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        ResultSetVisitor {
            meter: self.meter,
            budgets: self.budgets,
        }
        .visit_map(map)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let first: Option<&'de RawValue> = sequence.next_element()?;
        while sequence.next_element::<IgnoredAny>()?.is_some() {}
        let Some(first) = first else {
            return Ok(empty_query_result());
        };
        let mut deserializer = serde_json::Deserializer::from_str(first.get());
        let result = ResultSetSeed {
            meter: self.meter,
            budgets: self.budgets,
        }
        .deserialize(&mut deserializer)
        .map_err(de::Error::custom)?;
        deserializer.end().map_err(de::Error::custom)?;
        Ok(result)
    }
}

pub fn decode_query_result_bounded(
    body: &[u8],
    budgets: ResourceBudgets,
) -> Result<QueryResult, DecodeError> {
    let mut meter = DecodeMeter::default();
    let decoded = {
        let mut deserializer = serde_json::Deserializer::from_slice(body);
        SqlEnvelopeSeed {
            meter: &mut meter,
            budgets,
        }
        .deserialize(&mut deserializer)
        .and_then(|result| {
            deserializer.end()?;
            Ok(result)
        })
    };
    match decoded {
        Ok(result) => Ok(result),
        Err(error) => match meter.take_exceeded() {
            Some(exceeded) => Err(DecodeError::Budget(exceeded)),
            None => Err(DecodeError::Protocol(error)),
        },
    }
}
```

This exact design permits one temporary `Vec<Value>` row under the already-enforced HTTP body cap. It estimates the borrowed row slice, checks every arithmetic operation, and pushes only after acceptance.

- [ ] **Step 5 GREEN: Wire the bounded path into `SpacetimeClient::query_sql`**

Add `budgets: ResourceBudgets` to `SpacetimeClient` and add it as the final parameter of `SpacetimeClient::new`. In `src/main.rs` and `src/app.rs`, change each construction from `SpacetimeClient::new(base_url, auth_token)` to `SpacetimeClient::new(base_url, auth_token, config.resource_budgets)`. Replace only the current `resp.json::<Value>()` and `parse_query_result(raw)` lines with:

```rust
let body = crate::api::http_body::collect_bounded_body(
    resp,
    self.budgets.http_body_byte_limit,
)
.await
.map_err(|error| match error {
    crate::api::http_body::BodyReadError::Transport(error) => anyhow::Error::new(error),
    crate::api::http_body::BodyReadError::Budget(exceeded) => anyhow::Error::new(exceeded),
})?;

crate::api::query_decode::decode_query_result_bounded(&body, self.budgets)
    .map_err(|error| match error {
        crate::api::decode_error::DecodeError::Budget(exceeded) => anyhow::Error::new(exceeded),
        protocol @ crate::api::decode_error::DecodeError::Protocol(_) => anyhow::Error::new(protocol),
    })
```

Before converting endpoint errors to `anyhow::Error`, preserve response status with the typed wrapper. Import `crate::api::decode_error::HttpStatusError` in `src/api/client.rs`. Replace the non-success branches in all three Phase 2 read paths, `query_sql`, `get_schema`, and `list_databases`, so each returns `HttpStatusError` instead of `bail!`-formatted text. Keep the current bounded/snipped body wording, but construct the error with the numeric status:

```rust
// query_sql
if !status.is_success() {
    let body = resp.text().await.unwrap_or_default();
    return Err(HttpStatusError::new(
        status.as_u16(),
        format!("SQL query failed: {body}"),
    ).into());
}

// get_schema: preserve the current 500/404 guidance in `detail`.
if !status.is_success() {
    let body = resp.text().await.unwrap_or_default();
    let body_snip: String = body.chars().take(200).collect();
    let detail = match status.as_u16() {
        500 => format!(
            "schema for '{database}' could not be serialized; server body: {body_snip}",
        ),
        404 => format!(
            "database '{database}' does not exist or is not visible to the current identity",
        ),
        _ => format!("schema request failed: {body_snip}"),
    };
    return Err(HttpStatusError::new(status.as_u16(), detail).into());
}

// list_databases
if !status.is_success() {
    let body = resp.text().await.unwrap_or_default();
    return Err(HttpStatusError::new(
        status.as_u16(),
        format!("list databases failed: {body}"),
    ).into());
}
```

Do not stringify these statuses before they cross into `SpacetimeApiTransport`; otherwise `HttpStatus(429 | 500 | 502 | 503 | 504)` can never reach the retry classifier.

In the existing read-effect error adapter, preserve the typed downcasts:

```rust
use crate::api::decode_error::{DecodeError, HttpStatusError};
use crate::resource::budget::BudgetExceeded;

fn read_failure_from_anyhow(error: anyhow::Error) -> ReadFailure {
    match error.downcast::<BudgetExceeded>() {
        Ok(exceeded) => ReadFailure::BudgetExceeded(exceeded),
        Err(error) => match error.downcast::<DecodeError>() {
            Ok(DecodeError::Protocol(protocol)) => ReadFailure::Protocol(protocol.to_string()),
            Ok(DecodeError::Budget(exceeded)) => ReadFailure::BudgetExceeded(exceeded),
            Err(error) => match error.downcast::<HttpStatusError>() {
                Ok(http) => ReadFailure::HttpStatus(http.status()),
                Err(error) => match error.downcast::<reqwest::Error>() {
                    Ok(network) => ReadFailure::Network(network.to_string()),
                    Err(error) => ReadFailure::Transport(error.to_string()),
                },
            },
        },
    }
}
```

Replace the error branch in the inherited Phase 2 `SpacetimeApiTransport::execute_read` adapter with this exact constructor logic. The adapter already owns the original typed `retry: ReadOperation`; it must never reconstruct it from display text:

```rust
Err(error) => {
    let failure = read_failure_from_anyhow(error);
    let retry = match &failure {
        ReadFailure::Transport(_) | ReadFailure::Network(_) => Some(retry),
        ReadFailure::HttpStatus(429 | 500 | 502 | 503 | 504) => Some(retry),
        ReadFailure::BudgetExceeded(_)
        | ReadFailure::Protocol(_)
        | ReadFailure::HttpStatus(_) => None,
    };
    AppEvent::ScopedReadFailed {
        context,
        failure,
        retry,
    }
}
```

Remove the old `parse_query_result(Value)` function after migrating its existing tests to `decode_query_result_bounded`. Metadata endpoints may still decode a bounded complete value, but row-query endpoints must use this seed.

- [ ] **Step 6 GREEN: Extend the inherited typed read failure and direct `LoadState` reducer handling**

Modify the single Phase 2 `ReadFailure` enum in `src/app/event.rs`; preserve its inherited `Transport(String)` variant and append only the Phase 4 variants below inside that existing enum. Do not repeat its derive, visibility, enum header, inherited variant, or closing brace. Do not redefine `AppEvent::ScopedReadFailed`, whose exact inherited shape already includes `retry: Option<ReadOperation>`:

```rust
BudgetExceeded(crate::resource::budget::BudgetExceeded),
Protocol(String),
Network(String),
HttpStatus(u16),
```

In `src/app/reducer.rs`, add this exact `LoadState` helper, replace the Phase 2 `apply_scoped_read_failure` body with the scope-complete version below, and keep the inherited event arm shape. The reducer must still reject stale contexts before applying the failure:

```rust
fn apply_load_failure<T>(
    load: &mut LoadState<T>,
    delivered: &RequestContext,
    failure: &ReadFailure,
) {
    let replacement = match std::mem::replace(load, LoadState::Idle) {
        LoadState::Refreshing { data, request } if request.accepts(delivered) => {
            let reason = match failure {
                ReadFailure::BudgetExceeded(_) => StaleReason::BudgetExceeded,
                _ => StaleReason::ManualRefreshRequired,
            };
            LoadState::Stale { data, reason }
        }
        LoadState::Loading { request } if request.accepts(delivered) => LoadState::Error {
            previous: None,
            error: AppError {
                message: failure_message(failure),
            },
        },
        other => other,
    };
    *load = replacement;
}

fn failure_message(failure: &ReadFailure) -> String {
    match failure {
        ReadFailure::Transport(message) => format!("Transport error: {message}"),
        ReadFailure::BudgetExceeded(error) => format!("Budget exceeded: {error}"),
        ReadFailure::Protocol(message) => format!("Protocol error: {message}"),
        ReadFailure::Network(message) => format!("Network error: {message}"),
        ReadFailure::HttpStatus(status) => format!("HTTP {status}"),
    }
}

fn apply_scoped_read_failure(
    state: &mut AppState,
    context: &RequestContext,
    failure: &ReadFailure,
) {
    match context.scope() {
        RequestScope::DatabaseCatalog => {
            apply_load_failure(&mut state.resources.catalog, context, failure)
        }
        RequestScope::Schema { .. } => {
            apply_load_failure(&mut state.resources.schema, context, failure)
        }
        RequestScope::TableRows { .. } | RequestScope::SqlWorkspace { .. } => {
            apply_load_failure(&mut state.resources.table_rows, context, failure)
        }
        RequestScope::Logs { .. } => {
            apply_load_failure(&mut state.resources.logs, context, failure)
        }
        RequestScope::Metrics { .. } => {
            apply_load_failure(&mut state.resources.metrics, context, failure)
        }
        RequestScope::LiveClients { .. } => {
            apply_load_failure(&mut state.resources.live_clients, context, failure)
        }
    }
}

AppEvent::ScopedReadFailed { context, failure, retry: _ } => {
    if state.requests.is_current(&context) {
        apply_scoped_read_failure(state, &context, &failure);
    }
    Transition::none()
}
```

All Task 3 `ScopedReadFailed` constructors retain the exact inherited retry field. Budget, protocol, deterministic identifier-validation, and non-retryable HTTP failures use `retry: None`; transport, network, and retryable HTTP failures retain the exact typed `ReadOperation` produced by the Phase 2 effect adapter. Producers for `SqlWorkspace`, `Logs`, `Metrics`, and `LiveClients` use `retry: None` because no exact Phase 2 `ReadOperation` exists for those scopes. Do not add a second resource presentation enum.

- [ ] **Step 7 GREEN: Run focused checks**

```bash
cargo test sql_decoder_preserves_array_object_null_mutation_v1_and_v9_shapes -- --nocapture
cargo test sql_row_limit_is_budget_error_and_rows_are_not_returned -- --nocapture
cargo test malformed_sql_payload_is_protocol_error_not_arithmetic_overflow -- --nocapture
cargo test http_budget_failure_preserves_previous_rows_as_stale -- --nocapture
cargo test typed_http_status_survives_client_and_read_failure_adapter -- --nocapture
python3 - <<'PY'
from pathlib import Path
client = Path('src/api/client.rs').read_text()
assert client.count('HttpStatusError::new') >= 3
for old in ['bail!("SQL query HTTP', 'bail!("Schema HTTP', 'bail!("List databases HTTP']:
    assert old not in client, old
PY
cargo check
```

Expected: PASS.

- [ ] **Step 8 Commit**

```bash
git add Cargo.toml Cargo.lock src/api/decode_error.rs src/api/http_body.rs src/api/query_decode.rs src/api/mod.rs src/api/client.rs src/effects/runner.rs src/app/event.rs src/app/reducer.rs src/state/resources.rs src/main.rs src/app.rs
git commit -m "feat: bound and stream http row query decode"
```

---

### Task 4: Apply connect-time WebSocket caps and stream every update array

**Files:**
- Create: `src/api/ws_decode.rs`
- Modify: `src/api/mod.rs`
- Modify: `src/api/ws.rs`

- [ ] **Step 1 RED: Add connect-time cap tests**

Add to `src/api/ws.rs` tests:

```rust
#[test]
fn websocket_config_applies_frame_and_reassembled_message_limits() {
    let budgets = ResourceBudgets::default();
    let config = websocket_config_for_budgets(budgets);

    assert_eq!(config.max_frame_size, Some(budgets.ws_frame_byte_limit));
    assert_eq!(config.max_message_size, Some(budgets.ws_message_byte_limit));
}
```

Run: `cargo test websocket_config_applies_frame_and_reassembled_message_limits -- --nocapture`

Expected: FAIL because the helper and `WsConfig::budgets` do not exist.

- [ ] **Step 2 GREEN: Configure tungstenite before reading a frame**

Extend the existing `WsConfig` with:

```rust
pub budgets: crate::resource::budget::ResourceBudgets,
```

Replace `connect_async` imports and calls for subscription sockets with:

```rust
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::protocol::WebSocketConfig,
};

pub(crate) fn websocket_config_for_budgets(
    budgets: crate::resource::budget::ResourceBudgets,
) -> WebSocketConfig {
    WebSocketConfig {
        max_frame_size: Some(budgets.ws_frame_byte_limit),
        max_message_size: Some(budgets.ws_message_byte_limit),
        ..WebSocketConfig::default()
    }
}
```

At each subscription handshake:

```rust
let websocket_config = websocket_config_for_budgets(budgets);
let (ws_stream, _) = match connect_async_with_config(
    request,
    Some(websocket_config),
    false,
)
.await
{
    Ok(pair) => pair,
    Err(error) => return classify_connect_error(error),
};
```

Add this complete helper beside `ConnectOutcome` and reuse it from the existing connect loop:

```rust
fn classify_connect_error(error: tokio_tungstenite::tungstenite::Error) -> ConnectOutcome {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
        let status = response.status();
        if status.is_client_error() || status.is_server_error() {
            return ConnectOutcome::Fatal(format!("Server returned HTTP {status}"));
        }
    }
    ConnectOutcome::Lost(format!("Connect error: {error}"))
}
```

The max frame and max reassembled message sizes are active before tungstenite yields `Message`.

- [ ] **Step 3 RED: Add exact protocol preservation, budget, and malformed tests**

Create `src/api/ws_decode.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> SubscriptionContext {
        SubscriptionContext {
            key: SubscriptionKey {
                database: "db".into(),
                table: "users".into(),
            },
            task_generation: TaskGeneration(7),
            connection_generation: ConnectionGeneration(9),
            request_id: 42,
        }
    }

    #[test]
    fn ws_decoder_preserves_external_variants_and_exact_payload_fields() {
        let initial = decode_subscription_text_bounded(
            r#"{"InitialSubscription":{"request_id":42,"total_host_execution_duration":17,"database_update":{"tables":[{"table_id":3,"table_name":"users","num_rows":2,"inserts":[{"id":1}],"deletes":[{"id":2}]}]}}}"#,
            &context(),
            ResourceBudgets::default(),
        ).unwrap();
        let ScopedWsEvent::InitialSubscription { payload, .. } = initial else {
            panic!("expected initial subscription");
        };
        assert_eq!(payload.request_id, 42);
        assert_eq!(payload.total_host_execution_duration, Some(serde_json::json!(17)));
        assert_eq!(payload.database_update.tables[0].table_id, 3);
        assert_eq!(payload.database_update.tables[0].table_name, "users");
        assert_eq!(payload.database_update.tables[0].num_rows, 2);
        assert_eq!(payload.database_update.tables[0].inserts.len(), 1);
        assert_eq!(payload.database_update.tables[0].deletes.len(), 1);

        let transaction = decode_subscription_text_bounded(
            r#"{"TransactionUpdate":{"status":"committed","timestamp":99,"database_update":{"tables":[]}}}"#,
            &context(),
            ResourceBudgets::default(),
        ).unwrap();
        let ScopedWsEvent::TransactionUpdate { payload, .. } = transaction else {
            panic!("expected transaction update");
        };
        assert!(matches!(payload.status, Some(TransactionStatus::Committed)));
        assert_eq!(payload.extra.get("timestamp"), Some(&serde_json::json!(99)));

        let identity = decode_subscription_text_bounded(
            r#"{"IdentityToken":{"identity":{"hex":"01"},"token":"abc","connection_id":"c1"}}"#,
            &context(),
            ResourceBudgets::default(),
        ).unwrap();
        assert!(matches!(identity, ScopedWsEvent::IdentityToken { .. }));
    }

    #[test]
    fn ws_rows_are_streamed_and_rejected_before_push() {
        let budgets = ResourceBudgets {
            decoded_batch_row_limit: 1,
            ..ResourceBudgets::default()
        };
        let error = decode_subscription_text_bounded(
            r#"{"InitialSubscription":{"request_id":42,"database_update":{"tables":[{"table_id":3,"table_name":"users","num_rows":2,"inserts":[{"id":1},{"id":2}],"deletes":[]}]}}}"#,
            &context(),
            budgets,
        ).unwrap_err();

        assert!(matches!(
            error,
            DecodeError::Budget(BudgetExceeded::DecodedBatchRows { limit: 1, actual: 2 })
        ));
    }

    #[test]
    fn malformed_ws_payload_is_protocol_error() {
        let error = decode_subscription_text_bounded(
            r#"{"InitialSubscription":{"request_id":42,"database_update":{"tables":[}}}"#,
            &context(),
            ResourceBudgets::default(),
        ).unwrap_err();

        assert!(matches!(error, DecodeError::Protocol(_)));
    }
}
```

Run: `cargo test ws_rows_are_streamed_and_rejected_before_push -- --nocapture`

Expected: FAIL because the bounded decoder does not exist.

- [ ] **Step 4 GREEN: Define subscription identity and scoped event types before controller tasks use them**

Create the top of `src/api/ws_decode.rs`:

```rust
use crate::api::decode_error::DecodeError;
use crate::api::query_decode::DecodeMeter;
use crate::api::types::{
    DatabaseUpdate, IdentityTokenPayload, InitialSubscriptionPayload,
    TableUpdate, TransactionStatus, TransactionUpdatePayload,
};
use crate::effects::ports::{ConnectionGeneration, TaskGeneration};
use crate::resource::budget::ResourceBudgets;
use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{value::RawValue, Value};
use std::collections::HashMap;
use std::fmt;

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

#[derive(Debug, Clone)]
pub enum ScopedWsEvent {
    SocketConnected { context: SubscriptionContext },
    InitialSubscription {
        context: SubscriptionContext,
        payload: InitialSubscriptionPayload,
    },
    TransactionUpdate {
        context: SubscriptionContext,
        payload: TransactionUpdatePayload,
    },
    IdentityToken {
        context: SubscriptionContext,
        payload: IdentityTokenPayload,
    },
    Disconnected {
        context: SubscriptionContext,
        reason: String,
    },
    Reconnecting {
        context: SubscriptionContext,
        attempt: u32,
        delay: std::time::Duration,
    },
}
```

- [ ] **Step 5 GREEN: Add complete array seeds for inserts, deletes, and tables**

Continue `src/api/ws_decode.rs`:

```rust
struct JsonValuesSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for JsonValuesSeed<'_> {
    type Value = Vec<Value>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(JsonValuesVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct JsonValuesVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> Visitor<'de> for JsonValuesVisitor<'_> {
    type Value = Vec<Value>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an inserts or deletes array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<Value>()? {
            self.meter
                .accept_json_value::<A::Error>(&value, self.budgets)?;
            values.push(value);
        }
        Ok(values)
    }
}

fn decode_values(
    raw: &RawValue,
    meter: &mut DecodeMeter,
    budgets: ResourceBudgets,
) -> Result<Vec<Value>, serde_json::Error> {
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let values = JsonValuesSeed { meter, budgets }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(values)
}

struct TableUpdateSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for TableUpdateSeed<'_> {
    type Value = TableUpdate;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(TableUpdateVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct TableUpdateVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> Visitor<'de> for TableUpdateVisitor<'_> {
    type Value = TableUpdate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a table update object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut table_id = None;
        let mut table_name = None;
        let mut num_rows = 0;
        let mut inserts_raw: Option<&'de RawValue> = None;
        let mut deletes_raw: Option<&'de RawValue> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "table_id" => table_id = Some(map.next_value()?),
                "table_name" => table_name = Some(map.next_value()?),
                "num_rows" => num_rows = map.next_value()?,
                "inserts" => inserts_raw = Some(map.next_value()?),
                "deletes" => deletes_raw = Some(map.next_value()?),
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        let inserts = match inserts_raw {
            Some(raw) => decode_values(raw, self.meter, self.budgets)
                .map_err(de::Error::custom)?,
            None => Vec::new(),
        };
        let deletes = match deletes_raw {
            Some(raw) => decode_values(raw, self.meter, self.budgets)
                .map_err(de::Error::custom)?,
            None => Vec::new(),
        };
        Ok(TableUpdate {
            table_id: table_id.ok_or_else(|| de::Error::missing_field("table_id"))?,
            table_name: table_name
                .ok_or_else(|| de::Error::missing_field("table_name"))?,
            num_rows,
            inserts,
            deletes,
        })
    }
}

struct TablesSeed<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> DeserializeSeed<'de> for TablesSeed<'_> {
    type Value = Vec<TableUpdate>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(TablesVisitor {
            meter: self.meter,
            budgets: self.budgets,
        })
    }
}

struct TablesVisitor<'a> {
    meter: &'a mut DecodeMeter,
    budgets: ResourceBudgets,
}

impl<'de> Visitor<'de> for TablesVisitor<'_> {
    type Value = Vec<TableUpdate>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a tables array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut tables = Vec::new();
        while let Some(table) = sequence.next_element_seed(TableUpdateSeed {
            meter: self.meter,
            budgets: self.budgets,
        })? {
            tables.push(table);
        }
        Ok(tables)
    }
}

fn decode_database_update(
    raw: &RawValue,
    meter: &mut DecodeMeter,
    budgets: ResourceBudgets,
) -> Result<DatabaseUpdate, serde_json::Error> {
    struct DatabaseUpdateSeed<'a> {
        meter: &'a mut DecodeMeter,
        budgets: ResourceBudgets,
    }
    impl<'de> DeserializeSeed<'de> for DatabaseUpdateSeed<'_> {
        type Value = DatabaseUpdate;
        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct DatabaseUpdateVisitor<'a> {
                meter: &'a mut DecodeMeter,
                budgets: ResourceBudgets,
            }
            impl<'de> Visitor<'de> for DatabaseUpdateVisitor<'_> {
                type Value = DatabaseUpdate;
                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a database update object")
                }
                fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                where
                    A: MapAccess<'de>,
                {
                    let mut tables_raw: Option<&'de RawValue> = None;
                    while let Some(key) = map.next_key::<String>()? {
                        if key == "tables" {
                            tables_raw = Some(map.next_value()?);
                        } else {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                    let tables = match tables_raw {
                        Some(raw) => {
                            let mut deserializer = serde_json::Deserializer::from_str(raw.get());
                            let tables = TablesSeed {
                                meter: self.meter,
                                budgets: self.budgets,
                            }
                            .deserialize(&mut deserializer)
                            .map_err(de::Error::custom)?;
                            deserializer.end().map_err(de::Error::custom)?;
                            tables
                        }
                        None => Vec::new(),
                    };
                    Ok(DatabaseUpdate { tables })
                }
            }
            deserializer.deserialize_map(DatabaseUpdateVisitor {
                meter: self.meter,
                budgets: self.budgets,
            })
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let update = DatabaseUpdateSeed { meter, budgets }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(update)
}
```

- [ ] **Step 6 GREEN: Add complete externally tagged envelope and payload visitors**

Continue `src/api/ws_decode.rs` with visitors that store `database_update` as `&RawValue`, then call `decode_database_update`:

```rust
fn decode_initial_payload(
    raw: &RawValue,
    meter: &mut DecodeMeter,
    budgets: ResourceBudgets,
) -> Result<InitialSubscriptionPayload, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct RawInitial<'a> {
        #[serde(default, borrow)]
        database_update: Option<&'a RawValue>,
        #[serde(default)]
        request_id: u32,
        #[serde(default)]
        total_host_execution_duration: Option<Value>,
    }
    let wire: RawInitial<'_> = serde_json::from_str(raw.get())?;
    let database_update = match wire.database_update {
        Some(update) => decode_database_update(update, meter, budgets)?,
        None => DatabaseUpdate::default(),
    };
    Ok(InitialSubscriptionPayload {
        database_update,
        request_id: wire.request_id,
        total_host_execution_duration: wire.total_host_execution_duration,
    })
}

fn decode_transaction_payload(
    raw: &RawValue,
    meter: &mut DecodeMeter,
    budgets: ResourceBudgets,
) -> Result<TransactionUpdatePayload, serde_json::Error> {
    struct TransactionSeed<'a> {
        meter: &'a mut DecodeMeter,
        budgets: ResourceBudgets,
    }
    impl<'de> DeserializeSeed<'de> for TransactionSeed<'_> {
        type Value = TransactionUpdatePayload;
        fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            struct TransactionVisitor<'a> {
                meter: &'a mut DecodeMeter,
                budgets: ResourceBudgets,
            }
            impl<'de> Visitor<'de> for TransactionVisitor<'_> {
                type Value = TransactionUpdatePayload;
                fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                    formatter.write_str("a transaction update payload")
                }
                fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                where
                    A: MapAccess<'de>,
                {
                    let mut status: Option<TransactionStatus> = None;
                    let mut database_raw: Option<&'de RawValue> = None;
                    let mut extra = HashMap::new();
                    while let Some(key) = map.next_key::<String>()? {
                        match key.as_str() {
                            "status" => status = map.next_value()?,
                            "database_update" => database_raw = Some(map.next_value()?),
                            _ => {
                                let value = map.next_value::<Value>()?;
                                extra.insert(key, value);
                            }
                        }
                    }
                    let database_update = match database_raw {
                        Some(raw) => decode_database_update(raw, self.meter, self.budgets)
                            .map_err(de::Error::custom)?,
                        None => DatabaseUpdate::default(),
                    };
                    Ok(TransactionUpdatePayload {
                        status,
                        database_update,
                        extra,
                    })
                }
            }
            deserializer.deserialize_map(TransactionVisitor {
                meter: self.meter,
                budgets: self.budgets,
            })
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(raw.get());
    let payload = TransactionSeed { meter, budgets }.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(payload)
}

pub fn decode_subscription_text_bounded(
    text: &str,
    context: &SubscriptionContext,
    budgets: ResourceBudgets,
) -> Result<ScopedWsEvent, DecodeError> {
    let mut meter = DecodeMeter::default();
    let decoded: Result<ScopedWsEvent, serde_json::Error> = (|| {
        let envelope: HashMap<String, &RawValue> = serde_json::from_str(text)?;
        if envelope.len() != 1 {
            return Err(serde::de::Error::custom(
                "WebSocket message must contain one external variant",
            ));
        }
        let (variant, raw) = envelope.into_iter().next().ok_or_else(|| {
            serde::de::Error::custom("WebSocket message must contain one external variant")
        })?;
        match variant.as_str() {
            "InitialSubscription" => Ok(ScopedWsEvent::InitialSubscription {
                context: context.clone(),
                payload: decode_initial_payload(raw, &mut meter, budgets)?,
            }),
            "TransactionUpdate" => Ok(ScopedWsEvent::TransactionUpdate {
                context: context.clone(),
                payload: decode_transaction_payload(raw, &mut meter, budgets)?,
            }),
            "IdentityToken" => Ok(ScopedWsEvent::IdentityToken {
                context: context.clone(),
                payload: serde_json::from_str::<IdentityTokenPayload>(raw.get())?,
            }),
            _ => Err(serde::de::Error::custom(format!(
                "unknown WebSocket variant {variant}"
            ))),
        }
    })();
    match decoded {
        Ok(event) => Ok(event),
        Err(error) => match meter.take_exceeded() {
            Some(exceeded) => Err(DecodeError::Budget(exceeded)),
            None => Err(DecodeError::Protocol(error)),
        },
    }
}
```

The outer `HashMap` contains exactly one external variant and is bounded by the connect-time message cap. No `DatabaseUpdate`, tables array, inserts array, or deletes array is deserialized as a whole before the row and byte meter accepts each element. `DecodeMeter.exceeded` remains private to `query_decode`; both HTTP and sibling `ws_decode` consume typed budget state only through `DecodeMeter::take_exceeded`, so malformed JSON remains `DecodeError::Protocol` and budget rejection remains `DecodeError::Budget` on both paths.

- [ ] **Step 7 GREEN: Wire text frames to bounded decode**

Export the module from `src/api/mod.rs`:

```rust
pub mod ws_decode;
```

Replace the subscription text branch in `decode_subscription_frame` so it receives `&SubscriptionContext` and `ResourceBudgets` and calls `decode_subscription_text_bounded`. A `DecodeError::Budget` ends the socket attempt and returns a typed budget exit in Task 6. A `DecodeError::Protocol` emits one concise `WsEvent::Error(error.to_string())` and does not relabel it as arithmetic overflow.

- [ ] **Step 8 GREEN: Run focused tests**

```bash
cargo test websocket_config_applies_frame_and_reassembled_message_limits -- --nocapture
cargo test ws_decoder_preserves_external_variants_and_exact_payload_fields -- --nocapture
cargo test ws_rows_are_streamed_and_rejected_before_push -- --nocapture
cargo test malformed_ws_payload_is_protocol_error -- --nocapture
```

Expected: PASS.

- [ ] **Step 9 Commit**

```bash
git add src/api/mod.rs src/api/ws_decode.rs src/api/ws.rs
git commit -m "feat: bound websocket transport and streaming decode"
```

---

### Task 5: Add pure reconciliation over a fully defined metadata view

**Files:**
- Create: `src/effects/subscription.rs`
- Modify: `src/effects/mod.rs`

- [ ] **Step 1 RED: Add pure tests with no socket handle**

Create `src/effects/subscription.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn spec(table: &str) -> SubscriptionSpec {
        SubscriptionSpec {
            key: SubscriptionKey {
                database: "db".to_string(),
                table: table.to_string(),
            },
            query_strings: vec![format!("SELECT * FROM \"{table}\" LIMIT 200")],
            expected_bound: Some(200),
        }
    }

    fn metadata<'a>(
        spec: &'a SubscriptionSpec,
        generation: u64,
        phase: LivePhase,
    ) -> OwnedSubscriptionMetadata<'a> {
        OwnedSubscriptionMetadata {
            spec,
            task_generation: TaskGeneration(generation),
            phase,
        }
    }

    #[test]
    fn pure_reconcile_covers_none_same_and_replacement_without_a_ws_handle() {
        let users = spec("users");
        let orders = spec("orders");

        assert_eq!(reconcile(None, None), ReconcileAction::Keep);
        assert_eq!(
            reconcile(None, Some(metadata(&users, 7, LivePhase::Active))),
            ReconcileAction::CloseAndJoin {
                task_generation: TaskGeneration(7),
            }
        );
        assert_eq!(
            reconcile(Some(&users), None),
            ReconcileAction::Spawn {
                spec: users.clone(),
            }
        );
        assert_eq!(
            reconcile(
                Some(&users),
                Some(metadata(&users, 7, LivePhase::Active)),
            ),
            ReconcileAction::Keep
        );
        assert_eq!(
            reconcile(
                Some(&orders),
                Some(metadata(&users, 7, LivePhase::Active)),
            ),
            ReconcileAction::CloseAndJoin {
                task_generation: TaskGeneration(7),
            }
        );
    }

    #[test]
    fn ack_and_update_acceptance_require_the_current_active_context() {
        let current = SubscriptionContext {
            key: spec("users").key,
            task_generation: TaskGeneration(7),
            connection_generation: ConnectionGeneration(9),
            request_id: 42,
        };
        assert_eq!(phase_after_initial_ack(&current, 42), LivePhase::Active);
        assert_eq!(phase_after_initial_ack(&current, 41), LivePhase::Connecting);
        assert!(!context_matches_current(&current, &current, LivePhase::Connecting));
        assert!(context_matches_current(&current, &current, LivePhase::Active));

        let old_socket = SubscriptionContext {
            connection_generation: ConnectionGeneration(8),
            ..current.clone()
        };
        assert!(!context_matches_current(
            &old_socket,
            &current,
            LivePhase::Active,
        ));
    }
}
```

Run: `cargo test pure_reconcile_covers_none_same_and_replacement_without_a_ws_handle -- --nocapture`

Expected: FAIL because the module does not exist.

- [ ] **Step 2 GREEN: Implement metadata-only reconciliation**

Create the production portion of `src/effects/subscription.rs`:

```rust
pub use crate::api::ws_decode::{
    SubscriptionContext, SubscriptionKey, SubscriptionSpec,
};
use crate::effects::ports::{ConnectionGeneration, TaskGeneration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhase {
    Off,
    Connecting,
    Active,
    Reconnecting,
    Paused,
    Stale,
    Error,
    Closing,
}

#[derive(Debug, Clone, Copy)]
pub struct OwnedSubscriptionMetadata<'a> {
    pub spec: &'a SubscriptionSpec,
    pub task_generation: TaskGeneration,
    pub phase: LivePhase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    Keep,
    CloseAndJoin { task_generation: TaskGeneration },
    Spawn { spec: SubscriptionSpec },
}

pub fn reconcile(
    desired: Option<&SubscriptionSpec>,
    owned: Option<OwnedSubscriptionMetadata<'_>>,
) -> ReconcileAction {
    match (desired, owned) {
        (None, None) => ReconcileAction::Keep,
        (None, Some(task)) => ReconcileAction::CloseAndJoin {
            task_generation: task.task_generation,
        },
        (Some(spec), None) => ReconcileAction::Spawn { spec: spec.clone() },
        (Some(desired), Some(task)) if desired == task.spec => ReconcileAction::Keep,
        (Some(_), Some(task)) => ReconcileAction::CloseAndJoin {
            task_generation: task.task_generation,
        },
    }
}

pub fn phase_after_initial_ack(
    context: &SubscriptionContext,
    request_id: u32,
) -> LivePhase {
    if context.request_id == request_id {
        LivePhase::Active
    } else {
        LivePhase::Connecting
    }
}

pub fn context_matches_current(
    event: &SubscriptionContext,
    current: &SubscriptionContext,
    phase: LivePhase,
) -> bool {
    phase == LivePhase::Active
        && event.key == current.key
        && event.task_generation == current.task_generation
        && event.connection_generation == current.connection_generation
        && event.request_id == current.request_id
}
```

Export it from `src/effects/mod.rs`:

```rust
pub mod subscription;
```

Task 5 does not define `OwnedSubscriptionTask`; that real handle-owning type is introduced in Task 7 after `WsHandle::close_and_join` exists.

- [ ] **Step 3 GREEN: Run tests**

Run: `cargo test effects::subscription -- --nocapture`

Expected: PASS.

- [ ] **Step 4 Commit**

```bash
git add src/effects/mod.rs src/effects/subscription.rs
git commit -m "feat: add pure subscription reconciliation"
```

---

### Task 6: Give `WsHandle` exact close/join outcomes and fresh reconnect IDs

**Files:**
- Modify: `src/api/ws.rs`
- Modify: `src/effects/ports.rs`

- [ ] **Step 1 RED: Add close/join and fresh context tests**

Add to `src/api/ws.rs` tests:

```rust
#[tokio::test]
async fn ws_join_maps_to_exact_phase1_task_outcome() {
    assert_eq!(
        join_task_outcome(tokio::spawn(async {})).await,
        TaskOutcome::Completed
    );

    let cancelled = tokio::spawn(std::future::pending::<()>());
    cancelled.abort();
    assert_eq!(join_task_outcome(cancelled).await, TaskOutcome::Cancelled);

    let panicked = join_task_outcome(tokio::spawn(async {
        panic!("boom");
    })).await;
    assert!(matches!(panicked, TaskOutcome::Panicked { message } if message.contains("boom")));
}

#[tokio::test]
async fn each_socket_attempt_uses_fresh_connection_and_request_ids() {
    let ids = shared_id_source(FakeIdSource::new(40));
    let first = next_socket_ids(&ids).await.unwrap();
    let second = next_socket_ids(&ids).await.unwrap();

    assert_eq!(first, (ConnectionGeneration(41), 42));
    assert_eq!(second, (ConnectionGeneration(43), 44));
}

#[test]
fn subscribe_builder_sends_the_one_query_set_once() {
    let json = build_subscribe_json(
        &["SELECT * FROM \"users\" LIMIT 200".to_string()],
        42,
    ).unwrap();

    assert_eq!(json.matches("SELECT * FROM").count(), 1);
    assert!(json.contains("\"request_id\":42"));
}
```

Run: `cargo test each_socket_attempt_uses_fresh_connection_and_request_ids -- --nocapture`

Expected: FAIL because the shared exact `IdSource` owner and retained join handle are missing.

- [ ] **Step 2 GREEN: Add a shared wrapper around one Phase 2 `IdSource` instance**

Add to `src/effects/ports.rs`:

```rust
pub type SharedIdSource =
    std::sync::Arc<tokio::sync::Mutex<Box<dyn IdSource + Send>>>;

pub fn shared_id_source<I>(source: I) -> SharedIdSource
where
    I: IdSource + Send + 'static,
{
    std::sync::Arc::new(tokio::sync::Mutex::new(Box::new(source)))
}
```

This is a shared wrapper around one exact Phase 2 trait object, not a second ID trait.

- [ ] **Step 3 GREEN: Retain joins and return the exact `TaskOutcome`**

Change `WsHandle` in `src/api/ws.rs` and define its one-shot terminal reason before later controller tasks use it:

```rust
#[derive(Debug, Clone)]
pub enum WsTaskExit {
    Closed,
    Lost { reason: String },
    DecodeBudgetExceeded {
        context: SubscriptionContext,
        exceeded: BudgetExceeded,
    },
    QueueBudgetExceeded {
        context: SubscriptionContext,
        capacity: usize,
    },
}

pub struct WsHandle {
    cmd_tx: mpsc::Sender<WsCommand>,
    pub event_rx: mpsc::Receiver<WsEvent>,
    join_handle: tokio::task::JoinHandle<()>,
    exit_rx: tokio::sync::oneshot::Receiver<WsTaskExit>,
}
```

At each spawn, create `let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();`, run the subscription future to a `WsTaskExit`, send it once through `exit_tx`, and retain both `join_handle` and `exit_rx` in `WsHandle`. Store the returned join handle in both `spawn_subscription` and `spawn_log_follow`. Add:

```rust
async fn join_task_outcome(
    handle: tokio::task::JoinHandle<()>,
) -> crate::effects::task_registry::TaskOutcome {
    use crate::effects::task_registry::TaskOutcome;
    match handle.await {
        Ok(()) => TaskOutcome::Completed,
        Err(error) if error.is_cancelled() => TaskOutcome::Cancelled,
        Err(error) => TaskOutcome::Panicked {
            message: error.to_string(),
        },
    }
}

impl WsHandle {
    pub async fn close_and_join(self) -> crate::effects::task_registry::TaskOutcome {
        let WsHandle {
            cmd_tx,
            join_handle,
            exit_rx,
            ..
        } = self;
        let _ = cmd_tx.send(WsCommand::Close).await;
        let outcome = join_task_outcome(join_handle).await;
        let _ = exit_rx.await;
        outcome
    }

    pub async fn join_with_exit(
        self,
    ) -> (
        crate::effects::task_registry::TaskOutcome,
        WsTaskExit,
    ) {
        let WsHandle {
            join_handle,
            exit_rx,
            ..
        } = self;
        let outcome = join_task_outcome(join_handle).await;
        let exit = exit_rx.await.unwrap_or(WsTaskExit::Lost {
            reason: "subscription task ended without an exit reason".into(),
        });
        (outcome, exit)
    }
}
```

Do not add another task outcome type.

- [ ] **Step 4 GREEN: Allocate a fresh pair at the start of every connect attempt**

Add:

```rust
async fn next_socket_ids(
    ids: &crate::effects::ports::SharedIdSource,
) -> Result<
    (crate::effects::ports::ConnectionGeneration, u32),
    crate::effects::ports::SubscriptionRequestIdOverflow,
> {
    use crate::effects::ports::IdSourcePhase4Ext;
    let mut ids = ids.lock().await;
    let connection_generation = ids.next_connection_generation();
    let request_id = ids.next_subscription_request_id()?;
    Ok((connection_generation, request_id))
}

pub(crate) fn build_subscribe_json(
    queries: &[String],
    request_id: u32,
) -> anyhow::Result<String> {
    serde_json::to_string(&SubscribeEnvelope::new(queries.to_vec(), request_id))
        .map_err(Into::into)
}
```

Replace the current subscription entry point with this complete signature and wrapper:

```rust
pub fn spawn_subscription(
    config: WsConfig,
    spec: SubscriptionSpec,
    task_generation: TaskGeneration,
    ids: SharedIdSource,
) -> anyhow::Result<WsHandle> {
    let url = config.subscription_url()?;
    let (cmd_tx, cmd_rx) = mpsc::channel::<WsCommand>(32);
    let (event_tx, event_rx) = mpsc::channel::<WsEvent>(config.channel_capacity);
    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    let auth_token = config.auth_token.clone();
    let budgets = config.budgets;
    let join_handle = tokio::spawn(async move {
        let exit = subscription_task(
            url,
            auth_token,
            cmd_rx,
            event_tx,
            spec,
            task_generation,
            ids,
            budgets,
        )
        .await;
        let _ = exit_tx.send(exit);
    });
    Ok(WsHandle {
        cmd_tx,
        event_rx,
        join_handle,
        exit_rx,
    })
}
```

Change the existing function signature to `async fn subscription_task(url: url::Url, auth_token: Option<String>, mut cmd_rx: mpsc::Receiver<WsCommand>, event_tx: mpsc::Sender<WsEvent>, spec: SubscriptionSpec, task_generation: TaskGeneration, ids: SharedIdSource, budgets: ResourceBudgets) -> WsTaskExit`.

Keep its existing retry loop and replace every terminal branch exhaustively: a received `WsCommand::Close` or dropped command channel returns `WsTaskExit::Closed`; a permanent handshake refusal or exhausted transient reconnect returns `WsTaskExit::Lost { reason }`; `DecodeError::Budget(exceeded)` returns `WsTaskExit::DecodeBudgetExceeded { context, exceeded }`; Task 10 adds the already-declared queue-full exit. The loop cannot use a bare `return` and cannot fall through without a `WsTaskExit`.

At the top of every initial connect or reconnect attempt:

```rust
let (connection_generation, request_id) = next_socket_ids(&ids)
    .await
    .map_err(|error| ConnectOutcome::Fatal(error.to_string()))?;
let context = SubscriptionContext {
    key: spec.key.clone(),
    task_generation,
    connection_generation,
    request_id,
};
```

After the socket opens, send exactly one `build_subscribe_json(&spec.query_strings, request_id)` frame before reading updates. Remove `last_subscription: Option<(Vec<String>, u32)>`; reusing its old request ID would violate the approved reconnect contract.

- [ ] **Step 5 GREEN: Run focused tests**

```bash
cargo test ws_join_maps_to_exact_phase1_task_outcome -- --nocapture
cargo test each_socket_attempt_uses_fresh_connection_and_request_ids -- --nocapture
cargo test subscribe_builder_sends_the_one_query_set_once -- --nocapture
```

Expected: PASS.

- [ ] **Step 6 Commit**

```bash
git add src/api/ws.rs src/effects/ports.rs
git commit -m "feat: join websocket tasks with fresh reconnect ids"
```

---

### Task 7: Own one real subscription task in the effect runner and integrate exact events

**Files:**
- Modify: `src/effects/subscription.rs`
- Modify: `src/effects/runner.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/state/resources.rs`
- Modify: `src/state/activity.rs`
- Modify: `src/app.rs`

- [ ] **Step 1 RED: Add controller shape, barrier sequence, and reducer tests**

Add to `src/effects/subscription.rs` tests:

```rust
#[test]
fn controller_has_exact_desired_and_task_fields() {
    let controller = SubscriptionController::new();
    let _: &Option<SubscriptionSpec> = &controller.desired;
    let _: &Option<OwnedSubscriptionTask> = &controller.task;
    assert!(controller.desired.is_none());
    assert!(controller.task.is_none());
}

#[test]
fn replacement_barrier_sequence_allocates_generation_only_after_clear() {
    let users = SubscriptionSpec {
        key: SubscriptionKey { database: "db".into(), table: "users".into() },
        query_strings: vec!["SELECT * FROM \"users\" LIMIT 200".into()],
        expected_bound: Some(200),
    };
    let orders = SubscriptionSpec {
        key: SubscriptionKey { database: "db".into(), table: "orders".into() },
        query_strings: vec!["SELECT * FROM \"orders\" LIMIT 200".into()],
        expected_bound: Some(200),
    };
    let mut controller = SubscriptionController {
        desired: Some(orders.clone()),
        task: Some(OwnedSubscriptionTask {
            spec: users,
            task_generation: TaskGeneration(7),
            context: None,
            phase: LivePhase::Closing,
            handle: None,
        }),
    };
    let mut activity = ActivityState::default();
    let mut ids = FakeIdSource::new(7);
    let mut sequence = vec!["CloseSent", "JoinCompleted"];

    activity.record_subscription_join(
        &SubscriptionKey { database: "db".into(), table: "users".into() },
        &TaskOutcome::Completed,
    );
    sequence.push("ActivityRecorded");
    controller.finish_join(TaskGeneration(7));
    sequence.push("TaskCleared");
    let action = controller.reconcile();
    let ReconcileAction::Spawn { spec } = action else {
        panic!("expected spawn after clear");
    };
    let generation = ids.next_task_generation();
    sequence.push("SpawnNew");

    assert_eq!(sequence, vec![
        "CloseSent",
        "JoinCompleted",
        "ActivityRecorded",
        "TaskCleared",
        "SpawnNew",
    ]);
    assert_eq!(generation, TaskGeneration(8));
    assert_eq!(spec, orders);
}
```

Add to `src/app/reducer.rs` tests:

```rust
#[test]
fn stable_socket_connection_resets_singular_retry_and_keeps_connected_unit_variant() {
    let mut state = AppState::new("http://localhost:3000".to_string());
    state.activity.connection = ConnectionState::Reconnecting;
    state.activity.retry = RetryState {
        attempt: 3,
        next_retry_label: Some("in 4s".into()),
    };
    let context = SubscriptionContext {
        key: SubscriptionKey { database: "db".into(), table: "users".into() },
        task_generation: TaskGeneration(7),
        connection_generation: ConnectionGeneration(9),
        request_id: 42,
    };

    reduce(&mut state, AppEvent::SubscriptionSocketConnected { context });

    assert_eq!(state.activity.connection, ConnectionState::Connected);
    assert_eq!(state.activity.retry.attempt, 0);
    assert_eq!(state.activity.retry.next_retry_label, None);
}

fn live_context(table: &str) -> SubscriptionContext {
    SubscriptionContext {
        key: SubscriptionKey { database: "db".into(), table: table.into() },
        task_generation: TaskGeneration(7),
        connection_generation: ConnectionGeneration(9),
        request_id: 42,
    }
}

fn live_query_result(rows: Vec<Vec<serde_json::Value>>) -> QueryResult {
    QueryResult {
        schema: vec![
            SchemaElement {
                name: "id".into(),
                algebraic_type: serde_json::json!({"U64": null}),
            },
            SchemaElement {
                name: "name".into(),
                algebraic_type: serde_json::json!({"String": null}),
            },
        ],
        rows,
        total_duration_micros: 0,
    }
}

fn live_update(
    table_name: &str,
    inserts: Vec<serde_json::Value>,
    deletes: Vec<serde_json::Value>,
) -> DatabaseUpdate {
    DatabaseUpdate {
        tables: vec![TableUpdate {
            table_id: 1,
            table_name: table_name.into(),
            num_rows: u64::try_from(inserts.len() + deletes.len()).unwrap_or(u64::MAX),
            inserts,
            deletes,
        }],
    }
}

#[test]
fn live_rows_accept_array_and_object_in_schema_order() {
    let schema = live_query_result(Vec::new()).schema;
    assert_eq!(
        normalize_wire_row(&schema, serde_json::json!([1, "Ada"])),
        Ok(vec![serde_json::json!(1), serde_json::json!("Ada")])
    );
    assert_eq!(
        normalize_wire_row(&schema, serde_json::json!({"name": "Bob", "id": 2})),
        Ok(vec![serde_json::json!(2), serde_json::json!("Bob")])
    );
}

#[test]
fn live_insert_and_delete_accept_array_and_object_rows_for_only_scoped_table() {
    let mut load = LoadState::Ready {
        data: live_query_result(vec![
            vec![serde_json::json!(1), serde_json::json!("Ada")],
            vec![serde_json::json!(2), serde_json::json!("Bob")],
        ]),
        refreshed_at: std::time::Instant::now(),
    };
    apply_database_update_to_current_rows(
        &mut load,
        &live_context("users"),
        live_update(
            "users",
            vec![
                serde_json::json!([3, "Cid"]),
                serde_json::json!({"name": "Di", "id": 4}),
            ],
            vec![
                serde_json::json!([1, "Ada"]),
                serde_json::json!({"id": 2, "name": "Bob"}),
            ],
        ),
    ).unwrap();
    apply_database_update_to_current_rows(
        &mut load,
        &live_context("users"),
        live_update("orders", vec![serde_json::json!([99, "wrong table"])], Vec::new()),
    ).unwrap();

    let LoadState::Ready { data, .. } = load else {
        panic!("expected ready rows");
    };
    assert_eq!(data.rows, vec![
        vec![serde_json::json!(3), serde_json::json!("Cid")],
        vec![serde_json::json!(4), serde_json::json!("Di")],
    ]);
}

#[test]
fn malformed_live_row_is_typed_and_does_not_partially_mutate_rows() {
    let mut load = LoadState::Ready {
        data: live_query_result(vec![
            vec![serde_json::json!(1), serde_json::json!("Ada")],
        ]),
        refreshed_at: std::time::Instant::now(),
    };
    let before = match &load {
        LoadState::Ready { data, .. } => data.rows.clone(),
        _ => Vec::new(),
    };
    let error = apply_database_update_to_current_rows(
        &mut load,
        &live_context("users"),
        live_update(
            "users",
            vec![serde_json::json!(7)],
            vec![serde_json::json!([1, "Ada"])],
        ),
    ).unwrap_err();

    assert!(matches!(error, LiveApplyError::ScalarRow { kind: "number" }));
    let LoadState::Ready { data, .. } = load else {
        panic!("expected ready rows");
    };
    assert_eq!(data.rows, before);
    assert!(matches!(
        normalize_wire_row(
            &live_query_result(Vec::new()).schema,
            serde_json::json!({"id": 1}),
        ),
        Err(LiveApplyError::MissingObjectColumn { column }) if column == "name"
    ));
    assert!(matches!(
        normalize_wire_row(
            &live_query_result(Vec::new()).schema,
            serde_json::json!([1]),
        ),
        Err(LiveApplyError::ColumnCount { expected: 2, actual: 1 })
    ));
}

#[test]
fn live_apply_error_pauses_marks_stale_and_records_activity() {
    let mut state = AppState::new("http://localhost:3000".to_string());
    state.resources.table_rows = LoadState::Ready {
        data: live_query_result(vec![
            vec![serde_json::json!(1), serde_json::json!("Ada")],
        ]),
        refreshed_at: std::time::Instant::now(),
    };
    let context = live_context("users");

    reduce(
        &mut state,
        AppEvent::SubscriptionTransactionAccepted {
            context,
            payload: TransactionUpdatePayload {
                status: None,
                database_update: live_update("users", vec![serde_json::json!(false)], Vec::new()),
                extra: std::collections::HashMap::new(),
            },
            observed_at: std::time::Instant::now(),
        },
    );

    assert_eq!(state.resources.row_live.phase, LivePhase::Paused);
    assert!(matches!(state.resources.table_rows, LoadState::Stale { .. }));
    assert!(state.resources.row_live.detail.as_deref().is_some_and(|detail| {
        detail.contains("Live update rejected")
    }));
    assert!(matches!(
        state.activity.recent.last(),
        Some(ActivityItem { outcome: ActivityOutcome::Failed, .. })
    ));
}
```

Run: `cargo test replacement_barrier_sequence_allocates_generation_only_after_clear -- --nocapture`

Expected: FAIL because the real handle-owning controller and exact integration variants do not exist.

- [ ] **Step 2 GREEN: Add presentation-only row Live state**

Add to `src/state/resources.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowLiveState {
    pub phase: crate::effects::subscription::LivePhase,
    pub scope: Option<crate::api::ws_decode::SubscriptionKey>,
    pub context: Option<crate::api::ws_decode::SubscriptionContext>,
    pub last_event_at: Option<std::time::Instant>,
    pub last_event_label: Option<String>,
    pub detail: Option<String>,
}

impl Default for RowLiveState {
    fn default() -> Self {
        Self {
            phase: crate::effects::subscription::LivePhase::Off,
            scope: None,
            context: None,
            last_event_at: None,
            last_event_label: None,
            detail: None,
        }
    }
}
```

Add `pub row_live: RowLiveState` to the existing `ResourceState` and initialize it with `Default::default()`. It contains presentation data only. It does not contain `desired`, a handle, retry policy, or generation counter.

- [ ] **Step 3 GREEN: Define exact controller and actual owned task**

Add to `src/effects/subscription.rs`:

```rust
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

impl SubscriptionController {
    pub fn new() -> Self {
        Self {
            desired: None,
            task: None,
        }
    }

    pub fn set_desired(&mut self, desired: Option<SubscriptionSpec>) {
        self.desired = desired;
    }

    pub fn metadata(&self) -> Option<OwnedSubscriptionMetadata<'_>> {
        self.task.as_ref().map(|task| OwnedSubscriptionMetadata {
            spec: &task.spec,
            task_generation: task.task_generation,
            phase: task.phase,
        })
    }

    pub fn reconcile(&self) -> ReconcileAction {
        reconcile(self.desired.as_ref(), self.metadata())
    }

    pub fn accepts_event(&self, context: &SubscriptionContext) -> bool {
        let Some(task) = self.task.as_ref() else {
            return false;
        };
        let Some(current) = task.context.as_ref() else {
            return false;
        };
        context_matches_current(context, current, task.phase)
    }

    pub fn finish_join(&mut self, generation: TaskGeneration) -> bool {
        if self
            .task
            .as_ref()
            .is_some_and(|task| task.task_generation == generation)
        {
            self.task = None;
            true
        } else {
            false
        }
    }
}
```

No generation field is added to `SubscriptionController`.

- [ ] **Step 4 GREEN: Add exact `AppEvent` and `Effect` variants**

Extend the existing enums in `src/app/event.rs`:

```rust
// AppEvent additions
SetRowLiveIntent {
    desired: Option<SubscriptionSpec>,
},
SubscriptionTaskSpawned {
    key: SubscriptionKey,
    task_generation: TaskGeneration,
},
SubscriptionSocketConnected {
    context: SubscriptionContext,
},
SubscriptionReconnecting {
    context: SubscriptionContext,
    attempt: u32,
    delay: std::time::Duration,
},
SubscriptionInitialAccepted {
    context: SubscriptionContext,
    payload: crate::api::types::InitialSubscriptionPayload,
    observed_at: std::time::Instant,
},
SubscriptionTransactionAccepted {
    context: SubscriptionContext,
    payload: crate::api::types::TransactionUpdatePayload,
    observed_at: std::time::Instant,
},
SubscriptionIdentityToken {
    context: SubscriptionContext,
    payload: crate::api::types::IdentityTokenPayload,
},
SubscriptionTaskJoined {
    key: SubscriptionKey,
    task_generation: TaskGeneration,
    outcome: crate::effects::task_registry::TaskOutcome,
},
SubscriptionBudgetExceeded {
    context: SubscriptionContext,
    exceeded: crate::resource::budget::BudgetExceeded,
},

// Effect additions
SetDesiredSubscription {
    desired: Option<SubscriptionSpec>,
},
FinalizeSubscriptionJoin {
    task_generation: TaskGeneration,
},
```

Add the required imports from `api::ws_decode`, `effects::ports`, and `effects::subscription`. Keep existing variants unchanged.

- [ ] **Step 5 GREEN: Record Activity through the exact Phase 3 API**

Add methods to the existing `ActivityState` impl in `src/state/activity.rs`:

```rust
pub fn reset_after_stable_connection(&mut self) {
    self.connection = ConnectionState::Connected;
    self.retry.attempt = 0;
    self.retry.next_retry_label = None;
}

pub fn record_subscription_join(
    &mut self,
    key: &crate::api::ws_decode::SubscriptionKey,
    outcome: &crate::effects::task_registry::TaskOutcome,
) {
    let (activity_outcome, detail) = match outcome {
        crate::effects::task_registry::TaskOutcome::Completed => {
            (ActivityOutcome::Succeeded, None)
        }
        crate::effects::task_registry::TaskOutcome::Cancelled => {
            (ActivityOutcome::Cancelled, None)
        }
        crate::effects::task_registry::TaskOutcome::Panicked { message } => {
            (ActivityOutcome::Failed, Some(message.clone()))
        }
    };
    self.push_recent(ActivityItem {
        label: format!("Live {}.{} task joined", key.database, key.table),
        outcome: activity_outcome,
        detail,
    });
}
```

- [ ] **Step 6 GREEN: Make reducer update presentation and emit typed effects only**

Add these exact reducer arms:

```rust
AppEvent::SetRowLiveIntent { desired } => {
    let phase = if desired.is_some() { LivePhase::Connecting } else { LivePhase::Off };
    state.resources.row_live.phase = phase;
    state.resources.row_live.scope = desired.as_ref().map(|spec| spec.key.clone());
    Transition::effects(vec![Effect::SetDesiredSubscription { desired }])
}
AppEvent::SubscriptionTaskSpawned { key, .. } => {
    state.resources.row_live.phase = LivePhase::Connecting;
    state.resources.row_live.scope = Some(key);
    Transition::none()
}
AppEvent::SubscriptionSocketConnected { context } => {
    state.activity.reset_after_stable_connection();
    state.resources.row_live.phase = LivePhase::Connecting;
    state.resources.row_live.context = Some(context);
    Transition::none()
}
AppEvent::SubscriptionReconnecting { context, attempt, delay } => {
    state.activity.connection = ConnectionState::Reconnecting;
    state.activity.retry.attempt = attempt;
    state.activity.retry.next_retry_label = Some(format!(
        "retry {attempt} in {} ms",
        delay.as_millis(),
    ));
    state.resources.row_live.phase = LivePhase::Reconnecting;
    state.resources.row_live.context = Some(context);
    Transition::none()
}
AppEvent::SubscriptionInitialAccepted { context, payload, observed_at } => {
    state.resources.row_live.context = Some(context.clone());
    state.resources.row_live.last_event_at = Some(observed_at);
    match apply_database_update_to_current_rows(
        &mut state.resources.table_rows,
        &context,
        payload.database_update,
    ) {
        Ok(()) => {
            state.resources.row_live.phase = LivePhase::Active;
            state.resources.row_live.last_event_label = Some("initial snapshot".into());
        }
        Err(error) => record_live_apply_error(state, &context, error),
    }
    Transition::none()
}
AppEvent::SubscriptionTransactionAccepted { context, payload, observed_at } => {
    state.resources.row_live.context = Some(context.clone());
    state.resources.row_live.last_event_at = Some(observed_at);
    match apply_database_update_to_current_rows(
        &mut state.resources.table_rows,
        &context,
        payload.database_update,
    ) {
        Ok(()) => {
            state.resources.row_live.phase = LivePhase::Active;
            state.resources.row_live.last_event_label = Some("transaction update".into());
        }
        Err(error) => record_live_apply_error(state, &context, error),
    }
    Transition::none()
}
AppEvent::SubscriptionIdentityToken { context, payload: _ } => {
    state.resources.row_live.context = Some(context);
    state.activity.push_recent(ActivityItem {
        label: "WebSocket identity confirmed".into(),
        outcome: ActivityOutcome::Succeeded,
        detail: None,
    });
    Transition::none()
}
AppEvent::SubscriptionTaskJoined { key, task_generation, outcome } => {
    state.activity.record_subscription_join(&key, &outcome);
    Transition::effects(vec![Effect::FinalizeSubscriptionJoin { task_generation }])
}
AppEvent::SubscriptionBudgetExceeded { context, exceeded } => {
    state.resources.row_live.phase = LivePhase::Paused;
    state.resources.row_live.context = Some(context);
    state.resources.row_live.detail = Some(format!("Budget exceeded: {exceeded}"));
    mark_current_rows_stale(&mut state.resources.table_rows, StaleReason::BudgetExceeded);
    Transition::none()
}
```

Define the reducer-local helpers and typed apply error in the same task:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveApplyError {
    NoCurrentRows,
    DuplicateSchemaColumn { column: String },
    ColumnCount { expected: usize, actual: usize },
    MissingObjectColumn { column: String },
    UnexpectedObjectColumn { column: String },
    ScalarRow { kind: &'static str },
    DeleteRowNotFound,
    CapacityOverflow,
    AllocationFailed,
}

impl std::fmt::Display for LiveApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCurrentRows => write!(formatter, "no current rows are available"),
            Self::DuplicateSchemaColumn { column } => {
                write!(formatter, "schema contains duplicate column {column}")
            }
            Self::ColumnCount { expected, actual } => {
                write!(formatter, "row has {actual} columns; expected {expected}")
            }
            Self::MissingObjectColumn { column } => {
                write!(formatter, "object row is missing column {column}")
            }
            Self::UnexpectedObjectColumn { column } => {
                write!(formatter, "object row contains unexpected column {column}")
            }
            Self::ScalarRow { kind } => write!(formatter, "wire row is scalar {kind}"),
            Self::DeleteRowNotFound => write!(formatter, "delete row is not present"),
            Self::CapacityOverflow => write!(formatter, "row patch capacity overflow"),
            Self::AllocationFailed => write!(formatter, "row patch allocation failed"),
        }
    }
}

impl std::error::Error for LiveApplyError {}

fn scalar_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn normalize_wire_row(
    schema: &[crate::api::types::SchemaElement],
    value: serde_json::Value,
) -> Result<Vec<serde_json::Value>, LiveApplyError> {
    for (index, column) in schema.iter().enumerate() {
        if schema[..index]
            .iter()
            .any(|previous| previous.name == column.name)
        {
            return Err(LiveApplyError::DuplicateSchemaColumn {
                column: column.name.clone(),
            });
        }
    }
    match value {
        serde_json::Value::Array(row) => {
            if row.len() != schema.len() {
                return Err(LiveApplyError::ColumnCount {
                    expected: schema.len(),
                    actual: row.len(),
                });
            }
            Ok(row)
        }
        serde_json::Value::Object(mut object) => {
            let mut row = Vec::new();
            row.try_reserve_exact(schema.len())
                .map_err(|_| LiveApplyError::AllocationFailed)?;
            for column in schema {
                let value = object.remove(&column.name).ok_or_else(|| {
                    LiveApplyError::MissingObjectColumn {
                        column: column.name.clone(),
                    }
                })?;
                row.push(value);
            }
            if let Some((column, _)) = object.into_iter().next() {
                return Err(LiveApplyError::UnexpectedObjectColumn { column });
            }
            Ok(row)
        }
        scalar => Err(LiveApplyError::ScalarRow {
            kind: scalar_kind(&scalar),
        }),
    }
}

struct LiveRowPatch {
    deleted: Vec<bool>,
    inserts: Vec<Vec<serde_json::Value>>,
}

impl LiveRowPatch {
    fn apply(self, result: &mut crate::api::types::QueryResult) -> Result<(), LiveApplyError> {
        let deleted_count = self.deleted.iter().filter(|deleted| **deleted).count();
        let retained = result
            .rows
            .len()
            .checked_sub(deleted_count)
            .ok_or(LiveApplyError::CapacityOverflow)?;
        let final_len = retained
            .checked_add(self.inserts.len())
            .ok_or(LiveApplyError::CapacityOverflow)?;
        let mut next_rows = Vec::new();
        next_rows
            .try_reserve_exact(final_len)
            .map_err(|_| LiveApplyError::AllocationFailed)?;

        let current_rows = std::mem::take(&mut result.rows);
        for (row, deleted) in current_rows.into_iter().zip(self.deleted) {
            if !deleted {
                next_rows.push(row);
            }
        }
        next_rows.extend(self.inserts);
        result.rows = next_rows;
        Ok(())
    }
}

fn plan_database_update(
    result: &crate::api::types::QueryResult,
    context: &crate::api::ws_decode::SubscriptionContext,
    update: crate::api::types::DatabaseUpdate,
) -> Result<LiveRowPatch, LiveApplyError> {
    let mut deleted = Vec::new();
    deleted
        .try_reserve_exact(result.rows.len())
        .map_err(|_| LiveApplyError::AllocationFailed)?;
    deleted.resize(result.rows.len(), false);
    let mut inserts = Vec::new();

    for table in update
        .tables
        .into_iter()
        .filter(|table| table.table_name == context.key.table)
    {
        for delete in table.deletes {
            let delete = normalize_wire_row(&result.schema, delete)?;
            let position = result
                .rows
                .iter()
                .enumerate()
                .position(|(index, row)| !deleted[index] && row == &delete)
                .ok_or(LiveApplyError::DeleteRowNotFound)?;
            deleted[position] = true;
        }
        for insert in table.inserts {
            let insert = normalize_wire_row(&result.schema, insert)?;
            inserts
                .try_reserve(1)
                .map_err(|_| LiveApplyError::AllocationFailed)?;
            inserts.push(insert);
        }
    }

    Ok(LiveRowPatch { deleted, inserts })
}

fn mark_current_rows_stale(
    load: &mut LoadState<crate::api::types::QueryResult>,
    reason: StaleReason,
) {
    let replacement = match std::mem::replace(load, LoadState::Idle) {
        LoadState::Ready { data, .. }
        | LoadState::Refreshing { data, .. }
        | LoadState::Stale { data, .. } => LoadState::Stale { data, reason },
        other => other,
    };
    *load = replacement;
}

fn apply_database_update_to_current_rows(
    load: &mut LoadState<crate::api::types::QueryResult>,
    context: &crate::api::ws_decode::SubscriptionContext,
    update: crate::api::types::DatabaseUpdate,
) -> Result<(), LiveApplyError> {
    let result = match load {
        LoadState::Ready { data, .. }
        | LoadState::Refreshing { data, .. }
        | LoadState::Stale { data, .. } => data,
        _ => return Err(LiveApplyError::NoCurrentRows),
    };
    let patch = plan_database_update(result, context, update)?;
    patch.apply(result)
}

fn record_live_apply_error(
    state: &mut AppState,
    context: &crate::api::ws_decode::SubscriptionContext,
    error: LiveApplyError,
) {
    let detail = format!("Live update rejected: {error}");
    state.resources.row_live.phase = LivePhase::Paused;
    state.resources.row_live.context = Some(context.clone());
    state.resources.row_live.last_event_label = Some("rejected update".into());
    state.resources.row_live.detail = Some(detail.clone());
    mark_current_rows_stale(
        &mut state.resources.table_rows,
        StaleReason::ManualRefreshRequired,
    );
    state.activity.push_recent(ActivityItem {
        label: format!("Live {}.{} update rejected", context.key.database, context.key.table),
        outcome: ActivityOutcome::Failed,
        detail: Some(detail),
    });
}
```

All array/object/scalar validation, schema ordering, delete lookup, checked capacity arithmetic, and allocation reservation complete before `LiveRowPatch::apply` takes the old row vector. Only `TableUpdate` values whose `table_name` equals `SubscriptionContext.key.table` participate. Nonmatching table updates are ignored. After the first state mutation there is no fallible operation, so malformed rows, missing columns, incompatible column counts, and missing deletes leave both visible rows and later Task 9 cache admission unchanged. Task 9 must run its existing atomic `RowCache::admit` before publishing the successfully patched visible result; an admission error follows the same Paused, Stale, detail, and failed-Activity path.

- [ ] **Step 7 GREEN: Make the Phase 2 effect runner the controller owner**

Add the subscription controller, one shared ID source, and the resolved WS construction fields to the existing Phase 2 `EffectRunner<A, S, T>` beside its concrete `tasks: TaskRegistry` and bounded `event_sender: mpsc::Sender<AppEvent>`. Do not add generic parameters, another runner, or any of these owners to `AppState`:

```rust
pub subscription_controller: SubscriptionController,
pub subscription_ids: SharedIdSource,
pub ws_base_url: String,
pub ws_auth_token: Option<String>,
pub resource_budgets: ResourceBudgets,
```

Add the exact constructor used by the spawn branch:

```rust
fn ws_config_for(&self, database: &str) -> crate::api::ws::WsConfig {
    crate::api::ws::WsConfig {
        base_url: self.ws_base_url.clone(),
        database: database.to_string(),
        auth_token: self.ws_auth_token.clone(),
        channel_capacity: self.resource_budgets.event_queue_capacity,
        budgets: self.resource_budgets,
    }
}
```

Implement these subscription methods only on the existing `impl<A, S, T> EffectRunner<A, S, T>` and preserve the same `TaskRegistry` and bounded sender as the async ownership boundary. The close branch takes only the handle, leaving task metadata present and `Closing` until the reducer records Activity and emits finalization:

```rust
pub async fn dispatch_subscription_effect(
    &mut self,
    effect: Effect,
) -> anyhow::Result<Vec<AppEvent>> {
    match effect {
        Effect::SetDesiredSubscription { desired } => {
            self.subscription_controller.set_desired(desired);
            self.reconcile_subscription_once().await
        }
        Effect::FinalizeSubscriptionJoin { task_generation } => {
            self.subscription_controller.finish_join(task_generation);
            self.reconcile_subscription_once().await
        }
        _ => Ok(Vec::new()),
    }
}

async fn reconcile_subscription_once(&mut self) -> anyhow::Result<Vec<AppEvent>> {
    match self.subscription_controller.reconcile() {
        ReconcileAction::Keep => Ok(Vec::new()),
        ReconcileAction::CloseAndJoin { task_generation } => {
            let task = self
                .subscription_controller
                .task
                .as_mut()
                .ok_or_else(|| anyhow::anyhow!(
                    "reconcile requested close without an owned subscription task"
                ))?;
            task.phase = LivePhase::Closing;
            let key = task.spec.key.clone();
            let handle = task.handle.take().ok_or_else(|| anyhow::anyhow!(
                "reconcile requested close after the subscription handle was taken"
            ))?;
            let outcome = handle.close_and_join().await;
            Ok(vec![AppEvent::SubscriptionTaskJoined {
                key,
                task_generation,
                outcome,
            }])
        }
        ReconcileAction::Spawn { spec } => {
            let task_generation = {
                let mut ids = self.subscription_ids.lock().await;
                ids.next_task_generation()
            };
            let config = self.ws_config_for(&spec.key.database);
            let handle = crate::api::ws::spawn_subscription(
                config,
                spec.clone(),
                task_generation,
                self.subscription_ids.clone(),
            )?;
            self.subscription_controller.task = Some(OwnedSubscriptionTask {
                spec: spec.clone(),
                task_generation,
                context: None,
                phase: LivePhase::Connecting,
                handle: Some(handle),
            });
            Ok(vec![AppEvent::SubscriptionTaskSpawned {
                key: spec.key,
                task_generation,
            }])
        }
    }
}
```

Define `ws_config_for` on the runner by cloning its existing WS base URL, auth token, resolved budgets, and the supplied database into the Task 4 `WsConfig` fields. No default budget is constructed here.

The first replacement dispatch can emit only `SubscriptionTaskJoined`. The reducer records Activity before emitting `FinalizeSubscriptionJoin`. Only handling that later effect clears the task and obtains a new task generation. This enforces `CloseSent -> JoinCompleted -> ActivityRecorded -> TaskCleared -> SpawnNew` and prevents Close plus Spawn in one effect batch.

- [ ] **Step 8 GREEN: Filter acknowledgements and transaction updates in the effect runner**

Replace the old WebSocket drain match with this exact filtering function in the effect runner:

```rust
fn accept_scoped_ws_event(
    controller: &mut SubscriptionController,
    event: ScopedWsEvent,
    observed_at: std::time::Instant,
) -> Option<AppEvent> {
    match event {
        ScopedWsEvent::SocketConnected { context } => {
            let task = controller.task.as_mut()?;
            if task.task_generation != context.task_generation || task.spec.key != context.key {
                return None;
            }
            task.context = Some(context.clone());
            task.phase = LivePhase::Connecting;
            Some(AppEvent::SubscriptionSocketConnected { context })
        }
        ScopedWsEvent::Reconnecting { context, attempt, delay } => {
            let task = controller.task.as_mut()?;
            if task.task_generation != context.task_generation || task.spec.key != context.key {
                return None;
            }
            task.context = Some(context.clone());
            task.phase = LivePhase::Reconnecting;
            Some(AppEvent::SubscriptionReconnecting {
                context,
                attempt,
                delay,
            })
        }
        ScopedWsEvent::InitialSubscription { context, payload } => {
            let task = controller.task.as_mut()?;
            let current = task.context.as_ref()?;
            if current != &context || payload.request_id != context.request_id {
                return None;
            }
            task.phase = LivePhase::Active;
            Some(AppEvent::SubscriptionInitialAccepted {
                context,
                payload,
                observed_at,
            })
        }
        ScopedWsEvent::TransactionUpdate { context, payload } => {
            controller
                .accepts_event(&context)
                .then_some(AppEvent::SubscriptionTransactionAccepted {
                    context,
                    payload,
                    observed_at,
                })
        }
        ScopedWsEvent::IdentityToken { context, payload } => {
            let task = controller.task.as_ref()?;
            let current = task.context.as_ref()?;
            (current == &context).then_some(AppEvent::SubscriptionIdentityToken {
                context,
                payload,
            })
        }
        ScopedWsEvent::Disconnected { context, reason: _ } => {
            let task = controller.task.as_mut()?;
            if task.context.as_ref() != Some(&context) {
                return None;
            }
            task.phase = LivePhase::Reconnecting;
            Some(AppEvent::SubscriptionReconnecting {
                context,
                attempt: 1,
                delay: std::time::Duration::ZERO,
            })
        }
    }
}
```

Call `accept_scoped_ws_event(&mut self.subscription_controller, event, self.clock.now())`; this uses the existing Phase 2 `Clock` port for deterministic last-event time.

Remove the legacy all-table subscription call. Desired Live state is derived only from active database, active table, explicit Live intent, and Task 8 capability decision.

- [ ] **Step 9 GREEN: Run focused tests and forbidden-owner scans**

```bash
cargo test controller_has_exact_desired_and_task_fields -- --nocapture
cargo test replacement_barrier_sequence_allocates_generation_only_after_clear -- --nocapture
cargo test stable_socket_connection_resets_singular_retry_and_keeps_connected_unit_variant -- --nocapture
cargo test live_rows_accept_array_and_object_in_schema_order -- --nocapture
cargo test live_insert_and_delete_accept_array_and_object_rows_for_only_scoped_table -- --nocapture
cargo test malformed_live_row_is_typed_and_does_not_partially_mutate_rows -- --nocapture
cargo test live_apply_error_pauses_marks_stale_and_records_activity -- --nocapture
rg -n "ws_subscribe_all_tables|Vec<SubscriptionSpec>|HashMap<.*SubscriptionSpec" src/app.rs src/effects src/api/ws.rs
```

Expected: tests PASS and scan returns no multi-subscription owner or automatic all-table path.

- [ ] **Step 10 Commit**

```bash
git add src/effects/subscription.rs src/effects/runner.rs src/app/event.rs src/app/reducer.rs src/state/resources.rs src/state/activity.rs src/app.rs
git commit -m "feat: own one active-resource subscription task"
```

---

### Task 8: Add capability fallback and visible Live phases

**Files:**
- Modify: `src/effects/subscription.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/ui/tabs/live.rs`
- Modify: `src/ui/inspector.rs`

- [ ] **Step 1 RED: Add complete capability decision tests**

Add to `src/effects/subscription.rs` tests:

```rust
fn active_resource() -> ActiveResource {
    ActiveResource {
        database: "db".into(),
        table: "users".into(),
    }
}

#[test]
fn capability_decision_is_bounded_confirmed_or_unavailable() {
    let bounded = decide_live_capability(
        ServerLiveCapability::BoundedSubscription { max_rows: 200 },
        &active_resource(),
        UserLiveIntent::Enable,
    ).unwrap();
    assert_eq!(bounded.spec.unwrap().expected_bound, Some(200));
    assert!(!bounded.requires_confirmation);

    let warning = decide_live_capability(
        ServerLiveCapability::UnboundedOnly,
        &active_resource(),
        UserLiveIntent::Enable,
    ).unwrap();
    assert!(warning.spec.is_none());
    assert!(warning.requires_confirmation);

    let accepted = decide_live_capability(
        ServerLiveCapability::UnboundedOnly,
        &active_resource(),
        UserLiveIntent::AcceptUnboundedAfterWarning,
    ).unwrap();
    assert!(accepted.spec.is_some());
    assert_eq!(accepted.spec.unwrap().expected_bound, None);

    let unavailable = decide_live_capability(
        ServerLiveCapability::Unavailable,
        &active_resource(),
        UserLiveIntent::Enable,
    ).unwrap();
    assert!(unavailable.spec.is_none());
    assert_eq!(unavailable.message, "Row Live is unavailable because neither a bounded subscription nor safe unbounded transport is supported.");
}
```

- [ ] **Step 2 RED: Add a real `TestBackend` phase matrix**

Add to `src/ui/tabs/live.rs` tests:

```rust
fn render_live_text(model: &LiveViewModel) -> String {
    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|frame| {
        let area = frame.area();
        render_live_state(frame, area, model);
    }).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn every_visible_live_phase_has_scope_time_and_action_text() {
    let cases = [
        (LivePhase::Off, "Off", "Enable"),
        (LivePhase::Connecting, "Connecting", "Cancel"),
        (LivePhase::Active, "Live", "Pause"),
        (LivePhase::Reconnecting, "Connecting", "Retrying"),
        (LivePhase::Paused, "Paused", "Resume"),
        (LivePhase::Stale, "Stale", "Refresh"),
        (LivePhase::Error, "Error", "Retry"),
        (LivePhase::Closing, "Closing", "Waiting"),
    ];
    for (phase, label, action) in cases {
        let text = render_live_text(&LiveViewModel {
            phase,
            scope: "db.users".into(),
            last_event: "12:00:00".into(),
            action: action.into(),
            detail: None,
        });
        assert!(text.contains(label));
        assert!(text.contains("db.users"));
        assert!(text.contains("12:00:00"));
        assert!(text.contains(action));
    }
}
```

Run: `cargo test capability_decision_is_bounded_confirmed_or_unavailable -- --nocapture`

Expected: FAIL because the decision types do not exist.

- [ ] **Step 3 GREEN: Define exact capability types and query construction**

Add to `src/effects/subscription.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveResource {
    pub database: String,
    pub table: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerLiveCapability {
    BoundedSubscription { max_rows: usize },
    UnboundedOnly,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserLiveIntent {
    Enable,
    AcceptUnboundedAfterWarning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveCapabilityDecision {
    pub spec: Option<SubscriptionSpec>,
    pub requires_confirmation: bool,
    pub message: &'static str,
}

pub fn decide_live_capability(
    capability: ServerLiveCapability,
    active: &ActiveResource,
    intent: UserLiveIntent,
) -> Result<LiveCapabilityDecision, crate::effects::write_ops::SqlEncodingError> {
    let table = crate::effects::write_ops::encode_identifier(&active.table)?;
    let key = SubscriptionKey {
        database: active.database.clone(),
        table: active.table.clone(),
    };
    let decision = match (capability, intent) {
        (ServerLiveCapability::BoundedSubscription { max_rows }, _) => {
            LiveCapabilityDecision {
                spec: Some(SubscriptionSpec {
                    key,
                    query_strings: vec![format!("SELECT * FROM {table} LIMIT {max_rows}")],
                    expected_bound: Some(max_rows),
                }),
                requires_confirmation: false,
                message: "Bounded row Live is available.",
            }
        }
        (ServerLiveCapability::UnboundedOnly, UserLiveIntent::Enable) => {
            LiveCapabilityDecision {
                spec: None,
                requires_confirmation: true,
                message: "This server offers only an unbounded row subscription. Continue only with active transport and decode budgets.",
            }
        }
        (
            ServerLiveCapability::UnboundedOnly,
            UserLiveIntent::AcceptUnboundedAfterWarning,
        ) => LiveCapabilityDecision {
            spec: Some(SubscriptionSpec {
                key,
                query_strings: vec![format!("SELECT * FROM {table}")],
                expected_bound: None,
            }),
            requires_confirmation: false,
            message: "Unbounded row Live accepted with local budgets active.",
        },
        (ServerLiveCapability::Unavailable, _) => LiveCapabilityDecision {
            spec: None,
            requires_confirmation: false,
            message: "Row Live is unavailable because neither a bounded subscription nor safe unbounded transport is supported.",
        },
    };
    Ok(decision)
}
```

Database-wide transaction observation is not emulated. The Inspector text is exactly `Database-wide observation requires a native bounded feed.`

- [ ] **Step 4 GREEN: Define and render the Live view model**

Add to `src/ui/tabs/live.rs`:

```rust
pub struct LiveViewModel {
    pub phase: LivePhase,
    pub scope: String,
    pub last_event: String,
    pub action: String,
    pub detail: Option<String>,
}
```

Add the complete renderer beside `LiveViewModel`:

```rust
pub fn render_live_state(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    model: &LiveViewModel,
) {
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph};

    let phase = match model.phase {
        LivePhase::Off => "Off",
        LivePhase::Connecting => "Connecting",
        LivePhase::Active => "Live",
        LivePhase::Reconnecting => "Connecting",
        LivePhase::Paused => "Paused",
        LivePhase::Stale => "Stale",
        LivePhase::Error => "Error",
        LivePhase::Closing => "Closing",
    };
    let mut lines = vec![
        Line::from(format!("Status: {phase}")),
        Line::from(format!("Scope: {}", model.scope)),
        Line::from(format!("Last event: {}", model.last_event)),
        Line::from(format!("Action: {}", model.action)),
    ];
    if let Some(detail) = &model.detail {
        lines.push(Line::from(format!("Detail: {detail}")));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Live")),
        area,
    );
}
```

Add the complete state mapping used by the Live tab:

```rust
pub fn live_view_model(state: &crate::state::app_state::AppState) -> LiveViewModel {
    let live = &state.resources.row_live;
    let scope = live
        .scope
        .as_ref()
        .map(|key| format!("{}.{}", key.database, key.table))
        .unwrap_or_else(|| "no active resource".into());
    let last_event = live
        .last_event_at
        .map(|instant| format!("{instant:?}"))
        .unwrap_or_else(|| "none".into());
    let action = match live.phase {
        LivePhase::Off => "Enable",
        LivePhase::Connecting => "Cancel",
        LivePhase::Active => "Pause",
        LivePhase::Reconnecting => "Retrying",
        LivePhase::Paused => "Resume",
        LivePhase::Stale => "Refresh",
        LivePhase::Error => "Retry",
        LivePhase::Closing => "Waiting",
    }
    .to_string();
    let detail = if live.phase == LivePhase::Reconnecting {
        state
            .activity
            .retry
            .next_retry_label
            .clone()
            .or_else(|| live.detail.clone())
    } else {
        live.detail.clone()
    };
    LiveViewModel {
        phase: live.phase,
        scope,
        last_event,
        action,
        detail,
    }
}
```

The UI reads only presentation state. It does not read the controller or a socket handle.

- [ ] **Step 5 GREEN: Run focused tests**

```bash
cargo test capability_decision_is_bounded_confirmed_or_unavailable -- --nocapture
cargo test every_visible_live_phase_has_scope_time_and_action_text -- --nocapture
```

Expected: PASS.

- [ ] **Step 6 Commit**

```bash
git add src/effects/subscription.rs src/app/reducer.rs src/ui/tabs/live.rs src/ui/inspector.rs
git commit -m "feat: add live capability fallback and phases"
```

---

### Task 9: Add atomic global row-cache admission and deterministic inactive LRU

**Files:**
- Create: `src/state/row_cache.rs`
- Modify: `src/state/mod.rs`
- Modify: `src/state/app_state.rs`
- Modify: `src/ui/tabs/tables.rs`

- [ ] **Step 1 RED: Add complete cache fixtures and atomicity tests**

Create `src/state/row_cache.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::ports::Clock;
    use std::time::{Duration, Instant};

    struct StepClock {
        now: Instant,
    }

    impl StepClock {
        fn new(now: Instant) -> Self { Self { now } }
        fn advance(&mut self, duration: Duration) { self.now += duration; }
    }

    impl Clock for StepClock {
        fn now(&self) -> Instant { self.now }
    }

    fn key(resource: &str) -> CacheKey {
        CacheKey {
            database: "db".into(),
            resource: resource.into(),
            view: "browse".into(),
        }
    }

    fn result(rows: usize) -> QueryResult {
        QueryResult {
            schema: vec![SchemaElement {
                name: "id".into(),
                algebraic_type: serde_json::json!({"U64": null}),
            }],
            rows: (0..rows).map(|value| vec![serde_json::json!(value)]).collect(),
            total_duration_micros: 0,
        }
    }

    fn wide_result(bytes: usize) -> QueryResult {
        QueryResult {
            schema: vec![SchemaElement {
                name: "value".into(),
                algebraic_type: serde_json::json!({"String": null}),
            }],
            rows: vec![vec![serde_json::json!("x".repeat(bytes))]],
            total_duration_micros: 0,
        }
    }

    #[test]
    fn active_entry_is_never_evicted_and_inactive_lru_is_deterministic() {
        let budgets = ResourceBudgets {
            global_row_limit: 4,
            global_byte_limit: 1_000_000,
            ..ResourceBudgets::default()
        };
        let mut clock = StepClock::new(Instant::now());
        let mut cache = RowCache::new(budgets);
        let active = key("active");
        cache.admit(active.clone(), result(2), Some(&active), clock.now()).unwrap();
        clock.advance(Duration::from_secs(1));
        cache.admit(key("old"), result(2), Some(&active), clock.now()).unwrap();
        clock.advance(Duration::from_secs(1));
        cache.admit(key("new"), result(2), Some(&active), clock.now()).unwrap();

        assert!(cache.contains(&active));
        assert!(!cache.contains(&key("old")));
        assert!(cache.contains(&key("new")));
    }

    #[test]
    fn failed_replacement_is_fully_atomic() {
        let budgets = ResourceBudgets {
            global_row_limit: 100,
            global_byte_limit: 512,
            ..ResourceBudgets::default()
        };
        let clock = StepClock::new(Instant::now());
        let mut cache = RowCache::new(budgets);
        cache.admit(key("users"), result(1), None, clock.now()).unwrap();
        let before = cache.snapshot();

        assert!(matches!(
            cache.admit(key("users"), wide_result(1024), None, clock.now()),
            Err(BudgetExceeded::CacheBytes { .. })
        ));
        assert_eq!(cache.snapshot(), before);
    }

    #[test]
    fn one_view_cannot_bypass_global_row_limit() {
        let mut cache = RowCache::new(ResourceBudgets {
            global_row_limit: 1,
            global_byte_limit: 1_000_000,
            ..ResourceBudgets::default()
        });
        assert_eq!(
            cache.admit(key("users"), result(2), None, Instant::now()),
            Err(BudgetExceeded::CacheRows { limit: 1, requested: 2 })
        );
    }
}
```

Run: `cargo test failed_replacement_is_fully_atomic -- --nocapture`

Expected: FAIL because `RowCache` does not exist.

- [ ] **Step 2 GREEN: Implement staged checked admission**

Create `src/state/row_cache.rs`:

```rust
use crate::api::types::QueryResult;
use crate::resource::budget::{BudgetExceeded, ResourceBudgets};
use crate::resource::estimate::estimate_query_result_bytes;
use std::collections::HashMap;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey {
    pub database: String,
    pub resource: String,
    pub view: String,
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub result: QueryResult,
    pub rows: usize,
    pub estimated_bytes: usize,
    pub lru_sequence: u64,
    pub fetched_at: Instant,
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheSnapshot {
    pub entries: Vec<(CacheKey, usize, usize, u64)>,
    pub total_rows: usize,
    pub total_estimated_bytes: usize,
    pub next_lru_sequence: u64,
}

#[derive(Debug)]
pub struct RowCache {
    entries: HashMap<CacheKey, CacheEntry>,
    total_rows: usize,
    total_estimated_bytes: usize,
    next_lru_sequence: u64,
    budgets: ResourceBudgets,
}

impl RowCache {
    pub fn new(budgets: ResourceBudgets) -> Self {
        Self {
            entries: HashMap::new(),
            total_rows: 0,
            total_estimated_bytes: 0,
            next_lru_sequence: 1,
            budgets,
        }
    }

    pub fn admit(
        &mut self,
        key: CacheKey,
        result: QueryResult,
        active: Option<&CacheKey>,
        now: Instant,
    ) -> Result<(), BudgetExceeded> {
        let rows = result.rows.len();
        let estimated_bytes = estimate_query_result_bytes(&result)?;
        if rows > self.budgets.global_row_limit {
            return Err(BudgetExceeded::CacheRows {
                limit: self.budgets.global_row_limit,
                requested: rows,
            });
        }
        if estimated_bytes > self.budgets.global_byte_limit {
            return Err(BudgetExceeded::CacheBytes {
                limit: self.budgets.global_byte_limit,
                requested: estimated_bytes,
            });
        }

        let old_rows = self.entries.get(&key).map_or(0, |entry| entry.rows);
        let old_bytes = self
            .entries
            .get(&key)
            .map_or(0, |entry| entry.estimated_bytes);
        let mut candidate_rows = self
            .total_rows
            .checked_sub(old_rows)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
        let mut candidate_bytes = self
            .total_estimated_bytes
            .checked_sub(old_bytes)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;

        let mut inactive: Vec<(CacheKey, u64, usize, usize)> = self
            .entries
            .iter()
            .filter(|(candidate, _)| *candidate != &key)
            .filter(|(candidate, _)| active != Some(*candidate))
            .map(|(candidate, entry)| {
                (
                    candidate.clone(),
                    entry.lru_sequence,
                    entry.rows,
                    entry.estimated_bytes,
                )
            })
            .collect();
        inactive.sort_by(|left, right| {
            (left.1, &left.0).cmp(&(right.1, &right.0))
        });

        let mut evictions = Vec::new();
        let mut cursor = 0;
        loop {
            let requested_rows = candidate_rows
                .checked_add(rows)
                .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
            let requested_bytes = candidate_bytes
                .checked_add(estimated_bytes)
                .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
            if requested_rows <= self.budgets.global_row_limit
                && requested_bytes <= self.budgets.global_byte_limit
            {
                break;
            }
            let Some((candidate, _, evict_rows, evict_bytes)) = inactive.get(cursor) else {
                return if requested_bytes > self.budgets.global_byte_limit {
                    Err(BudgetExceeded::CacheBytes {
                        limit: self.budgets.global_byte_limit,
                        requested: requested_bytes,
                    })
                } else {
                    Err(BudgetExceeded::CacheRows {
                        limit: self.budgets.global_row_limit,
                        requested: requested_rows,
                    })
                };
            };
            candidate_rows = candidate_rows
                .checked_sub(*evict_rows)
                .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
            candidate_bytes = candidate_bytes
                .checked_sub(*evict_bytes)
                .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
            evictions.push(candidate.clone());
            cursor += 1;
        }

        let next_lru_sequence = self
            .next_lru_sequence
            .checked_add(1)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
        let final_rows = candidate_rows
            .checked_add(rows)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
        let final_bytes = candidate_bytes
            .checked_add(estimated_bytes)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;

        for evict in evictions {
            self.entries.remove(&evict);
        }
        self.entries.remove(&key);
        self.entries.insert(
            key,
            CacheEntry {
                result,
                rows,
                estimated_bytes,
                lru_sequence: self.next_lru_sequence,
                fetched_at: now,
                stale: false,
            },
        );
        self.total_rows = final_rows;
        self.total_estimated_bytes = final_bytes;
        self.next_lru_sequence = next_lru_sequence;
        Ok(())
    }

    pub fn touch(&mut self, key: &CacheKey) -> Result<(), BudgetExceeded> {
        let next = self
            .next_lru_sequence
            .checked_add(1)
            .ok_or(BudgetExceeded::BudgetArithmeticOverflow)?;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.lru_sequence = self.next_lru_sequence;
            self.next_lru_sequence = next;
        }
        Ok(())
    }

    pub fn contains(&self, key: &CacheKey) -> bool {
        self.entries.contains_key(key)
    }

    pub fn total_rows(&self) -> usize { self.total_rows }
    pub fn total_estimated_bytes(&self) -> usize { self.total_estimated_bytes }

    pub fn snapshot(&self) -> CacheSnapshot {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .map(|(key, entry)| {
                (
                    key.clone(),
                    entry.rows,
                    entry.estimated_bytes,
                    entry.lru_sequence,
                )
            })
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        CacheSnapshot {
            entries,
            total_rows: self.total_rows,
            total_estimated_bytes: self.total_estimated_bytes,
            next_lru_sequence: self.next_lru_sequence,
        }
    }
}
```

The entire candidate plan and every possible error are computed before the first map mutation. Admission receives `Clock::now()` from the caller; `RowCache` never calls `Instant::now()` in production.

- [ ] **Step 3 GREEN: Export and integrate**

Add `pub mod row_cache;` to `src/state/mod.rs`. Add `pub row_cache: Option<RowCache>` to `AppState` and initialize it to `None` in the existing one-argument `AppState::new`, preserving all Phase 2 and Phase 3 constructor call sites. Add these typed, panic-free APIs to `src/state/app_state.rs`:

```rust
#[derive(Debug)]
pub enum RowCacheStateError {
    AlreadyInstalled,
    NotInstalled,
    Admission(crate::resource::budget::BudgetExceeded),
}

impl std::fmt::Display for RowCacheStateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyInstalled => {
                write!(formatter, "row cache budgets may be installed only once")
            }
            Self::NotInstalled => write!(formatter, "row cache is not installed"),
            Self::Admission(error) => write!(formatter, "row cache admission failed: {error:?}"),
        }
    }
}

impl std::error::Error for RowCacheStateError {}

impl From<crate::resource::budget::BudgetExceeded> for RowCacheStateError {
    fn from(error: crate::resource::budget::BudgetExceeded) -> Self {
        Self::Admission(error)
    }
}

impl AppState {
    pub fn install_row_cache(
        &mut self,
        budgets: crate::resource::budget::ResourceBudgets,
    ) -> Result<(), RowCacheStateError> {
        if self.row_cache.is_some() {
            return Err(RowCacheStateError::AlreadyInstalled);
        }
        self.row_cache = Some(crate::state::row_cache::RowCache::new(budgets));
        Ok(())
    }

    pub fn admit_row_cache(
        &mut self,
        key: crate::state::row_cache::CacheKey,
        result: crate::api::types::QueryResult,
        active: Option<&crate::state::row_cache::CacheKey>,
        now: std::time::Instant,
    ) -> Result<(), RowCacheStateError> {
        self.row_cache
            .as_mut()
            .ok_or(RowCacheStateError::NotInstalled)?
            .admit(key, result, active, now)?;
        Ok(())
    }
}
```

In production bootstrap, call `state.install_row_cache(config.resource_budgets)?` before dispatching any read effect. Route each successful bounded table read and accepted Live batch through the exact panic-free call `state.admit_row_cache(key, result, active_key.as_ref(), clock.now())?` before replacing visible state. Propagate `RowCacheStateError` through the existing `anyhow::Result` application boundary. Tests that construct `AppState` directly and do not exercise cache admission remain valid; cache integration tests call `install_row_cache` explicitly and assert that a second installation returns `RowCacheStateError::AlreadyInstalled` and admission before installation returns `RowCacheStateError::NotInstalled`.

- [ ] **Step 4 GREEN: Run tests**

```bash
cargo test active_entry_is_never_evicted_and_inactive_lru_is_deterministic -- --nocapture
cargo test failed_replacement_is_fully_atomic -- --nocapture
cargo test one_view_cannot_bypass_global_row_limit -- --nocapture
```

Expected: PASS.

- [ ] **Step 5 Commit**

```bash
git add src/state/row_cache.rs src/state/mod.rs src/state/app_state.rs src/ui/tabs/tables.rs
git commit -m "feat: add atomic global row cache"
```

---

### Task 10: Treat a full Live event queue as an owned-task terminal outcome

**Files:**
- Modify: `src/api/ws.rs`
- Modify: `src/effects/subscription.rs`
- Modify: `src/effects/runner.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`

- [ ] **Step 1 RED: Add full-versus-closed and stale-state tests**

Add to `src/api/ws.rs` tests:

```rust
#[test]
fn data_event_send_distinguishes_full_from_closed() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.try_send(WsEvent::Connected).unwrap();
    assert_eq!(
        try_send_data_event(&tx, WsEvent::Connected),
        Err(DataEventSendFailure::Full)
    );
    drop(rx);
    assert_eq!(
        try_send_data_event(&tx, WsEvent::Connected),
        Err(DataEventSendFailure::Closed)
    );
}
```

Add to `src/app/reducer.rs` tests using the local `phase4_query_result` fixture from Task 3:

```rust
#[test]
fn queue_overflow_pauses_live_and_preserves_verified_rows_as_stale() {
    let mut state = AppState::new("http://localhost:3000".to_string());
    state.resources.table_rows = LoadState::Ready {
        data: phase4_query_result(2),
        refreshed_at: std::time::Instant::now(),
    };
    let context = SubscriptionContext {
        key: SubscriptionKey { database: "db".into(), table: "users".into() },
        task_generation: TaskGeneration(7),
        connection_generation: ConnectionGeneration(9),
        request_id: 42,
    };

    reduce(
        &mut state,
        AppEvent::SubscriptionQueueOverflow {
            context,
            capacity: 1,
        },
    );

    assert_eq!(state.resources.row_live.phase, LivePhase::Paused);
    assert!(matches!(
        state.resources.table_rows,
        LoadState::Stale { reason: StaleReason::BudgetExceeded, .. }
    ));
}
```

Run: `cargo test data_event_send_distinguishes_full_from_closed -- --nocapture`

Expected: FAIL because the producer currently awaits an indefinitely growing backlog or loses the reason.

- [ ] **Step 2 GREEN: Add a bounded producer result and task exit reason**

Add the bounded producer result to `src/api/ws.rs`; `WsTaskExit` and `WsHandle::join_with_exit` already exist from Task 6:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataEventSendFailure {
    Full,
    Closed,
}

pub fn try_send_data_event(
    sender: &mpsc::Sender<WsEvent>,
    event: WsEvent,
) -> Result<(), DataEventSendFailure> {
    match sender.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) => Err(DataEventSendFailure::Full),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(DataEventSendFailure::Closed),
    }
}
```

On `DataEventSendFailure::Full`, stop reading immediately and return `WsTaskExit::QueueBudgetExceeded`. Do not enqueue an overflow event into the same full queue.

- [ ] **Step 3 GREEN: Add exact event and reducer arm**

Add to `AppEvent`:

```rust
SubscriptionQueueOverflow {
    context: SubscriptionContext,
    capacity: usize,
},
```

Reducer arm:

```rust
AppEvent::SubscriptionQueueOverflow { context, capacity } => {
    state.resources.row_live.phase = LivePhase::Paused;
    state.resources.row_live.context = Some(context);
    state.resources.row_live.detail = Some(format!(
        "Paused because the Live event queue reached capacity {capacity}",
    ));
    mark_current_rows_stale(
        &mut state.resources.table_rows,
        StaleReason::BudgetExceeded,
    );
    Transition::none()
}
```

The effect runner converts `WsTaskExit::QueueBudgetExceeded` to this event, then emits `SubscriptionTaskJoined`; Activity records the join before finalization clears the task. Any later data event is rejected because `controller.accepts_event` is false after the task is cleared or non-Active.

- [ ] **Step 4 GREEN: Run tests**

```bash
cargo test data_event_send_distinguishes_full_from_closed -- --nocapture
cargo test queue_overflow_pauses_live_and_preserves_verified_rows_as_stale -- --nocapture
```

Expected: PASS.

- [ ] **Step 5 Commit**

```bash
git add src/api/ws.rs src/effects/subscription.rs src/effects/runner.rs src/app/event.rs src/app/reducer.rs
git commit -m "feat: terminate live tasks on queue overflow"
```

---

### Task 11: Add deterministic bounded retry for the exact typed Phase 2 reads only

**Files:**
- Create: `src/effects/read_retry.rs`
- Modify: `src/effects/mod.rs`
- Modify: `src/effects/runner.rs`
- Modify: `src/app/event.rs`
- Modify: `src/app/reducer.rs`
- Modify: `src/state/activity.rs`

- [ ] **Step 1 RED: Add exact typed-descriptor, chain-preservation, and no-mutation-retry tests**

Create `src/effects/read_retry.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::{Effect, ReadOperation, TableTarget};
    use crate::effects::ports::{
        FakeRandomSource, FixedBackoffPolicy, Sleeper,
    };
    use crate::effects::request::{RequestContext, RequestId, RequestScope};
    use crate::state::safety::MutationOutcome;
    use std::future::Future;
    use std::pin::Pin;

    struct ImmediateSleeper;

    impl Sleeper for ImmediateSleeper {
        fn sleep(
            &self,
            _duration: std::time::Duration,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }

    fn context(scope: RequestScope) -> RequestContext {
        RequestContext::new(RequestId::from_u64(9), scope, 3)
    }

    #[test]
    fn bounded_read_retry_uses_exact_backoff_signature() {
        let policy = ReadRetryPolicy { max_attempts: 3 };
        let backoff = FixedBackoffPolicy {
            base: std::time::Duration::from_millis(100),
        };
        let mut random = FakeRandomSource::new(vec![3]);

        assert_eq!(
            policy.delay(2, &backoff, &mut random),
            Some(std::time::Duration::from_millis(203))
        );
        let mut random = FakeRandomSource::new(vec![3]);
        assert_eq!(policy.delay(4, &backoff, &mut random), None);
    }

    #[test]
    fn read_operation_maps_to_exact_phase2_effects_and_excludes_other_scopes() {
        let catalog_context = context(RequestScope::DatabaseCatalog);
        assert_eq!(
            ReadOperation::Catalog.into_effect(catalog_context.clone()),
            Effect::LoadCatalog { context: catalog_context }
        );

        let schema_context = context(RequestScope::Schema { database: "db".into() });
        assert_eq!(
            ReadOperation::Schema { database: "db".into() }
                .into_effect(schema_context.clone()),
            Effect::LoadSchema {
                database: "db".into(),
                context: schema_context,
            }
        );

        let table_context = context(RequestScope::TableRows {
            database: "db".into(),
            table: "users".into(),
            view: "browse".into(),
        });
        let target = TableTarget { database: "db".into(), table: "users".into() };
        assert_eq!(
            ReadOperation::TableRows { target: target.clone() }
                .into_effect(table_context.clone()),
            Effect::LoadTableRows { target, context: table_context }
        );

        for scope in [
            RequestScope::SqlWorkspace { database: "db".into(), workspace: "sql".into() },
            RequestScope::Logs { database: "db".into() },
            RequestScope::Metrics { database: "db".into() },
            RequestScope::LiveClients { database: "db".into() },
        ] {
            assert_eq!(ReadOperation::from_scope(&scope), None);
        }
    }

    #[tokio::test]
    async fn sleeper_preserves_read_operation_without_real_time() {
        let context = context(RequestScope::TableRows {
            database: "db".into(),
            table: "users".into(),
            view: "browse".into(),
        });
        let retry = ReadOperation::TableRows {
            target: TableTarget { database: "db".into(), table: "users".into() },
        };
        let event = sleep_then_retry(
            &ImmediateSleeper,
            context.clone(),
            retry.clone(),
            2,
            std::time::Duration::from_millis(203),
        ).await;

        assert!(matches!(
            event,
            AppEvent::ReadRetryReady {
                context: delivered,
                retry: delivered_retry,
                attempt: 2,
            } if delivered == context && delivered_retry == retry
        ));
    }

    #[test]
    fn every_mutation_outcome_schedules_zero_automatic_retries() {
        let outcomes = [
            MutationOutcome::DefinitelyNotSent { reason: "validation".into() },
            MutationOutcome::SentAndConfirmed { affected_rows: Some(1) },
            MutationOutcome::Conflict { reason: "changed".into() },
            MutationOutcome::Unknown { reason: "timeout".into() },
        ];
        for outcome in &outcomes {
            assert!(!automatic_mutation_retry(outcome));
        }
    }
}
```

Add to `src/app/reducer.rs` tests:

```rust
#[test]
fn read_operation_survives_plan_sleep_ready_and_becomes_exact_phase2_effect() {
    let mut state = AppState::new("http://localhost:3000".to_string());
    let context = state.requests.next_context(RequestScope::TableRows {
        database: "db".into(),
        table: "users".into(),
        view: "browse".into(),
    });
    state.resources.table_rows = LoadState::Refreshing {
        data: phase4_query_result(2),
        request: context.clone(),
    };
    let target = TableTarget { database: "db".into(), table: "users".into() };
    let retry = ReadOperation::TableRows { target: target.clone() };

    let planned = reduce(
        &mut state,
        AppEvent::ScopedReadFailed {
            context: context.clone(),
            failure: ReadFailure::Transport("reset".into()),
            retry: Some(retry.clone()),
        },
    );
    assert!(matches!(
        planned.effects.as_slice(),
        [Effect::PlanReadRetry {
            context: delivered,
            retry: delivered_retry,
            attempt: 1,
            ..
        }] if delivered == &context && delivered_retry == &retry
    ));

    let scheduled = reduce(
        &mut state,
        AppEvent::ReadRetryScheduled {
            context: context.clone(),
            retry: retry.clone(),
            attempt: 2,
            delay: std::time::Duration::from_millis(203),
        },
    );
    assert!(matches!(
        scheduled.effects.as_slice(),
        [Effect::SleepReadRetry {
            context: delivered,
            retry: delivered_retry,
            attempt: 2,
            ..
        }] if delivered == &context && delivered_retry == &retry
    ));

    let ready = reduce(
        &mut state,
        AppEvent::ReadRetryReady {
            context: context.clone(),
            retry,
            attempt: 2,
        },
    );
    assert!(matches!(
        ready.effects.as_slice(),
        [Effect::LoadTableRows { target: delivered, context: delivered_context }]
            if delivered == &target && delivered_context == &context
    ));
}

#[test]
fn retry_exhaustion_keeps_verified_rows_stale() {
    let mut state = AppState::new("http://localhost:3000".to_string());
    let context = state.requests.next_context(RequestScope::TableRows {
        database: "db".into(),
        table: "users".into(),
        view: "browse".into(),
    });
    state.resources.table_rows = LoadState::Refreshing {
        data: phase4_query_result(2),
        request: context.clone(),
    };

    reduce(
        &mut state,
        AppEvent::ReadRetryExhausted {
            context,
            failure: ReadFailure::HttpStatus(503),
        },
    );

    assert!(matches!(state.resources.table_rows, LoadState::Stale { .. }));
    assert_eq!(state.activity.retry.attempt, 0);
    assert_eq!(state.activity.retry.next_retry_label, None);
}
```

Run: `cargo test read_operation_maps_to_exact_phase2_effects_and_excludes_other_scopes -- --nocapture`

Expected: FAIL because the Phase 4 `ReadOperation` extension methods and retry-chain variants do not exist.

- [ ] **Step 2 GREEN: Extend the exact Phase 2 operation and implement pure retry policy**

Add `RequestScope` to the existing `src/app/event.rs` request imports. Add the following methods to the existing Phase 2 `ReadOperation`; do not move or redefine the enum in `read_retry.rs`:

```rust
impl ReadOperation {
    pub fn from_effect(effect: &Effect) -> Option<Self> {
        match effect {
            Effect::LoadCatalog { .. } => Some(Self::Catalog),
            Effect::LoadSchema { database, .. } => {
                Some(Self::Schema { database: database.clone() })
            }
            Effect::LoadTableRows { target, .. } => {
                Some(Self::TableRows { target: target.clone() })
            }
            _ => None,
        }
    }

    pub fn from_scope(scope: &RequestScope) -> Option<Self> {
        match scope {
            RequestScope::DatabaseCatalog => Some(Self::Catalog),
            RequestScope::Schema { database } => {
                Some(Self::Schema { database: database.clone() })
            }
            RequestScope::TableRows { database, table, .. } => Some(Self::TableRows {
                target: TableTarget { database: database.clone(), table: table.clone() },
            }),
            RequestScope::SqlWorkspace { .. }
            | RequestScope::Logs { .. }
            | RequestScope::Metrics { .. }
            | RequestScope::LiveClients { .. } => None,
        }
    }

    pub fn matches_scope(&self, scope: &RequestScope) -> bool {
        match (self, scope) {
            (Self::Catalog, RequestScope::DatabaseCatalog) => true,
            (Self::Schema { database: expected }, RequestScope::Schema { database }) => {
                expected == database
            }
            (
                Self::TableRows { target },
                RequestScope::TableRows { database, table, .. },
            ) => target.database == *database && target.table == *table,
            _ => false,
        }
    }

    pub fn into_effect(self, context: RequestContext) -> Effect {
        match self {
            Self::Catalog => Effect::LoadCatalog { context },
            Self::Schema { database } => Effect::LoadSchema { database, context },
            Self::TableRows { target } => Effect::LoadTableRows { target, context },
        }
    }
}
```

Create `src/effects/read_retry.rs` with the policy definitions below after extending `ReadOperation`.

```rust
use crate::app::event::{AppEvent, ReadOperation};
use crate::effects::ports::{BackoffPolicy, RandomSource, Sleeper};
use crate::effects::request::RequestContext;
use crate::state::safety::MutationOutcome;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRetryClass {
    Retryable,
    NotRetryable,
}

pub fn classify_read_failure(failure: &crate::app::event::ReadFailure) -> ReadRetryClass {
    match failure {
        crate::app::event::ReadFailure::Transport(_)
        | crate::app::event::ReadFailure::Network(_) => ReadRetryClass::Retryable,
        crate::app::event::ReadFailure::HttpStatus(429 | 500 | 502 | 503 | 504) => {
            ReadRetryClass::Retryable
        }
        crate::app::event::ReadFailure::BudgetExceeded(_)
        | crate::app::event::ReadFailure::Protocol(_)
        | crate::app::event::ReadFailure::HttpStatus(_) => ReadRetryClass::NotRetryable,
    }
}

pub struct ReadRetryPolicy {
    pub max_attempts: u32,
}

impl ReadRetryPolicy {
    pub fn delay(
        &self,
        attempt: u32,
        backoff: &dyn BackoffPolicy,
        random: &mut dyn RandomSource,
    ) -> Option<Duration> {
        if attempt > self.max_attempts {
            None
        } else {
            Some(backoff.delay(attempt, random))
        }
    }
}

pub async fn sleep_then_retry(
    sleeper: &dyn Sleeper,
    context: RequestContext,
    retry: ReadOperation,
    attempt: u32,
    delay: Duration,
) -> AppEvent {
    sleeper.sleep(delay).await;
    AppEvent::ReadRetryReady { context, retry, attempt }
}

pub fn automatic_mutation_retry(_outcome: &MutationOutcome) -> bool {
    false
}
```

Export it from `src/effects/mod.rs`.

- [ ] **Step 3 GREEN: Preserve the exact `ReadOperation` through the inherited failure event, reducer, and Phase 2 effects**

The Phase 2 `AppEvent::ScopedReadFailed` variant already carries `context`, `failure`, and `retry: Option<ReadOperation>`. Do not add, replace, or restate that variant in `src/app/event.rs`; consume it unchanged. This task adds only the retry lifecycle events below.

Add only the retry lifecycle events in `AppEvent`:

```rust
ReadRetryScheduled {
    context: RequestContext,
    retry: ReadOperation,
    attempt: u32,
    delay: std::time::Duration,
},
ReadRetryReady {
    context: RequestContext,
    retry: ReadOperation,
    attempt: u32,
},
ReadRetryExhausted {
    context: RequestContext,
    failure: ReadFailure,
},
```

Add only these retry-control variants to `Effect`:

```rust
PlanReadRetry {
    context: RequestContext,
    retry: ReadOperation,
    attempt: u32,
    failure: ReadFailure,
},
SleepReadRetry {
    context: RequestContext,
    retry: ReadOperation,
    attempt: u32,
    delay: std::time::Duration,
},
```

Update every Task 3 and later `ScopedReadFailed` constructor. The existing Phase 2 runner arms attach the descriptor directly from their typed effect arm:

```rust
// Effect::LoadCatalog failure
AppEvent::ScopedReadFailed {
    context,
    failure,
    retry: Some(ReadOperation::Catalog),
}

// Effect::LoadSchema { database, context } failure
AppEvent::ScopedReadFailed {
    context,
    failure,
    retry: Some(ReadOperation::Schema { database }),
}

// Effect::LoadTableRows { target, context } failure
AppEvent::ScopedReadFailed {
    context,
    failure,
    retry: Some(ReadOperation::TableRows { target }),
}
```

Failures for `RequestScope::SqlWorkspace`, `Logs`, `Metrics`, and `LiveClients` set `retry: None`. They remain visible scoped failures but are not automatically retried until a future phase introduces exact typed execution effects for those operations. A display label from `RequestScope::fmt` is never retained as an execution target.

Reuse the single scope-complete `apply_scoped_read_failure` helper installed in Task 3. Do not define a second helper or change its `SqlWorkspace` mapping.

Replace the Task 3 `ScopedReadFailed` arm and add the retry event arms:

```rust
AppEvent::ScopedReadFailed { context, failure, retry } => {
    if !state.requests.is_current(&context) {
        return Transition::none();
    }
    let retry = retry.filter(|retry| retry.matches_scope(context.scope()));
    if classify_read_failure(&failure) != ReadRetryClass::Retryable || retry.is_none() {
        apply_scoped_read_failure(state, &context, &failure);
        return Transition::none();
    }
    let Some(attempt) = state.activity.retry.attempt.checked_add(1) else {
        state.activity.retry.attempt = 0;
        state.activity.retry.next_retry_label = None;
        apply_scoped_read_failure(state, &context, &failure);
        return Transition::none();
    };
    let Some(retry) = retry else {
        apply_scoped_read_failure(state, &context, &failure);
        return Transition::none();
    };
    Transition::effects(vec![Effect::PlanReadRetry {
        context,
        retry,
        attempt,
        failure,
    }])
}
AppEvent::ReadRetryScheduled { context, retry, attempt, delay } => {
    if !state.requests.is_current(&context) || !retry.matches_scope(context.scope()) {
        return Transition::none();
    }
    state.activity.retry.attempt = attempt;
    state.activity.retry.next_retry_label = Some(format!(
        "retry {attempt} in {} ms",
        delay.as_millis(),
    ));
    Transition::effects(vec![Effect::SleepReadRetry {
        context,
        retry,
        attempt,
        delay,
    }])
}
AppEvent::ReadRetryReady { context, retry, attempt: _ } => {
    if state.requests.is_current(&context) && retry.matches_scope(context.scope()) {
        Transition::effects(vec![retry.into_effect(context)])
    } else {
        Transition::none()
    }
}
AppEvent::ReadRetryExhausted { context, failure } => {
    if state.requests.is_current(&context) {
        state.activity.retry.attempt = 0;
        state.activity.retry.next_retry_label = None;
        apply_scoped_read_failure(state, &context, &failure);
    }
    Transition::none()
}
```

On an accepted `ScopedReadCompleted`, reset `state.activity.retry.attempt` to `0` and `state.activity.retry.next_retry_label` to `None`. The current-request check rejects late retry wakeups after resource replacement.

- [ ] **Step 4 GREEN: Extend the one `EffectRunner<A, S, T>` and preserve owned bounded dispatch**

Add one concrete runtime field to the existing Phase 2 runner. Do not add generic parameters beyond `A, S, T`, do not define another runner, and do not replace its concrete `TaskRegistry` or bounded `mpsc::Sender<AppEvent>`:

```rust
pub struct ReadRetryRuntime {
    pub policy: ReadRetryPolicy,
    pub backoff: Box<dyn BackoffPolicy + Send + Sync>,
    pub random: Box<dyn RandomSource + Send>,
    pub sleeper: std::sync::Arc<dyn Sleeper + Send + Sync>,
}

// Add to the existing EffectRunner<A, S, T> fields:
pub read_retry: ReadRetryRuntime,
```

Append `read_retry: ReadRetryRuntime` to the existing `EffectRunner::new` parameters and initialize that field while preserving the exact Phase 2 fields `api`, `subscriptions`, `terminal`, `tasks: TaskRegistry`, and `event_sender: mpsc::Sender<AppEvent>`, plus the Phase 4 subscription fields from Task 7. Update every constructor call in this task. The production composition root injects the same resolved `BackoffPolicy`, `RandomSource`, and `Sleeper` port implementations used by subscription reconnect; tests inject `FixedBackoffPolicy`, `FakeRandomSource`, and `ImmediateSleeper`.

Add this private method on the existing `impl<A, S, T> EffectRunner<A, S, T>` with the existing Phase 2 bounds. It either owns a retry-control effect or returns the unchanged effect to normal dispatch:

```rust
fn dispatch_read_retry_control(
    &mut self,
    effect: Effect,
) -> Result<Option<crate::effects::task_registry::TaskId>, Effect> {
    match effect {
        Effect::PlanReadRetry {
            context,
            retry,
            attempt,
            failure,
        } => {
            let event = match self.read_retry.policy.delay(
                attempt,
                self.read_retry.backoff.as_ref(),
                self.read_retry.random.as_mut(),
            ) {
                Some(delay) => AppEvent::ReadRetryScheduled {
                    context,
                    retry,
                    attempt,
                    delay,
                },
                None => AppEvent::ReadRetryExhausted { context, failure },
            };
            let sender = self.event_sender.clone();
            Ok(Some(self.tasks.spawn("plan read retry", async move {
                let _ = sender.send(event).await;
            })))
        }
        Effect::SleepReadRetry {
            context,
            retry,
            attempt,
            delay,
        } => {
            let sender = self.event_sender.clone();
            let sleeper = std::sync::Arc::clone(&self.read_retry.sleeper);
            Ok(Some(self.tasks.spawn("sleep read retry", async move {
                let event = sleep_then_retry(
                    sleeper.as_ref(),
                    context,
                    retry,
                    attempt,
                    delay,
                ).await;
                let _ = sender.send(event).await;
            })))
        }
        other => Err(other),
    }
}
```

At the start of the existing Phase 2 `dispatch(&mut self, effect: Effect) -> Option<TaskId>`, add this ownership-preserving gate before its existing `Effect::LoadCatalog`, `LoadSchema`, `LoadTableRows`, subscription, and persistence arms:

```rust
let effect = match self.dispatch_read_retry_control(effect) {
    Ok(task_id) => return task_id,
    Err(effect) => effect,
};
```

`ReadRetryReady` reduces to an existing `Effect::LoadCatalog`, `Effect::LoadSchema`, or `Effect::LoadTableRows`. The unchanged Phase 2 dispatch arms then call `ApiTransport::execute_read(ReadOperation, RequestContext)`, spawn the returned future through the same concrete `TaskRegistry`, and deliver completion through the same bounded `event_sender`. No retry branch returns a bare future or an `AppEvent` directly to callers. Raw SQL, logs, metrics, and live clients remain outside automatic retry, and every `MutationOutcome` produces zero automatic retry effects.

- [ ] **Step 5 GREEN: Run focused tests and forbidden-dispatch scan**

```bash
cargo test bounded_read_retry_uses_exact_backoff_signature -- --nocapture
cargo test read_operation_maps_to_exact_phase2_effects_and_excludes_other_scopes -- --nocapture
cargo test sleeper_preserves_read_operation_without_real_time -- --nocapture
cargo test read_operation_survives_plan_sleep_ready_and_becomes_exact_phase2_effect -- --nocapture
cargo test retry_exhaustion_keeps_verified_rows_stale -- --nocapture
cargo test every_mutation_outcome_schedules_zero_automatic_retries -- --nocapture
python3 - <<'PY'
from pathlib import Path
runner = Path('src/effects/runner.rs').read_text()
event = Path('src/app/event.rs').read_text()
assert runner.count('pub struct ' + 'EffectRunner') == 1
assert 'pub struct EffectRunner<A, S, T>' in runner
assert 'tasks: TaskRegistry' in runner
assert 'event_sender: mpsc::Sender<AppEvent>' in runner
assert 'EffectRunner<A, S, T,' not in runner
assert event.count('pub enum ' + 'ReadOperation') == 1
PY
! rg 'Retryable(Read)|is_none_(or)|context\.scope\(\)\.to_string|execute_read\([^,]*String' src/effects src/app
```

Expected: tests PASS. The structural check shows one `EffectRunner<A, S, T>`, one existing `ReadOperation`, concrete `TaskRegistry`, and the bounded event sender. The forbidden scan returns no duplicate operation, post-MSRV option helper, formatted dispatch key, or string-typed read transport.

- [ ] **Step 6 Commit**

```bash
git add src/effects/read_retry.rs src/effects/mod.rs src/effects/runner.rs src/app/event.rs src/app/reducer.rs src/state/activity.rs
git commit -m "feat: add typed bounded read retry"
```

---

### Task 12: Precompute `GridViewIndex` and `ColumnMetrics` outside render without duplicating row strings

**Files:**
- Create: `src/ui/components/grid_index.rs`
- Modify: `src/ui/components/mod.rs`
- Modify: `src/ui/components/table_grid.rs`
- Modify: `src/ui/tabs/tables.rs`
- Modify: `src/app.rs` where SQL result grids are rebuilt

- [ ] **Step 1 RED: Add complete derived-state, direct-slice viewport, and stale-index render tests**

Create `src/ui/components/grid_index.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn result(count: usize) -> QueryResult {
        QueryResult {
            schema: vec![
                SchemaElement {
                    name: "id".into(),
                    algebraic_type: serde_json::json!({"U64": null}),
                },
                SchemaElement {
                    name: "name".into(),
                    algebraic_type: serde_json::json!({"String": null}),
                },
            ],
            rows: (0..count)
                .map(|value| vec![
                    serde_json::json!(value),
                    serde_json::json!(format!("user-{value}")),
                ])
                .collect(),
            total_duration_micros: 0,
        }
    }

    #[test]
    fn rebuild_computes_index_and_unicode_metrics_without_display_row_copy() {
        let data = result(3);
        let query = GridQuery {
            sort_column: Some(0),
            descending: true,
            search: Some("user".into()),
            max_column_width: 40,
        };
        let derived = GridDerivedState::rebuild(7, &data, query);

        assert_eq!(derived.data_generation, 7);
        assert_eq!(derived.index.visible_to_data, vec![2, 1, 0]);
        assert_eq!(derived.metrics.widths.len(), 2);
    }

    #[test]
    fn visible_rows_returns_direct_high_offset_slice() {
        let data = result(10_000);
        let derived = GridDerivedState::rebuild(7, &data, GridQuery::default());
        let rows = derived.visible_rows(5_000, 20);

        assert_eq!(rows, &derived.index.visible_to_data[5_000..5_020]);
        assert_eq!(
            rows.as_ptr(),
            derived.index.visible_to_data[5_000..].as_ptr(),
        );
    }

    #[test]
    fn visible_rows_saturates_out_of_range_offsets() {
        let data = result(3);
        let derived = GridDerivedState::rebuild(7, &data, GridQuery::default());

        assert!(derived.visible_rows(usize::MAX, usize::MAX).is_empty());
        assert_eq!(derived.visible_rows(2, usize::MAX), &[2]);
    }
}
```

Also add this regression test to the existing `src/ui/components/table_grid.rs` test module. It intentionally supplies a derived index from an older, larger row generation. A render completes successfully by skipping that stale entry rather than indexing the new row set or panicking.

```rust
#[test]
fn stale_derived_index_does_not_panic_when_rows_shrink_before_rebuild() {
    use crate::api::types::{QueryResult, SchemaElement};
    use crate::ui::components::grid_index::{
        ColumnMetrics, GridDerivedState, GridQuery, GridViewIndex,
    };
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::StatefulWidget;

    let data = QueryResult {
        schema: vec![SchemaElement {
            name: "id".into(),
            algebraic_type: serde_json::json!({"U64": null}),
        }],
        rows: vec![vec![serde_json::json!(1)]],
        total_duration_micros: 0,
    };
    let derived = GridDerivedState {
        data_generation: 6,
        query: GridQuery::default(),
        index: GridViewIndex {
            visible_to_data: vec![7],
        },
        metrics: ColumnMetrics { widths: vec![4] },
    };
    let area = Rect::new(0, 0, 20, 3);
    let mut buffer = Buffer::empty(area);
    let mut state = TableGridState::new();

    StatefulWidget::render(
        TableGrid {
            data: &data,
            derived: &derived,
            offset: 0,
            viewport_height: 1,
        },
        area,
        &mut buffer,
        &mut state,
    );
}
```

Run: `cargo test visible_rows_returns_direct_high_offset_slice -- --nocapture`

Expected: FAIL because the derived structures do not exist.

- [ ] **Step 2 GREEN: Retain only the index, query, and column metrics**

Create `src/ui/components/grid_index.rs`:

```rust
use crate::api::types::QueryResult;
use std::borrow::Cow;
use std::cmp::Ordering;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GridQuery {
    pub sort_column: Option<usize>,
    pub descending: bool,
    pub search: Option<String>,
    pub max_column_width: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridViewIndex {
    pub visible_to_data: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMetrics {
    pub widths: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridDerivedState {
    pub data_generation: u64,
    pub query: GridQuery,
    pub index: GridViewIndex,
    pub metrics: ColumnMetrics,
}

pub(crate) fn display_value(value: &serde_json::Value) -> Cow<'_, str> {
    match value {
        serde_json::Value::Null => Cow::Borrowed("NULL"),
        serde_json::Value::String(value) => Cow::Borrowed(value.as_str()),
        value => Cow::Owned(value.to_string()),
    }
}

fn value_rank(value: &serde_json::Value) -> u8 {
    match value {
        serde_json::Value::Null => 0,
        serde_json::Value::Bool(_) => 1,
        serde_json::Value::Number(_) => 2,
        serde_json::Value::String(_) => 3,
        serde_json::Value::Array(_) => 4,
        serde_json::Value::Object(_) => 5,
    }
}

fn compare_values(
    left: Option<&serde_json::Value>,
    right: Option<&serde_json::Value>,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => {
            let rank = value_rank(left).cmp(&value_rank(right));
            if rank != Ordering::Equal {
                return rank;
            }
            match (left, right) {
                (serde_json::Value::Null, serde_json::Value::Null) => Ordering::Equal,
                (serde_json::Value::Bool(left), serde_json::Value::Bool(right)) => {
                    left.cmp(right)
                }
                (serde_json::Value::Number(left), serde_json::Value::Number(right)) => {
                    match (left.as_f64(), right.as_f64()) {
                        (Some(left), Some(right)) => match left.partial_cmp(&right) {
                            Some(ordering) => ordering,
                            None => Ordering::Equal,
                        },
                        _ => Ordering::Equal,
                    }
                }
                (serde_json::Value::String(left), serde_json::Value::String(right)) => {
                    left.cmp(right)
                }
                _ => format!("{left:?}").cmp(&format!("{right:?}")),
            }
        }
    }
}

impl GridDerivedState {
    pub fn rebuild(
        data_generation: u64,
        result: &QueryResult,
        mut query: GridQuery,
    ) -> Self {
        if query.max_column_width == 0 {
            query.max_column_width = 40;
        }
        let needle = query.search.as_ref().map(|value| value.to_lowercase());
        let mut visible_to_data: Vec<usize> = result
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                needle.as_ref().map_or(true, |needle| {
                    row.iter().any(|cell| {
                        display_value(cell).to_lowercase().contains(needle)
                    })
                })
            })
            .map(|(index, _)| index)
            .collect();
        if let Some(column) = query.sort_column {
            visible_to_data.sort_by(|left, right| {
                compare_values(
                    result.rows[*left].get(column),
                    result.rows[*right].get(column),
                )
            });
            if query.descending {
                visible_to_data.reverse();
            }
        }

        let mut widths: Vec<usize> = result
            .schema
            .iter()
            .map(|column| {
                column
                    .name
                    .width()
                    .max(4)
                    .min(query.max_column_width)
            })
            .collect();
        for row in &result.rows {
            for (index, cell) in row.iter().enumerate() {
                if let Some(width) = widths.get_mut(index) {
                    let display = display_value(cell);
                    *width = (*width)
                        .max(display.as_ref().width().min(query.max_column_width));
                }
            }
        }

        Self {
            data_generation,
            query,
            index: GridViewIndex { visible_to_data },
            metrics: ColumnMetrics { widths },
        }
    }

    pub fn visible_rows(&self, offset: usize, height: usize) -> &[usize] {
        let len = self.index.visible_to_data.len();
        let start = offset.min(len);
        let end = start.saturating_add(height).min(len);
        &self.index.visible_to_data[start..end]
    }
}
```

Export it from `src/ui/components/mod.rs`. `GridDerivedState` deliberately has no whole-dataset display-row or header copy. Rebuild may hold one temporary display representation while measuring, searching, or comparing a cell, but persistent derived memory is only the query, `Vec<usize>` index, and column widths. This preserves conservative headroom under the global cache budget instead of creating a second whole-dataset string representation.

- [ ] **Step 3 GREEN: Store and rebuild outside render, then format viewport cells only**

Add this field to the existing `TableGridState` in `src/ui/components/table_grid.rs` and initialize it to `None` in its current constructor/default:

```rust
pub derived: Option<crate::ui::components::grid_index::GridDerivedState>,
```

Add the exact rebuild method:

```rust
pub fn rebuild_derived(
    &mut self,
    data_generation: u64,
    result: &crate::api::types::QueryResult,
    search: &str,
    sort_column: Option<usize>,
    descending: bool,
    max_column_width: usize,
) {
    self.derived = Some(
        crate::ui::components::grid_index::GridDerivedState::rebuild(
            data_generation,
            result,
            crate::ui::components::grid_index::GridQuery {
                sort_column,
                descending,
                search: (!search.is_empty()).then(|| search.to_string()),
                max_column_width,
            },
        ),
    );
}
```

Call `TableGridState::rebuild_derived` after a table or SQL result generation changes and after search or sort settings change. Change `TableGrid` to require these borrowed inputs:

```rust
pub struct TableGrid<'a> {
    pub data: &'a crate::api::types::QueryResult,
    pub derived: &'a crate::ui::components::grid_index::GridDerivedState,
    pub offset: usize,
    pub viewport_height: usize,
}
```

In `TableGrid::render`, iterate the direct slice only:

```rust
for (visible_row, data_index) in self
    .derived
    .visible_rows(self.offset, self.viewport_height)
    .iter()
    .copied()
    .enumerate()
{
    let row_offset = match u16::try_from(visible_row) {
        Ok(value) => value,
        Err(_) => u16::MAX,
    };
    let y = area.y.saturating_add(row_offset);
    if y >= area.bottom() {
        break;
    }
    let Some(row) = self.data.rows.get(data_index) else {
        // A result generation can change between event reduction and the next
        // derived-state rebuild. Treat an old index as absent for this frame.
        continue;
    };
    render_display_row(
        row,
        &self.derived.metrics.widths,
        ratatui::layout::Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        },
        buffer,
    );
}
```

Define the called helper in `table_grid.rs`:

```rust
fn render_display_row(
    row: &[serde_json::Value],
    widths: &[usize],
    area: ratatui::layout::Rect,
    buffer: &mut ratatui::buffer::Buffer,
) {
    let mut x = area.x;
    for (cell, width) in row.iter().zip(widths) {
        if x >= area.right() {
            break;
        }
        let requested = match u16::try_from(*width) {
            Ok(value) => value,
            Err(_) => u16::MAX,
        };
        let available = requested.min(area.right().saturating_sub(x));
        let display = crate::ui::components::grid_index::display_value(cell);
        buffer.set_stringn(
            x,
            area.y,
            display.as_ref(),
            usize::from(available),
            ratatui::style::Style::default(),
        );
        x = x.saturating_add(available.saturating_add(1));
    }
}
```

Move any selected-row style choice into the `Style` argument without adding row scans. Render formats only cells in the borrowed viewport slice, one cell at a time. Sorting, filtering, index construction, and column-width scans remain outside render, and no render path traverses the prefix before `offset`.

- [ ] **Step 4 GREEN: Run focused tests**

```bash
cargo test rebuild_computes_index_and_unicode_metrics_without_display_row_copy -- --nocapture
cargo test visible_rows_returns_direct_high_offset_slice -- --nocapture
cargo test visible_rows_saturates_out_of_range_offsets -- --nocapture
cargo test stale_derived_index_does_not_panic_when_rows_shrink_before_rebuild -- --nocapture
cargo test ui::components::table_grid -- --nocapture
```

Expected: PASS.

- [ ] **Step 5 Commit**

```bash
git add src/ui/components/grid_index.rs src/ui/components/mod.rs src/ui/components/table_grid.rs src/ui/tabs/tables.rs src/app.rs
git commit -m "feat: precompute bounded grid view state"
```

---

### Task 13: Add deterministic peak, cache, queue, and render gates

**Files:**
- Modify: `src/api/http_body.rs`
- Modify: `src/api/query_decode.rs`
- Modify: `src/api/ws_decode.rs`
- Modify: `src/api/ws.rs`
- Modify: `src/state/row_cache.rs`
- Modify: `src/ui/components/grid_index.rs`
- No dependency changes

- [ ] **Step 1 RED: Add gates over real production APIs**

Add these named tests to their owning modules. Each fixture is defined in its local module by the earlier tasks.

```rust
// src/api/http_body.rs
#[test]
fn phase4_gate_http_body_rejects_before_decode() {
    let mut body = BoundedBodyAccumulator::new(4);
    assert_eq!(
        body.push(b"12345"),
        Err(BudgetExceeded::HttpBody { limit: 4, actual: 5 })
    );
}

// src/api/query_decode.rs
#[test]
fn phase4_gate_single_http_row_rejects_before_result_return() {
    let budgets = ResourceBudgets {
        single_row_byte_limit: 32,
        ..ResourceBudgets::default()
    };
    assert!(matches!(
        decode_query_result_bounded(
            format!(r#"[{{"schema":[],"rows":[["{}"]]}}]"#, "x".repeat(128)).as_bytes(),
            budgets,
        ),
        Err(DecodeError::Budget(BudgetExceeded::SingleRow { .. }))
    ));
}

// src/ui/components/grid_index.rs
#[test]
fn phase4_gate_large_render_slice_is_viewport_bounded() {
    let data = result(10_000);
    let derived = GridDerivedState::rebuild(1, &data, GridQuery::default());
    assert_eq!(derived.visible_rows(4_000, 20).len(), 20);
}
```

Add this cache gate to `src/state/row_cache.rs` tests:

```rust
#[test]
fn phase4_gate_switching_100_resources_stays_under_global_limits() {
    let budgets = ResourceBudgets::default();
    let mut cache = RowCache::new(budgets);
    let now = std::time::Instant::now();
    for resource in 0..100 {
        let _ = cache.admit(
            key(&format!("resource-{resource}")),
            result(200),
            None,
            now,
        );
        assert!(cache.total_rows() <= budgets.global_row_limit);
        assert!(cache.total_estimated_bytes() <= budgets.global_byte_limit);
    }
}
```

Run: `cargo test phase4_gate_http_body_rejects_before_decode -- --nocapture`

Expected: PASS only after Tasks 2 through 12 are complete.

- [ ] **Step 2 GREEN: Add the missing WebSocket and queue gates**

Add this WebSocket gate to `src/api/ws_decode.rs` tests:

```rust
fn phase4_gate_context() -> SubscriptionContext {
    SubscriptionContext {
        key: SubscriptionKey {
            database: "db".into(),
            table: "users".into(),
        },
        task_generation: TaskGeneration(7),
        connection_generation: ConnectionGeneration(9),
        request_id: 42,
    }
}

#[test]
fn phase4_gate_ws_second_row_rejects_before_event_return() {
    let budgets = ResourceBudgets {
        decoded_batch_row_limit: 1,
        ..ResourceBudgets::default()
    };
    let result = decode_subscription_text_bounded(
        r#"{"TransactionUpdate":{"status":"committed","database_update":{"tables":[{"table_id":3,"table_name":"users","num_rows":2,"inserts":[{"id":1},{"id":2}],"deletes":[]}]}}}"#,
        &phase4_gate_context(),
        budgets,
    );

    assert!(matches!(
        result,
        Err(DecodeError::Budget(BudgetExceeded::DecodedBatchRows {
            limit: 1,
            actual: 2,
        }))
    ));
}
```

Add this queue gate to `src/api/ws.rs` tests:

```rust
#[test]
fn phase4_gate_queue_capacity_is_a_terminal_full_signal() {
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    try_send_data_event(&sender, WsEvent::Connected).unwrap();

    assert_eq!(
        try_send_data_event(&sender, WsEvent::Connected),
        Err(DataEventSendFailure::Full)
    );
}
```

Both tests use production APIs from Tasks 4 and 10; no instrumentation-only production API is added.

- [ ] **Step 3 GREEN: Run all deterministic gates and release checks**

```bash
cargo test phase4_gate_ -- --nocapture
cargo test -- --nocapture
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo build --release
```

Expected: PASS with zero failed tests and zero warnings.

- [ ] **Step 4 Commit**

```bash
git add src/api/http_body.rs src/api/query_decode.rs src/api/ws_decode.rs src/api/ws.rs src/state/row_cache.rs src/ui/components/grid_index.rs
git commit -m "test: add phase 4 resource and render gates"
```

---

### Task 14: Update user-facing truth and run the final protocol scan

**Files:**
- Modify: `README.md`
- Modify: `src/app/command.rs`
- Modify: `src/ui/components/help.rs`
- Create: `docs/releases/phase-4.md`

- [ ] **Step 1 RED: Test the shared command/help truth**

Add one shared constant in `src/app/command.rs` and make both the Live command description and help renderer consume it:

```rust
pub const LIVE_BEHAVIOR_HELP: &str = "Row Live is opt-in for the active resource. Bounded transport, decode, queue, and cache budgets pause Live instead of silently dropping data. Recoverable read failures use bounded retry. Mutations are not automatically retried after transport ownership.";
```

Add tests:

```rust
#[test]
fn phase4_live_help_states_scope_budgets_and_retry_safety() {
    assert!(LIVE_BEHAVIOR_HELP.contains("active resource"));
    assert!(LIVE_BEHAVIOR_HELP.contains("transport"));
    assert!(LIVE_BEHAVIOR_HELP.contains("decode"));
    assert!(LIVE_BEHAVIOR_HELP.contains("queue"));
    assert!(LIVE_BEHAVIOR_HELP.contains("cache"));
    assert!(LIVE_BEHAVIOR_HELP.contains("Mutations are not automatically retried"));
}
```

Run: `cargo test phase4_live_help_states_scope_budgets_and_retry_safety -- --nocapture`

Expected: FAIL until the shared truth exists.

- [ ] **Step 2 GREEN: Update README, generated help, and release notes**

`README.md` and `docs/releases/phase-4.md` must state all of the following:

```markdown
- Row Live is off by default and scoped to the active resource.
- The application does not silently subscribe to every table.
- Bounded capability is preferred; unbounded capability requires explicit confirmation and active safety budgets.
- HTTP bodies, WebSocket frames/messages, decoded rows/batches, event queues, and the global row cache are bounded.
- Budget exhaustion pauses Live and preserves verified data as stale.
- Recoverable reads use bounded retry with visible state.
- Mutations are never automatically retried after transport ownership when the outcome is not authoritative.
- Database-wide observation requires a native bounded feed.
```

Make `src/ui/components/help.rs` render `LIVE_BEHAVIOR_HELP`; do not duplicate the paragraph.

- [ ] **Step 3 GREEN: Run docs test and exact final scans**

```bash
cargo test phase4_live_help_states_scope_budgets_and_retry_safety -- --nocapture
cargo fmt --check
cargo test -- --nocapture
cargo clippy --all-targets -- -D warnings
cargo build --release
rg -n "ws_subscribe_all_tables|Vec<SubscriptionSpec>|HashMap<.*SubscriptionSpec" src
rg -n "GridViewIndex|ColumnMetrics|visible_rows" src/ui src/state
rg -n "ResourceBudgets|BudgetExceeded|decoded_batch|event_queue_capacity" src
rg -n "active resource|native bounded feed|Mutations are not automatically retried" README.md src/app/command.rs src/ui/components/help.rs docs/releases/phase-4.md
```

Expected: build and tests PASS. The first scan returns no all-table or multi-subscription owner. The remaining scans show the required derived-state, budget, and documentation paths.

- [ ] **Step 4 Commit**

```bash
git add README.md src/app/command.rs src/ui/components/help.rs docs/releases/phase-4.md
git commit -m "docs: describe bounded active-resource live behavior"
```

---

## Final implementation-branch acceptance checklist

- [ ] Exactly one `SubscriptionController` owns exactly `desired` and `task`.
- [ ] The actual `OwnedSubscriptionTask` owns the real `WsHandle`.
- [ ] Scope replacement produces `CloseSent -> JoinCompleted -> ActivityRecorded -> TaskCleared -> SpawnNew` with no Close plus Spawn batch.
- [ ] Every initial connect and reconnect obtains a fresh connection generation and request ID from the one shared Phase 2 `IdSource` instance.
- [ ] Active begins only after the current `InitialSubscription.request_id` matches.
- [ ] Pre-ack, old-task, old-socket, and replaced updates are ignored.
- [ ] HTTP rows and WebSocket inserts/deletes are visited one element at a time from a borrowed encoded array.
- [ ] Malformed protocol payloads are `DecodeError::Protocol`; budget failures are `DecodeError::Budget`.
- [ ] No decoder estimates a cloned row.
- [ ] Cache admission is atomic and globally bounded by both rows and bytes.
- [ ] Queue overflow terminates the owning task and pauses/stales presentation without using the full queue to report itself.
- [ ] Retry uses the exact Phase 2 ports, resets singular retry state after success, and never automatically retries mutations.
- [ ] Render consumes precomputed `GridViewIndex`, `ColumnMetrics`, and a viewport slice.
- [ ] Resolved user budget overrides are passed to HTTP, WS, queue, decoder, and cache constructors rather than reconstructed with defaults.
