# Contextual Workbench Phase 1 Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Phase 1 safety and crash-resistance changes for the contextual workbench without changing the larger UI architecture.

**Architecture:** Add narrow `effects`, safety-state, terminal, and UI text helpers, then integrate them into the existing binary crate through explicit parent module declarations. Guided update/delete become validated `WritePlan`s built from `crate::api::types::TableInfo` and selected raw row values, async reads become `RequestContext` guarded, mutations distinguish `DefinitelyNotSent` from `Unknown`, broad automatic Live subscriptions are temporarily unavailable while safety controls are tightened, and terminal/layout/text paths become panic-resistant.

**Tech Stack:** Rust binary crate, Tokio, Ratatui, Crossterm, SpacetimeDB HTTP and WebSocket adapters, `serde_json`, `unicode-width`, existing unit tests.

---

## File structure and responsibilities

- Create `src/effects/mod.rs`: parent module for Phase 1 effect helpers. Add `mod effects;` in `src/main.rs` in Task 1.
- Create `src/effects/request.rs`: `RequestId`, stable `RequestScope` variants, and public-field `RequestContext { id, scope, generation }` with stale-result matching.
- Create `src/state/safety.rs`: stable domain types `WritePlanId`, `QualifiedTable`, `PrimaryKeyPart`, `CompletePrimaryKey`, `ColumnChange`, `GuidedMutation`, `WritePlan`, and `MutationOutcome`.
- Create `src/effects/write_ops.rs`: typed conversion from raw `serde_json::Value` plus declared `ColumnInfo` algebraic types, declared-PK validation from `TableInfo`, the single shared `type_tag`/`json_to_sql_literal` SQL encoder, `WritePlan` creation, confirmation-time revalidation, and mutation outcome classification.
- Create `src/effects/task_registry.rs`: minimal joined `TaskRegistry` for owned async work, cancellation, completion, and panic reporting.
- Create `src/terminal.rs`: `TerminalOps` and `TerminalGuard` with partial, idempotent restoration. Add `mod terminal;` in `src/main.rs` in Task 11.
- Create `src/ui/text.rs`: Unicode display-width truncation and saturating rectangle helpers. Add `pub mod text;` in `src/ui/mod.rs` in Task 12.
- Modify `src/app.rs`: replace heuristic guided row targeting, apply request guards, route mutation outcomes, aggregate spreadsheet changes per single row, disable all-table Live, and fix nav-up reload symmetry.
- Modify `src/state/mod.rs`: add `pub mod safety;` in Task 2.
- Modify `src/state/edit_mode.rs`: represent one dirty spreadsheet row, aggregate changed cells, and expose Save/Discard/Stay transition data.
- Modify `src/state/modal.rs`: add confirmation modal state for `WritePlanId` and dirty-row Save/Discard/Stay.
- Modify `src/ui/tabs/sql.rs`: use Unicode-safe truncation for SQL history and output labels.
- Modify `src/ui/tabs/live.rs`: show clear row-Live disabled state and prevent all-table subscription behavior.
- Modify `src/ui/layout.rs`: use saturating rectangle helpers and tiny-terminal fallback.
- Modify `src/ui/components/help.rs`: document one-row spreadsheet save, disabled Live, and mutation uncertainty.
- Modify `README.md`: update Phase 1 behavior claims.
- Create `CHANGELOG.md`: initial project changelog with Phase 1 safety release notes. Later tasks modify this file.

## Dependency-ordered TDD tasks

### Task 1: Request identity primitives and stale-result matching

**Files:**
- Create: `src/effects/mod.rs`
- Create: `src/effects/request.rs`
- Modify: `src/main.rs`
- Test: `src/effects/request.rs`

- [ ] **Step 1 RED: Write the failing test**

Create `src/effects/mod.rs` with the parent declaration:

```rust
pub mod request;
```

Add `mod effects;` beside the other module declarations in `src/main.rs`:

```rust
mod api;
mod app;
mod config;
mod effects;
mod state;
mod ui;
mod user_config;
```

Create `src/effects/request.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_matches_only_same_id_scope_and_generation() {
        let current = RequestContext::new(
            RequestId::from_u64(7),
            RequestScope::TableRows {
                database: "db".into(),
                table: "inventory".into(),
                view: "main".into(),
            },
            3,
        );

        assert!(current.accepts(&current));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(8),
            current.scope.clone(),
            3,
        )));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(7),
            RequestScope::TableRows {
                database: "db".into(),
                table: "orders".into(),
                view: "main".into(),
            },
            3,
        )));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(7),
            current.scope.clone(),
            4,
        )));
    }

    #[test]
    fn request_scope_distinguishes_all_phase_one_read_scopes() {
        let scopes = [
            RequestScope::DatabaseCatalog,
            RequestScope::Schema { database: "db".into() },
            RequestScope::TableRows { database: "db".into(), table: "t".into(), view: "main".into() },
            RequestScope::SqlWorkspace { database: "db".into(), workspace: "scratch".into() },
            RequestScope::Logs { database: "db".into() },
            RequestScope::Metrics { database: "db".into() },
            RequestScope::LiveClients { database: "db".into() },
        ];

        for (left_index, left) in scopes.iter().enumerate() {
            for (right_index, right) in scopes.iter().enumerate() {
                assert_eq!(left == right, left_index == right_index);
            }
        }
    }

    #[test]
    fn request_context_exposes_scope_by_accessor_without_moving_it() {
        let context = RequestContext::new(
            RequestId::from_u64(9),
            RequestScope::TableRows { database: "secret-db".into(), table: "orders".into(), view: "current".into() },
            4,
        );

        assert!(matches!(context.scope(), RequestScope::TableRows { view, .. } if view == "current"));
        assert_eq!(context.id.get(), 9);
    }

    #[test]
    fn request_scope_display_uses_deterministic_non_sensitive_labels() {
        assert_eq!(RequestScope::DatabaseCatalog.to_string(), "database catalog");
        assert_eq!(RequestScope::Schema { database: "secret-db".into() }.to_string(), "schema");
        assert_eq!(RequestScope::TableRows { database: "secret-db".into(), table: "orders".into(), view: "current".into() }.to_string(), "table rows");
        assert_eq!(RequestScope::SqlWorkspace { database: "secret-db".into(), workspace: "scratch".into() }.to_string(), "SQL workspace");
        assert_eq!(RequestScope::Logs { database: "secret-db".into() }.to_string(), "logs");
        assert_eq!(RequestScope::Metrics { database: "secret-db".into() }.to_string(), "metrics");
        assert_eq!(RequestScope::LiveClients { database: "secret-db".into() }.to_string(), "live clients");
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::request`

Expected: FAIL with compiler errors containing `cannot find type RequestContext in this scope`, `cannot find type RequestId in this scope`, or `cannot find type RequestScope in this scope` because `src/effects/request.rs` has only tests.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this implementation above the test module in `src/effects/request.rs`:

```rust
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(u64);

impl RequestId {
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RequestScope {
    DatabaseCatalog,
    Schema { database: String },
    TableRows { database: String, table: String, view: String },
    SqlWorkspace { database: String, workspace: String },
    Logs { database: String },
    Metrics { database: String },
    LiveClients { database: String },
}

impl fmt::Display for RequestScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::DatabaseCatalog => "database catalog",
            Self::Schema { .. } => "schema",
            Self::TableRows { .. } => "table rows",
            Self::SqlWorkspace { .. } => "SQL workspace",
            Self::Logs { .. } => "logs",
            Self::Metrics { .. } => "metrics",
            Self::LiveClients { .. } => "live clients",
        };
        f.write_str(label)
    }
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

    pub fn scope(&self) -> &RequestScope {
        &self.scope
    }

    pub fn accepts(&self, delivered: &RequestContext) -> bool {
        self.id == delivered.id && self.scope == delivered.scope && self.generation == delivered.generation
    }
}
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::request`

Expected: PASS with request identity, scope accessor, and non-sensitive display-label tests passing.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/effects/mod.rs src/effects/request.rs src/main.rs
git commit -m "feat: add scoped request context"
```

### Task 2: Complete primary-key validation domain types

**Files:**
- Create: `src/state/safety.rs`
- Modify: `src/state/mod.rs`
- Test: `src/state/safety.rs`

- [ ] **Step 1 RED: Write the failing test**

Add `pub mod safety;` to `src/state/mod.rs`:

```rust
pub mod safety;
```

Create `src/state/safety.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn typed_values() -> Vec<Option<SqlValue>> {
        vec![Some(SqlValue::I64(10)), Some(SqlValue::Text("blue".into())), Some(SqlValue::Bool(true))]
    }

    #[test]
    fn complete_primary_key_parts_are_schema_ordered_not_declared_ordered() {
        let key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            PrimaryKeyPart { column_id: 0u16, value: SqlValue::I64(10) },
            PrimaryKeyPart { column_id: 1u16, value: SqlValue::Text("blue".into()) },
        ]).unwrap();

        assert_eq!(key.parts()[0].column_id, 0);
        assert_eq!(key.parts()[1].column_id, 1);
    }

    #[test]
    fn complete_primary_key_rejects_empty_duplicate_null_and_unrepresentable_parts() {
        assert_eq!(CompletePrimaryKey::from_schema_ordered_parts(vec![]), Err(PrimaryKeyError::NoDeclaredPrimaryKey));
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![
                PrimaryKeyPart { column_id: 2u16, value: SqlValue::I64(1) },
                PrimaryKeyPart { column_id: 2u16, value: SqlValue::I64(2) },
            ]),
            Err(PrimaryKeyError::DuplicateColumnId(2))
        );
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart { column_id: 0u16, value: SqlValue::Null }]),
            Err(PrimaryKeyError::NullValue { column_id: 0 })
        );
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart { column_id: 0u16, value: SqlValue::Unrepresentable("array".into()) }]),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 0 })
        );
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test state::safety`

Expected: FAIL with compiler errors containing `cannot find type CompletePrimaryKey`, `cannot find type PrimaryKeyPart`, or `cannot find type SqlValue`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this implementation above the tests in `src/state/safety.rs`:

```rust
use std::collections::HashSet;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PrimaryKeyError {
    NoDeclaredPrimaryKey,
    DuplicateColumnId(u16),
    UnknownColumnId(u16),
    ColumnIdOutOfRange { column_id: u32 },
    MissingValue { column_id: u16 },
    NullValue { column_id: u16 },
    UnrepresentableValue { column_id: u16 },
    TypeMismatch { column_id: u16, expected: String },
}

impl CompletePrimaryKey {
    pub fn from_schema_ordered_parts(parts: Vec<PrimaryKeyPart>) -> Result<Self, PrimaryKeyError> {
        if parts.is_empty() {
            return Err(PrimaryKeyError::NoDeclaredPrimaryKey);
        }
        let mut seen = HashSet::with_capacity(parts.len());
        for part in &parts {
            if !seen.insert(part.column_id) {
                return Err(PrimaryKeyError::DuplicateColumnId(part.column_id));
            }
            match &part.value {
                SqlValue::Null => return Err(PrimaryKeyError::NullValue { column_id: part.column_id }),
                SqlValue::Unrepresentable(_) => return Err(PrimaryKeyError::UnrepresentableValue { column_id: part.column_id }),
                _ => {}
            }
        }
        Ok(Self { parts })
    }

    pub fn parts(&self) -> &[PrimaryKeyPart] {
        &self.parts
    }
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
}
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test state::safety`

Expected: PASS with complete primary-key domain type tests passing.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/state/safety.rs src/state/mod.rs
git commit -m "feat: add write safety domain types"
```

### Task 3: Type-correct raw row conversion and declared PK validation from `TableInfo`

**Files:**
- Create: `src/effects/write_ops.rs`
- Modify: `src/effects/mod.rs`
- Test: `src/effects/write_ops.rs`

- [ ] **Step 1 RED: Write the failing test**

Add `pub mod write_ops;` to `src/effects/mod.rs`:

```rust
pub mod request;
pub mod write_ops;
```

Create `src/effects/write_ops.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{ColumnInfo, TableInfo};
    use crate::state::safety::{PrimaryKeyError, SqlValue};

    fn col(id: u32, name: &str, type_name: &str) -> ColumnInfo {
        let col_type = match type_name {
            "U64" => serde_json::json!({ "AlgebraicType": { "U64": {} } }),
            "String" => serde_json::json!({ "AlgebraicType": { "String": {} } }),
            "Bool" => serde_json::json!({ "AlgebraicType": { "Bool": {} } }),
            other => panic!("unexpected fixture type {other}"),
        };
        ColumnInfo { col_id: id, col_name: name.into(), col_type, is_autoinc: false }
    }

    fn table(primary_key_cols: Vec<u16>) -> TableInfo {
        TableInfo {
            table_name: "inventory".into(),
            product_type_ref: 0,
            table_type: "user".into(),
            table_access: "public".into(),
            columns: vec![col(7, "account", "U64"), col(2, "region", "String"), col(9, "name", "String")],
            primary_key_cols,
            indexes: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn declared_primary_key_resolves_parts_in_schema_order() {
        let key = complete_primary_key_from_table_row(
            &table(vec![2, 7]),
            &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        ).unwrap();

        assert_eq!(key.parts()[0].column_id, 7);
        assert_eq!(key.parts()[0].value, SqlValue::U64(42));
        assert_eq!(key.parts()[1].column_id, 2);
        assert_eq!(key.parts()[1].value, SqlValue::Text("eu".into()));
    }

    #[test]
    fn declared_primary_key_rejects_no_duplicate_unknown_missing_null_and_unrepresentable_values() {
        assert_eq!(complete_primary_key_from_table_row(&table(vec![]), &[]), Err(PrimaryKeyError::NoDeclaredPrimaryKey));
        assert_eq!(complete_primary_key_from_table_row(&table(vec![7, 7]), &[]), Err(PrimaryKeyError::DuplicateColumnId(7)));
        assert_eq!(complete_primary_key_from_table_row(&table(vec![99]), &[]), Err(PrimaryKeyError::UnknownColumnId(99)));
        let mut out_of_range = table(vec![7]);
        out_of_range.columns[0].col_id = u32::from(u16::MAX) + 1;
        assert_eq!(complete_primary_key_from_table_row(&out_of_range, &[]), Err(PrimaryKeyError::ColumnIdOutOfRange { column_id: u32::from(u16::MAX) + 1 }));
        assert_eq!(complete_primary_key_from_table_row(&table(vec![9]), &[serde_json::json!(1)]), Err(PrimaryKeyError::MissingValue { column_id: 9 }));
        assert_eq!(complete_primary_key_from_table_row(&table(vec![7]), &[serde_json::Value::Null]), Err(PrimaryKeyError::NullValue { column_id: 7 }));
        assert_eq!(complete_primary_key_from_table_row(&table(vec![7]), &[serde_json::json!([1, 2])]), Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 }));
    }

    #[test]
    fn declared_column_types_reject_mismatched_json_before_write_plan() {
        let mut typed = table(vec![7]);
        typed.columns[0].col_type = serde_json::json!({ "AlgebraicType": { "U64": {} } });
        typed.columns[1].col_type = serde_json::json!({ "AlgebraicType": { "Bool": {} } });
        typed.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "String": {} } });

        assert_eq!(typed_sql_value_from_json(&typed.columns[0], &serde_json::json!("not a number")), Err(PrimaryKeyError::TypeMismatch { column_id: 7, expected: "U64".into() }));
        assert_eq!(typed_sql_value_from_json(&typed.columns[1], &serde_json::json!(1)), Err(PrimaryKeyError::TypeMismatch { column_id: 2, expected: "Bool".into() }));
        assert_eq!(typed_sql_value_from_json(&typed.columns[2], &serde_json::json!(false)), Err(PrimaryKeyError::TypeMismatch { column_id: 9, expected: "String".into() }));
        assert_eq!(typed_sql_value_from_json(&typed.columns[0], &serde_json::json!({ "U64": "not numeric" })), Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 }));
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::write_ops`

Expected: FAIL with compiler errors containing `cannot find function complete_primary_key_from_table_row` or missing imports.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this implementation above the tests in `src/effects/write_ops.rs`:

```rust
use std::collections::HashSet;

use crate::api::types::TableInfo;
use crate::state::safety::{CompletePrimaryKey, PrimaryKeyError, PrimaryKeyPart, SqlValue};

pub fn typed_sql_value_from_json(column: &crate::api::types::ColumnInfo, value: &serde_json::Value) -> Result<SqlValue, PrimaryKeyError> {
    let declared = type_tag(&column.col_type).map_err(|_| PrimaryKeyError::UnrepresentableValue { column_id: u16::try_from(column.col_id).map_err(|_| PrimaryKeyError::ColumnIdOutOfRange { column_id: column.col_id })? })?;
    let column_id = u16::try_from(column.col_id).map_err(|_| PrimaryKeyError::ColumnIdOutOfRange { column_id: column.col_id })?;
    sql_value_from_json_for_type(value, &declared).map_err(|error| match error {
        SqlEncodingError::TypeMismatch { expected, .. } => PrimaryKeyError::TypeMismatch { column_id, expected },
        _ => PrimaryKeyError::UnrepresentableValue { column_id },
    })
}

pub fn complete_primary_key_from_table_row(table: &TableInfo, row: &[serde_json::Value]) -> Result<CompletePrimaryKey, PrimaryKeyError> {
    if table.primary_key_cols.is_empty() {
        return Err(PrimaryKeyError::NoDeclaredPrimaryKey);
    }

    let mut declared = HashSet::<u16>::with_capacity(table.primary_key_cols.len());
    for &declared_id in &table.primary_key_cols {
        if !declared.insert(declared_id) {
            return Err(PrimaryKeyError::DuplicateColumnId(declared_id));
        }
    }

    let mut parts = Vec::with_capacity(table.primary_key_cols.len());
    for (column_index, column) in table.columns.iter().enumerate() {
        let declared_id = u16::try_from(column.col_id).map_err(|_| PrimaryKeyError::ColumnIdOutOfRange { column_id: column.col_id })?;
        if declared.contains(&declared_id) {
            let raw_value = row.get(column_index).ok_or(PrimaryKeyError::MissingValue { column_id: declared_id })?;
            let value = typed_sql_value_from_json(column, raw_value)?;
            parts.push(PrimaryKeyPart { column_id: declared_id, value });
        }
    }

    for &declared_id in &table.primary_key_cols {
        let found = table.columns.iter().filter_map(|column| u16::try_from(column.col_id).ok()).any(|id| id == declared_id);
        if !found {
            return Err(PrimaryKeyError::UnknownColumnId(declared_id));
        }
    }

    CompletePrimaryKey::from_schema_ordered_parts(parts)
}
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::write_ops`

Expected: PASS with declared-PK validation resolving schema-order parts and rejecting malformed metadata, type mismatches, malformed tagged values, unrepresentable values, nulls, duplicates, missing values, and out-of-range column IDs before any `WritePlan` can be created. Every PK component is validated against the actual declared `ColumnInfo.col_type`.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/effects/write_ops.rs src/effects/mod.rs
git commit -m "feat: validate table primary keys from raw rows"
```

### Task 4: Shared SQL encoder, WritePlan creation, and confirmation revalidation

**Files:**
- Modify: `src/effects/write_ops.rs`
- Test: `src/effects/write_ops.rs`

- [ ] **Step 1 RED: Write the failing test**

Append these tests to `src/effects/write_ops.rs`:

```rust
#[test]
fn update_sql_uses_original_complete_key_and_u32_column_changes() {
    use crate::state::safety::{ColumnChange, GuidedMutation, QualifiedTable, WritePlanId};

    let table_info = table(vec![2, 7]);
    let key = complete_primary_key_from_table_row(&table_info, &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")]).unwrap();
    let plan = create_write_plan(
        WritePlanId(11),
        QualifiedTable { database: "db".into(), schema: None, table: "inventory".into() },
        key,
        Generations { schema: 1, row: 2, server_context: 3, database: 4 },
        GuidedMutation::Update { changes: vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }] },
    );

    assert_eq!(
        build_update_sql(&plan, &table_info).unwrap(),
        "UPDATE \"inventory\" SET \"name\" = 'Grace' WHERE \"account\" = 42 AND \"region\" = 'eu'"
    );
}

#[test]
fn delete_sql_uses_complete_original_key() {
    use crate::state::safety::{GuidedMutation, QualifiedTable, WritePlanId};

    let table_info = table(vec![7, 2]);
    let key = complete_primary_key_from_table_row(&table_info, &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")]).unwrap();
    let plan = create_write_plan(
        WritePlanId(12),
        QualifiedTable { database: "db".into(), schema: None, table: "inventory".into() },
        key,
        Generations { schema: 1, row: 2, server_context: 3, database: 4 },
        GuidedMutation::Delete,
    );

    assert_eq!(build_delete_sql(&plan, &table_info).unwrap(), "DELETE FROM \"inventory\" WHERE \"account\" = 42 AND \"region\" = 'eu'");
}

#[test]
fn shared_encoder_preserves_identity_connection_bigint_quote_and_identifier_semantics() {
    let identity = type_tag(&serde_json::json!({ "AlgebraicType": { "Identity": {} } })).unwrap();
    let connection = type_tag(&serde_json::json!({ "AlgebraicType": { "ConnectionId": {} } })).unwrap();
    let u128_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "U128": {} } })).unwrap();
    let u256_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "U256": {} } })).unwrap();
    let string_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "String": {} } })).unwrap();

    assert_eq!(json_to_sql_literal(&serde_json::json!({ "Identity": "0x1234" }), &identity).unwrap(), "0x1234");
    assert_eq!(json_to_sql_literal(&serde_json::json!({ "ConnectionId": "0xabcd" }), &connection).unwrap(), "0xabcd");
    assert_eq!(json_to_sql_literal(&serde_json::json!({ "U128": "340282366920938463463374607431768211455" }), &u128_tag).unwrap(), "340282366920938463463374607431768211455");
    assert_eq!(json_to_sql_literal(&serde_json::json!({ "U256": "0x01" }), &u256_tag).unwrap(), "0x01");
    assert_eq!(json_to_sql_literal(&serde_json::json!("O'Brien"), &string_tag).unwrap(), "'O''Brien'");
    assert_eq!(encode_identifier("weird\"name").unwrap(), "\"weird\"\"name\"");
}

#[test]
fn changes_are_validated_against_declared_column_type_before_sql_encoding() {
    let mut table_info = table(vec![7]);
    table_info.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "String": {} } });
    let key = complete_primary_key_from_table_row(&table_info, &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")]).unwrap();
    let plan = create_write_plan(
        WritePlanId(14),
        QualifiedTable { database: "db".into(), schema: None, table: "inventory".into() },
        key,
        Generations { schema: 1, row: 2, server_context: 3, database: 4 },
        GuidedMutation::Update { changes: vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Bool(true) }] },
    );

    assert_eq!(build_update_sql(&plan, &table_info), Err(SqlEncodingError::TypeMismatch { column_id: 9, expected: "String".into() }));
}

#[test]
fn confirmation_revalidation_rejects_stale_row_before_dispatch() {
    use crate::state::safety::{GuidedMutation, MutationOutcome, QualifiedTable, WritePlanId};

    let table_info = table(vec![7]);
    let key = complete_primary_key_from_table_row(&table_info, &[serde_json::json!(42)]).unwrap();
    let plan = create_write_plan(
        WritePlanId(13),
        QualifiedTable { database: "db".into(), schema: None, table: "inventory".into() },
        key.clone(),
        Generations { schema: 1, row: 2, server_context: 3, database: 4 },
        GuidedMutation::Delete,
    );

    assert_eq!(
        revalidate_before_dispatch(&plan, &ConfirmationSnapshot { generations: Generations { schema: 1, row: 99, server_context: 3, database: 4 }, table: &table_info, current_raw_row: &[serde_json::json!(42)], row_locked: false }),
        Err(MutationOutcome::DefinitelyNotSent { reason: "row generation changed before confirmation".into() })
    );
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::write_ops`

Expected: FAIL with compiler errors containing `cannot find function create_write_plan`, `cannot find function build_update_sql`, `cannot find function json_to_sql_literal`, `cannot find function type_tag`, or `cannot find function revalidate_before_dispatch`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this code to `src/effects/write_ops.rs` below the PK conversion helpers. Move or reuse the existing tested `type_tag` and `json_to_sql_literal` semantics here as the one real encoder; remove divergent app-local literal helpers. `build_update_sql`, `build_delete_sql`, confirmation revalidation, and app integration must all call this encoder and validate every `PrimaryKeyPart` and `ColumnChange` against its declared `ColumnInfo.col_type` before dispatch:

```rust
use crate::api::types::{ColumnInfo, TableInfo};
use crate::state::safety::{ColumnChange, CompletePrimaryKey, GuidedMutation, MutationOutcome, QualifiedTable, WritePlan, WritePlanId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeTag(String);

pub fn type_tag(declared_type: &serde_json::Value) -> Result<TypeTag, SqlEncodingError> {
    // Parse the existing AlgebraicType JSON shape into stable tags such as Bool, I64, U64, String, Identity, ConnectionId, U128, and U256.
    // Unknown or malformed declarations return TypeMismatch with the column-specific expected type at call sites.
}

pub fn json_to_sql_literal(raw_value: &serde_json::Value, declared_type: &TypeTag) -> Result<String, SqlEncodingError> {
    // Preserve existing semantics for Identity, ConnectionId, U128, U256, strings, bools, numbers, bytes, and null rejection.
    // Reject JSON values that do not match the declared type, including non-finite or unrepresentable values.
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Generations { pub schema: u64, pub row: u64, pub server_context: u64, pub database: u64 }

#[derive(Clone, Debug)]
pub struct ConfirmationSnapshot<'a> { pub generations: Generations, pub table: &'a TableInfo, pub current_raw_row: &'a [serde_json::Value], pub row_locked: bool }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqlEncodingError {
    EmptyIdentifier,
    NulInIdentifier,
    UnknownColumnId(u32),
    NullLiteralRejected,
    FloatNotFinite,
    WrongMutationKind,
    NoChangedFields,
    TypeMismatch { column_id: u32, expected: String },
}

pub fn create_write_plan(id: WritePlanId, table: QualifiedTable, original_primary_key: CompletePrimaryKey, generations: Generations, mutation: GuidedMutation) -> WritePlan { /* unchanged field mapping */ }

pub fn encode_identifier(identifier: &str) -> Result<String, SqlEncodingError> { /* quote with doubled embedded quotes and reject empty/NUL */ }

fn column(table: &TableInfo, column_id: u32) -> Result<&ColumnInfo, SqlEncodingError> { /* find by u32 col_id */ }

fn encode_key_predicate(key: &CompletePrimaryKey, table: &TableInfo) -> Result<String, SqlEncodingError> { /* lookup ColumnInfo, use type_tag(column.col_type), then json_to_sql_literal */ }

fn validate_column_changes_against_table(table: &TableInfo, changes: &[ColumnChange]) -> Result<(), SqlEncodingError> { /* verify each change column exists and new_value matches declared ColumnInfo.col_type */ }

pub fn build_update_sql(plan: &WritePlan, table: &TableInfo) -> Result<String, SqlEncodingError> { /* validate changes, encode assignments and complete-key WHERE using shared encoder */ }

pub fn build_delete_sql(plan: &WritePlan, table: &TableInfo) -> Result<String, SqlEncodingError> { /* encode complete-key WHERE using shared encoder */ }

pub fn revalidate_before_dispatch(plan: &WritePlan, snapshot: &ConfirmationSnapshot<'_>) -> Result<(), MutationOutcome> { /* preserve generation checks and reconstruct typed key from current raw values before transport ownership */ }
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::write_ops`

Expected: PASS with SQL construction and confirmation revalidation tests passing, using the shared type-correct encoder for declared numeric/bool/string values, non-finite rejection, identity and connection IDs with correct `0x` form, U128/U256 tagged values, quote escaping, and identifier escaping. App integration must call the same encoder and must not keep divergent SQL literal helpers.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/effects/write_ops.rs
git commit -m "feat: build and revalidate guided write plans"
```

### Task 5: Pure guided write builder policy and app integration

**Files:**
- Modify: `src/effects/write_ops.rs`
- Modify: `src/app.rs`
- Modify: `src/state/modal.rs`
- Test: `src/effects/write_ops.rs`

- [ ] **Step 1 RED: Write the failing test**

Append these tests to `src/effects/write_ops.rs`. These tests call the real pure builder API with `TableInfo`, raw `&[serde_json::Value]`, explicit generation inputs, and `ColumnChange` values. The builder validates every PK component and every `ColumnChange` against declared `ColumnInfo.col_type` before returning a `WritePlan`. They do not use test-only `App` methods.

```rust
#[test]
fn build_guided_update_plan_rejects_table_without_declared_pk_even_when_id_like_column_exists() {
    let mut no_key = table(vec![]);
    no_key.columns[0].col_name = "id".into();

    let result = build_guided_update_plan(
        WritePlanId(20),
        QualifiedTable { database: "db".into(), schema: None, table: no_key.table_name.clone() },
        &no_key,
        &[serde_json::json!(1), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    );

    assert_eq!(result.unwrap_err(), WritePlanBuildError::PrimaryKey(PrimaryKeyError::NoDeclaredPrimaryKey));
}

#[test]
fn build_guided_update_plan_uses_actual_tableinfo_raw_row_and_generations() {
    let table_info = table(vec![2, 7]);
    let plan = build_guided_update_plan(
        WritePlanId(21),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(1), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 3, row: 4, server_context: 5, database: 6 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    ).unwrap();

    assert_eq!(plan.schema_generation, 3);
    assert_eq!(plan.row_generation, 4);
    assert_eq!(plan.server_context_generation, 5);
    assert_eq!(plan.database_generation, 6);
    assert_eq!(plan.original_primary_key.parts()[0].column_id, 7);
    assert_eq!(plan.original_primary_key.parts()[1].column_id, 2);
}

#[test]
fn build_guided_delete_plan_reuses_same_complete_key_policy() {
    let table_info = table(vec![7]);
    let plan = build_guided_delete_plan(
        WritePlanId(22),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(1), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 3, row: 4, server_context: 5, database: 6 },
    ).unwrap();

    assert!(matches!(plan.mutation, GuidedMutation::Delete));
    assert_eq!(plan.original_primary_key.parts().len(), 1);
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::write_ops::tests::build_guided`

Expected: FAIL with compiler errors containing `cannot find function build_guided_update_plan`, `cannot find function build_guided_delete_plan`, or `cannot find type WritePlanBuildError`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this pure API to `src/effects/write_ops.rs`:

```rust
use crate::state::safety::{PrimaryKeyError, WritePlan};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WritePlanBuildError {
    PrimaryKey(PrimaryKeyError),
    NoChangedFields,
    TypeMismatch { column_id: u32, expected: String },
    Encoding(SqlEncodingError),
}

pub fn build_guided_update_plan(
    id: WritePlanId,
    qualified_table: QualifiedTable,
    table: &TableInfo,
    selected_raw_row: &[serde_json::Value],
    generations: Generations,
    changes: Vec<ColumnChange>,
) -> Result<WritePlan, WritePlanBuildError> {
    validate_column_changes_against_table(table, &changes).map_err(|error| match error { SqlEncodingError::TypeMismatch { column_id, expected } => WritePlanBuildError::TypeMismatch { column_id, expected }, other => WritePlanBuildError::Encoding(other) })?;
    if changes.is_empty() {
        return Err(WritePlanBuildError::NoChangedFields);
    }
    let key = complete_primary_key_from_table_row(table, selected_raw_row).map_err(WritePlanBuildError::PrimaryKey)?;
    Ok(create_write_plan(id, qualified_table, key, generations, GuidedMutation::Update { changes }))
}

pub fn build_guided_delete_plan(
    id: WritePlanId,
    qualified_table: QualifiedTable,
    table: &TableInfo,
    selected_raw_row: &[serde_json::Value],
    generations: Generations,
) -> Result<WritePlan, WritePlanBuildError> {
    let key = complete_primary_key_from_table_row(table, selected_raw_row).map_err(WritePlanBuildError::PrimaryKey)?;
    Ok(create_write_plan(id, qualified_table, key, generations, GuidedMutation::Delete))
}
```

Modify `src/state/modal.rs` to carry stable write plan IDs:

```rust
use crate::state::safety::WritePlanId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafetyModalAction {
    ConfirmWritePlan { plan_id: WritePlanId },
    DirtyRowChoice { requested_row: usize, requested_column: u32 },
}
```

Modify `src/app.rs` integration only to gather current data and call the pure API. The app must not infer PKs and must not own a test-only builder. Use the existing selected table and raw row sources:

```rust
let Some(table_info) = self.state.selected_table().cloned() else {
    self.state.set_notification("No table selected".to_string());
    return;
};
let Some(data_idx) = self.active_data_row_index() else {
    self.state.set_notification("No row selected".to_string());
    return;
};
let Some(selected_raw_row) = self.state.table_browse_result.as_ref().and_then(|result| result.rows.get(data_idx)) else {
    self.state.set_notification("No row selected".to_string());
    return;
};
let generations = Generations {
    schema: self.state.schema_generation,
    row: self.state.table_generation,
    server_context: self.state.server_context_generation,
    database: self.state.database_generation,
};
let qualified = QualifiedTable {
    database: self.state.selected_database().unwrap_or_default().to_string(),
    schema: None,
    table: table_info.table_name.clone(),
};
let plan = build_guided_update_plan(self.next_write_plan_id(), qualified, &table_info, selected_raw_row, generations, changes);
```

For delete availability and modal creation, call `build_guided_delete_plan` with the same gathered `TableInfo`, raw row, and generations. On `WritePlanBuildError::PrimaryKey`, show a disabled reason in the modal/Inspector path. Store the returned `WritePlan` under its `WritePlanId` and put `SafetyModalAction::ConfirmWritePlan { plan_id }` in modal state.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::write_ops::tests::build_guided`

Expected: PASS with no declared-key heuristic and no first-column fallback. The pure builder uses `TableInfo.primary_key_cols`, raw row values, and explicit generations.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/state/safety.rs src/effects/write_ops.rs src/app.rs src/state/modal.rs
git commit -m "fix: integrate pure guided write plan builder"
```

### Task 6: One-row spreadsheet aggregation and Save/Discard/Stay modal

**Files:**
- Modify: `src/state/edit_mode.rs`
- Modify: `src/state/modal.rs`
- Modify: `src/app.rs`
- Test: `src/state/edit_mode.rs`

- [ ] **Step 1 RED: Write the failing test**

Add these tests to `src/state/edit_mode.rs`:

```rust
#[cfg(test)]
mod one_row_spreadsheet_tests {
    use super::*;
    use crate::state::safety::{CompletePrimaryKey, PrimaryKeyPart, SqlValue};

    fn key(value: u64) -> CompletePrimaryKey {
        CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart { column_id: 7, value: SqlValue::U64(value) }]).unwrap()
    }

    fn target(value: u64, row_generation: u64) -> EditRowTarget {
        EditRowTarget { primary_key: key(value), row_generation, data_row_index: usize::try_from(value).unwrap() }
    }

    #[test]
    fn changed_cells_in_one_stable_row_aggregate_into_one_write_plan_input() {
        let mut edits = SpreadsheetEdits::default();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("old".into()), SqlValue::Text("A".into())).unwrap();
        edits.set_cell(target(1, 10), 2, SqlValue::I64(8), SqlValue::I64(9)).unwrap();

        let save = edits.pending_single_row_save().unwrap();

        assert_eq!(save.target, target(1, 10));
        assert_eq!(save.changes.len(), 2);
        assert_eq!(save.changes[0].old_value, Some(SqlValue::Text("old".into())));
        assert_eq!(save.changes[0].new_value, SqlValue::Text("A".into()));
    }

    #[test]
    fn editing_same_cell_twice_preserves_first_old_value() {
        let mut edits = SpreadsheetEdits::default();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("original".into()), SqlValue::Text("first".into())).unwrap();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("first".into()), SqlValue::Text("second".into())).unwrap();

        let save = edits.pending_single_row_save().unwrap();

        assert_eq!(save.changes.len(), 1);
        assert_eq!(save.changes[0].old_value, Some(SqlValue::Text("original".into())));
        assert_eq!(save.changes[0].new_value, SqlValue::Text("second".into()));
    }

    #[test]
    fn identical_display_text_does_not_collapse_different_raw_keys() {
        let mut edits = SpreadsheetEdits::default();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("old".into()), SqlValue::Text("A".into())).unwrap();

        let result = edits.begin_edit(target(2, 10), 1);

        assert_eq!(result, BeginEditResult::NeedsDirtyRowChoice { dirty_row: target(1, 10), requested_row: target(2, 10), requested_column: 1 });
    }

    #[test]
    fn scope_or_generation_change_fails_before_revalidation_or_dispatch() {
        let mut edits = SpreadsheetEdits::default();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("old".into()), SqlValue::Text("A".into())).unwrap();

        assert_eq!(edits.pending_single_row_save_for_current_target(&target(1, 11)), Err(SpreadsheetEditError::DefinitelyNotSent { reason: "row generation changed before save".into() }));
    }

    #[test]
    fn multi_row_save_all_is_unavailable_in_phase_one() {
        let mut edits = SpreadsheetEdits::default();
        edits.set_cell(target(1, 10), 1, SqlValue::Text("old".into()), SqlValue::Text("A".into())).unwrap();

        assert_eq!(edits.save_all(), Err(SpreadsheetEditError::MultiRowSaveAllDisabled));
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test state::edit_mode::one_row_spreadsheet_tests`

Expected: FAIL with compiler errors containing `cannot find type SpreadsheetEdits`, `cannot find type EditRowTarget`, old display-string row identity, or old multi-row save behavior succeeding.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add these types and methods to `src/state/edit_mode.rs` while adapting existing `EditMode.pending` to call them. Visual display text is not identity; the edit target is the raw complete primary key plus current row generation and underlying data row identity:

```rust
use std::collections::BTreeMap;
use crate::state::safety::{CompletePrimaryKey, SqlValue};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditRowTarget {
    pub primary_key: CompletePrimaryKey,
    pub row_generation: u64,
    pub data_row_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CellChange {
    pub column_id: u32,
    pub old_value: Option<SqlValue>,
    pub new_value: SqlValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SingleRowSave {
    pub target: EditRowTarget,
    pub changes: Vec<CellChange>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BeginEditResult {
    Began,
    NeedsDirtyRowChoice { dirty_row: EditRowTarget, requested_row: EditRowTarget, requested_column: u32 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpreadsheetEditError {
    MultiRowSaveAllDisabled,
    DefinitelyNotSent { reason: String },
}

#[derive(Default)]
pub struct SpreadsheetEdits {
    dirty_target: Option<EditRowTarget>,
    changes: BTreeMap<u32, CellChange>,
}

impl SpreadsheetEdits {
    pub fn set_cell(&mut self, target: EditRowTarget, column_id: u32, old_value: SqlValue, new_value: SqlValue) -> Result<(), SpreadsheetEditError> {
        match &self.dirty_target {
            Some(existing) if existing != &target => Err(SpreadsheetEditError::MultiRowSaveAllDisabled),
            None => {
                self.dirty_target = Some(target);
                self.changes.insert(column_id, CellChange { column_id, old_value: Some(old_value), new_value });
                Ok(())
            }
            Some(_) => {
                self.changes
                    .entry(column_id)
                    .and_modify(|change| change.new_value = new_value.clone())
                    .or_insert(CellChange { column_id, old_value: Some(old_value), new_value });
                Ok(())
            }
        }
    }

    pub fn begin_edit(&self, target: EditRowTarget, column_id: u32) -> BeginEditResult {
        match &self.dirty_target {
            Some(existing) if existing != &target => BeginEditResult::NeedsDirtyRowChoice { dirty_row: existing.clone(), requested_row: target, requested_column: column_id },
            _ => BeginEditResult::Began,
        }
    }

    pub fn pending_single_row_save_for_current_target(&self, current_target: &EditRowTarget) -> Result<SingleRowSave, SpreadsheetEditError> {
        let Some(target) = &self.dirty_target else { return Err(SpreadsheetEditError::DefinitelyNotSent { reason: "no dirty row".into() }); };
        if target.primary_key != current_target.primary_key || target.data_row_index != current_target.data_row_index {
            return Err(SpreadsheetEditError::DefinitelyNotSent { reason: "row scope changed before save".into() });
        }
        if target.row_generation != current_target.row_generation {
            return Err(SpreadsheetEditError::DefinitelyNotSent { reason: "row generation changed before save".into() });
        }
        Ok(SingleRowSave { target: target.clone(), changes: self.changes.values().cloned().collect() })
    }

    pub fn pending_single_row_save(&self) -> Option<SingleRowSave> {
        self.dirty_target.clone().map(|target| SingleRowSave { target, changes: self.changes.values().cloned().collect() })
    }

    pub fn discard(&mut self) {
        self.dirty_target = None;
        self.changes.clear();
    }

    pub fn save_all(&self) -> Result<(), SpreadsheetEditError> {
        Err(SpreadsheetEditError::MultiRowSaveAllDisabled)
    }
}
```

Modify `src/state/modal.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirtyRowChoice {
    Save,
    Discard,
    Stay,
}
```

Modify `src/app.rs` row-change handling so `BeginEditResult::NeedsDirtyRowChoice` opens a modal with Save, Discard, and Stay. Save calls `pending_single_row_save_for_current_target` with the current raw complete key, row generation, and data row index. If scope or generation changed, record `MutationOutcome::DefinitelyNotSent` and do not call revalidation or dispatch. Otherwise convert its captured `CellChange { old_value, new_value }` values into one `WritePlan` through `build_guided_update_plan`. Do not derive old values from a later cache snapshot. If the same cell is edited repeatedly before save, preserve the original old_value captured on the first edit and update only new_value. Discard clears the row edits and starts the requested edit. Stay closes the modal and leaves focus on the dirty row.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test state::edit_mode::one_row_spreadsheet_tests`

Expected: PASS with stable typed row identity, one-row aggregation into a single `WritePlan`, captured old/new cell values, Save/Discard/Stay gating, and disabled Phase 1 multi-row Save All.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/state/edit_mode.rs src/state/modal.rs src/app.rs
git commit -m "feat: constrain spreadsheet saves to one row"
```

### Task 7: Authoritative mutation outcome verification and no automatic retry for Unknown

**Files:**
- Modify: `src/state/safety.rs`
- Modify: `src/effects/write_ops.rs`
- Modify: `src/app.rs`
- Test: `src/effects/write_ops.rs`

- [ ] **Step 1 RED: Write the failing test**

Append these tests to `src/effects/write_ops.rs`. They exercise the same real verifier functions used by app integration, with no test-only production seams. The affected-row fixture is local to the test module and is not a production constructor:

```rust
fn request_from_affected_rows(plan_id: WritePlanId, affected_rows: Option<u64>) -> WriteVerificationRequest {
    let key = CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart { column_id: 7, value: SqlValue::U64(42) }]).unwrap();
    WriteVerificationRequest {
        plan: WritePlan {
            id: plan_id,
            table: QualifiedTable { database: "db".into(), schema: None, table: "inventory".into() },
            original_primary_key: key,
            schema_generation: 1,
            row_generation: 1,
            server_context_generation: 1,
            database_generation: 1,
            mutation: GuidedMutation::Delete,
        },
        affected_rows,
        postcondition: None,
    }
}

#[test]
fn authoritative_affected_row_count_classifies_success_not_found_and_critical_overwrite() {
    assert_eq!(
        verify_authoritative_mutation_result(&request_from_affected_rows(WritePlanId(30), Some(1))).outcome,
        MutationOutcome::SentAndConfirmed { affected_rows: Some(1) }
    );
    let not_found = verify_authoritative_mutation_result(&request_from_affected_rows(WritePlanId(31), Some(0)));
    assert_eq!(
        not_found.outcome,
        MutationOutcome::Conflict { reason: "mutation matched no rows; target was not found or changed".into() }
    );
    assert!(not_found.mark_resource_stale);
    assert_eq!(not_found.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);

    let report = verify_authoritative_mutation_result(&request_from_affected_rows(WritePlanId(32), Some(2)));

    assert_eq!(report.outcome, MutationOutcome::CriticalSafetyError { reason: "mutation affected 2 rows; expected exactly 1".into() });
    assert!(report.mark_resource_stale);
    assert_eq!(report.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
}

#[test]
fn unavailable_affected_count_requires_explicit_postconditions_for_normal_update() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(33),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update {
            table: table_info,
            row_by_original_key: Some(vec![serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Grace")]),
            row_by_new_key: None,
            old_key_still_present: None,
        }),
    });

    assert_eq!(report.outcome, MutationOutcome::SentAndConfirmed { affected_rows: None });
}

#[test]
fn primary_key_update_requires_new_tuple_fields_and_old_tuple_absent() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(34),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 7, old_value: Some(SqlValue::U64(42)), new_value: SqlValue::U64(43) }],
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update {
            table: table_info,
            row_by_original_key: None,
            row_by_new_key: Some(vec![serde_json::json!(43), serde_json::json!("eu"), serde_json::json!("Ada")]),
            old_key_still_present: Some(false),
        }),
    });

    assert_eq!(report.outcome, MutationOutcome::SentAndConfirmed { affected_rows: None });
}

#[test]
fn delete_requires_original_tuple_absent_when_count_unavailable() {
    let table_info = table(vec![7]);
    let plan = build_guided_delete_plan(
        WritePlanId(35),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Delete { original_tuple_present: false }),
    });

    assert_eq!(report.outcome, MutationOutcome::SentAndConfirmed { affected_rows: None });
}

#[test]
fn missing_changed_field_in_verified_update_row_is_unknown() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(36),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update { table: table_info, row_by_original_key: Some(vec![serde_json::json!(42)]), row_by_new_key: None, old_key_still_present: None }),
    });

    assert!(matches!(report.outcome, MutationOutcome::Unknown { .. }));
    assert!(report.mark_resource_stale);
}

#[test]
fn mismatched_changed_field_in_verified_update_row_is_conflict() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(37),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update { table: table_info, row_by_original_key: Some(vec![serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")]), row_by_new_key: None, old_key_still_present: None }),
    });

    assert_eq!(report.outcome, MutationOutcome::Conflict { reason: "postcondition mismatch for column 9".into() });
    assert!(report.mark_resource_stale);
    assert_eq!(report.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
}

#[test]
fn explicit_missing_original_row_for_normal_update_is_conflict() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(38),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 9, old_value: Some(SqlValue::Text("Ada".into())), new_value: SqlValue::Text("Grace".into()) }],
    ).unwrap();

    let report = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update { table: table_info, row_by_original_key: None, row_by_new_key: None, old_key_still_present: None }),
    });

    assert_eq!(report.outcome, MutationOutcome::Conflict { reason: "updated tuple was not found by original key during verification".into() });
    assert!(report.mark_resource_stale);
    assert_eq!(report.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
}

#[test]
fn primary_key_update_old_presence_policy_distinguishes_conflict_from_unknown() {
    let table_info = table(vec![7]);
    let plan = build_guided_update_plan(
        WritePlanId(39),
        QualifiedTable { database: "db".into(), schema: None, table: table_info.table_name.clone() },
        &table_info,
        &[serde_json::json!(42), serde_json::json!("eu"), serde_json::json!("Ada")],
        Generations { schema: 1, row: 1, server_context: 1, database: 1 },
        vec![ColumnChange { column_id: 7, old_value: Some(SqlValue::U64(42)), new_value: SqlValue::U64(43) }],
    ).unwrap();

    let old_present = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan: plan.clone(),
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update { table: table_info.clone(), row_by_original_key: None, row_by_new_key: Some(vec![serde_json::json!(43), serde_json::json!("eu"), serde_json::json!("Ada")]), old_key_still_present: Some(true) }),
    });
    let old_not_checked = verify_authoritative_mutation_result(&WriteVerificationRequest {
        plan,
        affected_rows: None,
        postcondition: Some(PostconditionEvidence::Update { table: table_info, row_by_original_key: None, row_by_new_key: Some(vec![serde_json::json!(43), serde_json::json!("eu"), serde_json::json!("Ada")]), old_key_still_present: None }),
    });

    assert_eq!(old_present.outcome, MutationOutcome::Conflict { reason: "old primary-key tuple is still present after verification".into() });
    assert_eq!(old_present.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
    assert!(matches!(old_not_checked.outcome, MutationOutcome::Unknown { .. }));
    assert_eq!(old_not_checked.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
}

#[test]
fn absent_postcondition_evidence_is_unknown_and_disables_guided_writes() {
    let report = verify_authoritative_mutation_result(&request_from_affected_rows(WritePlanId(40), None));

    assert!(matches!(report.outcome, MutationOutcome::Unknown { .. }));
    assert!(report.mark_resource_stale);
    assert_eq!(report.guided_write_availability, GuidedWriteAvailability::DisabledUntilRefresh);
}

#[test]
fn non_authoritative_result_after_transport_ownership_is_unknown_and_not_retryable() {
    for failure in [TransportFailure::Timeout, TransportFailure::Disconnected, TransportFailure::Cancelled, TransportFailure::Http5xx(500)] {
        let outcome = classify_mutation_error(MutationDispatchStage::AfterTransportOwnership, failure.clone());
        assert_eq!(outcome, MutationOutcome::Unknown { reason: failure.to_string() });
        assert!(!can_auto_retry_mutation(&outcome));
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::write_ops::tests::authoritative_affected_row_count_classifies_success_not_found_and_critical_overwrite`

Expected: FAIL with compiler errors containing `cannot find function verify_authoritative_mutation_result`, `cannot find type PostconditionEvidence`, `cannot find type WriteVerificationRequest`, or `cannot find variant CriticalSafetyError`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add `CriticalSafetyError` to `MutationOutcome` in `src/state/safety.rs`, preserving the existing variants:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationOutcome {
    DefinitelyNotSent { reason: String },
    SentAndConfirmed { affected_rows: Option<u64> },
    Conflict { reason: String },
    Unknown { reason: String },
    CriticalSafetyError { reason: String },
}
```

Add the real verifier API to `src/effects/write_ops.rs` without weakening confirmation-time `revalidate_before_dispatch` from Task 4. Confirmation still reconstructs the key and checks generations before any transport ownership; this verifier runs only after transport returns an authoritative response or after explicit postcondition re-fetches:

```rust
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutationDispatchStage { BeforeTransportOwnership, AfterTransportOwnership }

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportFailure { Validation(String), Timeout, Disconnected, Cancelled, Http5xx(u16), ProvenNotSent(String) }

impl fmt::Display for TransportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(reason) | Self::ProvenNotSent(reason) => write!(f, "{reason}"),
            Self::Timeout => write!(f, "mutation timed out after transport ownership"),
            Self::Disconnected => write!(f, "connection dropped after transport ownership"),
            Self::Cancelled => write!(f, "mutation cancelled after transport ownership"),
            Self::Http5xx(status) => write!(f, "server returned HTTP {status} after transport ownership"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuidedWriteAvailability { Enabled, DisabledUntilRefresh }

#[derive(Clone, Debug)]
pub enum PostconditionEvidence {
    Update { table: TableInfo, row_by_original_key: Option<Vec<serde_json::Value>>, row_by_new_key: Option<Vec<serde_json::Value>>, old_key_still_present: Option<bool> },
    Delete { original_tuple_present: bool },
}

#[derive(Clone, Debug)]
pub struct WriteVerificationRequest { pub plan: WritePlan, pub affected_rows: Option<u64>, pub postcondition: Option<PostconditionEvidence> }


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteVerificationReport { pub outcome: MutationOutcome, pub mark_resource_stale: bool, pub guided_write_availability: GuidedWriteAvailability }

pub fn classify_mutation_error(stage: MutationDispatchStage, failure: TransportFailure) -> MutationOutcome {
    match (stage, failure) {
        (_, TransportFailure::ProvenNotSent(reason)) => MutationOutcome::DefinitelyNotSent { reason },
        (MutationDispatchStage::BeforeTransportOwnership, failure) => MutationOutcome::DefinitelyNotSent { reason: failure.to_string() },
        (MutationDispatchStage::AfterTransportOwnership, failure) => MutationOutcome::Unknown { reason: failure.to_string() },
    }
}

pub fn can_auto_retry_mutation(outcome: &MutationOutcome) -> bool {
    matches!(outcome, MutationOutcome::DefinitelyNotSent { .. })
}

pub fn verify_authoritative_mutation_result(request: &WriteVerificationRequest) -> WriteVerificationReport {
    if let Some(affected_rows) = request.affected_rows {
        return match affected_rows {
            1 => confirmed(Some(1)),
            0 => conflict("mutation matched no rows; target was not found or changed"),
            count => critical(format!("mutation affected {count} rows; expected exactly 1")),
        };
    }
    match (&request.plan.mutation, &request.postcondition) {
        (GuidedMutation::Update { changes }, Some(PostconditionEvidence::Update { table, row_by_original_key, row_by_new_key, old_key_still_present })) => verify_update_postcondition(&request.plan, changes, table, row_by_original_key.as_deref(), row_by_new_key.as_deref(), *old_key_still_present),
        (GuidedMutation::Delete, Some(PostconditionEvidence::Delete { original_tuple_present: false })) => confirmed(None),
        (GuidedMutation::Delete, Some(PostconditionEvidence::Delete { original_tuple_present: true })) => conflict("deleted tuple is still present after verification"),
        _ => unknown("mutation result was not authoritative and no explicit postcondition verification was available"),
    }
}
```

Add these verifier helpers in the same module:

```rust
fn verify_update_postcondition(
    plan: &WritePlan,
    changes: &[ColumnChange],
    table: &TableInfo,
    row_by_original_key: Option<&[serde_json::Value]>,
    row_by_new_key: Option<&[serde_json::Value]>,
    old_key_still_present: Option<bool>,
) -> WriteVerificationReport {
    let primary_key_changed = changes.iter().any(|change| {
        plan.original_primary_key.parts().iter().any(|part| u32::from(part.column_id) == change.column_id)
    });
    if primary_key_changed {
        match old_key_still_present {
            Some(true) => return conflict("old primary-key tuple is still present after verification"),
            None => return unknown("old primary-key tuple absence was not checked"),
            Some(false) => {}
        }
        let Some(new_row) = row_by_new_key else {
            return conflict("updated tuple was not found by new primary key during verification");
        };
        return verify_changed_fields(table, new_row, changes);
    }
    let Some(current_row) = row_by_original_key else {
        return conflict("updated tuple was not found by original key during verification");
    };
    verify_changed_fields(table, current_row, changes)
}

fn verify_changed_fields(table: &TableInfo, row: &[serde_json::Value], changes: &[ColumnChange]) -> WriteVerificationReport {
    for change in changes {
        let Some((column_index, column)) = table.columns.iter().enumerate().find(|(_, column)| column.col_id == change.column_id) else {
            return unknown(format!("changed column {} is not in current schema", change.column_id));
        };
        let Some(raw_value) = row.get(column_index) else {
            return unknown(format!("verified row is missing changed column {}", change.column_id));
        };
        let Ok(actual) = typed_sql_value_from_json(column, raw_value) else {
            return unknown(format!("verified row has malformed value for changed column {}", change.column_id));
        };
        if actual != change.new_value {
            return conflict(format!("postcondition mismatch for column {}", change.column_id));
        }
    }
    confirmed(None)
}

fn confirmed(affected_rows: Option<u64>) -> WriteVerificationReport {
    WriteVerificationReport { outcome: MutationOutcome::SentAndConfirmed { affected_rows }, mark_resource_stale: false, guided_write_availability: GuidedWriteAvailability::Enabled }
}

fn conflict(reason: impl Into<String>) -> WriteVerificationReport {
    WriteVerificationReport { outcome: MutationOutcome::Conflict { reason: reason.into() }, mark_resource_stale: true, guided_write_availability: GuidedWriteAvailability::DisabledUntilRefresh }
}

fn unknown(reason: impl Into<String>) -> WriteVerificationReport {
    WriteVerificationReport { outcome: MutationOutcome::Unknown { reason: reason.into() }, mark_resource_stale: true, guided_write_availability: GuidedWriteAvailability::DisabledUntilRefresh }
}

fn critical(reason: impl Into<String>) -> WriteVerificationReport {
    WriteVerificationReport { outcome: MutationOutcome::CriticalSafetyError { reason: reason.into() }, mark_resource_stale: true, guided_write_availability: GuidedWriteAvailability::DisabledUntilRefresh }
}
```

Modify `src/app.rs` mutation handling so the app uses the same real `verify_authoritative_mutation_result` verifier after every transport-owned mutation. If the server returns an authoritative affected-row count, pass it directly. If the count is unavailable, re-fetch explicit postconditions before reporting success: normal update re-fetches the row by the original complete key and compares every intended changed field; primary-key update re-fetches the new complete tuple, compares every intended changed field, and proves the old tuple is absent; delete proves the original tuple is absent. Timeouts, disconnects, cancellations, HTTP 5xx, and any other non-authoritative result after transport ownership call `classify_mutation_error(MutationDispatchStage::AfterTransportOwnership, failure)` and record `Unknown`. Do not enqueue any retry when `can_auto_retry_mutation` returns false. A local validation/construction failure before dispatch records `DefinitelyNotSent`. If verification returns `CriticalSafetyError`, mark the resource stale, disable guided writes for that resource until refresh, and add an Activity item requiring manual refresh before any further guided mutation.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::write_ops::tests::authoritative_affected_row_count_classifies_success_not_found_and_critical_overwrite`

Expected: PASS with authoritative verification for exact affected-row counts and explicit postconditions, `CriticalSafetyError` disabling guided writes until refresh, and `Unknown` outcomes never auto-retryable.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/state/safety.rs src/effects/write_ops.rs src/app.rs
git commit -m "feat: verify authoritative mutation outcomes"
```

### Task 8: Apply `RequestContext` stale-result guards in existing async paths

**Files:**
- Modify: `src/app.rs`
- Modify: `src/effects/request.rs`
- Test: `src/app.rs`

- [ ] **Step 1 RED: Write the failing test**

Add these pure event-application tests to `src/app.rs` near existing app tests:

```rust
#[derive(Default)]
struct RequestOwner {
    next_request_id: u64,
    request_generations: std::collections::HashMap<RequestScope, u64>,
    latest_requests: std::collections::HashMap<RequestScope, RequestContext>,
}

impl RequestOwner {
    fn next_request_context(&mut self, scope: RequestScope) -> RequestContext {
        self.next_request_id = self.next_request_id.saturating_add(1);
        let generation = self.request_generations.entry(scope.clone()).and_modify(|value| *value = value.saturating_add(1)).or_insert(1);
        let context = RequestContext::new(RequestId::from_u64(self.next_request_id), scope, *generation);
        self.latest_requests.insert(context.scope().clone(), context.clone());
        context
    }
}

#[test]
fn stale_table_rows_result_cannot_replace_newer_active_scope() {
    let mut latest = std::collections::HashMap::new();
    let current = RequestContext::new(
        RequestId::from_u64(2),
        RequestScope::TableRows { database: "db".into(), table: "inventory".into(), view: "main".into() },
        10,
    );
    latest.insert(current.scope.clone(), current.clone());
    let stale = RequestContext::new(RequestId::from_u64(1), current.scope.clone(), 9);

    assert!(!should_apply_result(&latest, &stale));
    assert!(should_apply_result(&latest, &current));
}

#[test]
fn next_request_context_increments_id_and_scope_generation() {
    let mut owner = RequestOwner::default();
    let first = owner.next_request_context(RequestScope::TableRows { database: "db".into(), table: "inventory".into(), view: "main".into() });
    let second = owner.next_request_context(RequestScope::TableRows { database: "db".into(), table: "inventory".into(), view: "main".into() });

    assert_ne!(first.id, second.id);
    assert_eq!(first.generation, 1);
    assert_eq!(second.generation, 2);
    assert!(should_apply_result(&owner.latest_requests, &second));
    assert!(!should_apply_result(&owner.latest_requests, &first));
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test app::stale_table_rows_result_cannot_replace_newer_active_scope`

Expected: FAIL because old result events do not carry `RequestContext`, the pure `should_apply_result` function does not exist, or the minimal request owner has no `next_request_context` method.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add `RequestContext` to existing async result variants in `src/app.rs` as an additive migration: preserve all existing event variants and payload fields, and only add context to database list, schema, table rows, SQL workspace, logs, metrics, and live-client metadata events that represent async read results:

```diff
 pub enum AppEvent {
+    // Existing input, tick, terminal, WebSocket, mutation, and UI variants stay unchanged.
-    DatabasesLoaded { databases: Vec<String> },
+    DatabasesLoaded { context: RequestContext, databases: Vec<String> },
-    SchemaLoaded { schema: crate::api::types::SchemaResponse },
+    SchemaLoaded { context: RequestContext, schema: crate::api::types::SchemaResponse },
-    SchemaError { error: String },
+    SchemaError { context: RequestContext, error: String },
-    TableBrowseResult { result: crate::api::types::QueryResult },
+    TableBrowseResult { context: RequestContext, result: crate::api::types::QueryResult },
-    TableBrowseError { error: String },
+    TableBrowseError { context: RequestContext, error: String },
-    QueryResult { result: crate::api::types::QueryResult, duration: Duration, sql: String },
+    QueryResult { context: RequestContext, result: crate::api::types::QueryResult, duration: Duration, sql: String },
-    QueryError { sql: String, error: String },
+    QueryError { context: RequestContext, sql: String, error: String },
-    LogsLoaded { logs: Vec<crate::api::types::LogEntry> },
+    LogsLoaded { context: RequestContext, logs: Vec<crate::api::types::LogEntry> },
-    MetricsLoaded { snapshot: crate::state::MetricsSnapshot },
+    MetricsLoaded { context: RequestContext, snapshot: crate::state::MetricsSnapshot },
-    LiveClientsLoaded { clients: Vec<crate::state::app_state::LiveClientEntry> },
+    LiveClientsLoaded { context: RequestContext, clients: Vec<crate::state::app_state::LiveClientEntry> },
 }
```

Input, tick, terminal, WebSocket, mutation-result, modal, navigation, and quit variants must remain in the enum exactly as they exist today; this migration only adds `context` fields to async read-result variants.

Add the minimal Phase 1 request owner fields and methods to `App` or `AppState`:

```rust
use std::collections::HashMap;
use crate::effects::request::{RequestContext, RequestId, RequestScope};

next_request_id: u64,
request_generations: HashMap<RequestScope, u64>,
latest_requests: HashMap<RequestScope, RequestContext>,
ignored_stale_results: u64,

fn next_request_context(&mut self, scope: RequestScope) -> RequestContext {
    self.next_request_id = self.next_request_id.saturating_add(1);
    let generation = self.request_generations.entry(scope.clone()).and_modify(|value| *value = value.saturating_add(1)).or_insert(1);
    let context = RequestContext::new(RequestId::from_u64(self.next_request_id), scope, *generation);
    self.latest_requests.insert(context.scope().clone(), context.clone());
    context
}

fn should_apply_result(latest: &HashMap<RequestScope, RequestContext>, delivered: &RequestContext) -> bool {
    latest.get(delivered.scope()).is_some_and(|current| current.accepts(delivered))
}

fn apply_result_if_current(&mut self, delivered: &RequestContext) -> bool {
    let should_apply = should_apply_result(&self.latest_requests, delivered);
    if !should_apply {
        self.ignored_stale_results = self.ignored_stale_results.saturating_add(1);
    }
    should_apply
}
```

Every read spawn must create a context with the stable variants from Task 1. Example for table rows:

```rust
let context = self.next_request_context(RequestScope::TableRows { database: db.clone(), table: table.clone(), view: "main".into() });
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test app::stale_table_rows_result_cannot_replace_newer_active_scope`

Expected: PASS with stale result rejection and matching result acceptance.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/app.rs src/effects/request.rs
git commit -m "fix: guard async read results by request context"
```

### Task 9: Minimal joined `TaskRegistry` ownership and panic reporting

**Files:**
- Create: `src/effects/task_registry.rs`
- Modify: `src/effects/mod.rs`
- Modify: `src/app.rs`
- Test: `src/effects/task_registry.rs`

- [ ] **Step 1 RED: Write the failing test**

Add `pub mod task_registry;` to `src/effects/mod.rs`:

```rust
pub mod request;
pub mod task_registry;
pub mod write_ops;
```

Create `src/effects/task_registry.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_task_is_joined_and_reported() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("read rows", async { 42usize });

        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "read rows");
        assert_eq!(report.outcome, TaskOutcome::Completed);
    }

    #[tokio::test]
    async fn panic_is_joined_and_reported_instead_of_detached() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("boom", async { panic!("task panic for test") });

        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "boom");
        assert_ne!(report.name, "unknown");
        assert!(matches!(report.outcome, TaskOutcome::Panicked { .. }));
    }

    #[tokio::test]
    async fn abort_all_drains_cancelled_tasks_with_original_identity() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("slow read", async { std::future::pending::<()>().await });

        registry.abort_all();
        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "slow read");
        assert_ne!(report.name, "unknown");
        assert_eq!(report.outcome, TaskOutcome::Cancelled);
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test effects::task_registry`

Expected: FAIL with compiler errors containing `cannot find type TaskRegistry`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this implementation above the tests in `src/effects/task_registry.rs`:

```rust
use std::{future::Future, panic::AssertUnwindSafe};
use futures_util::FutureExt;
use tokio::task::{AbortHandle, JoinSet};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    Completed,
    Cancelled,
    Panicked { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReport {
    pub id: TaskId,
    pub name: String,
    pub outcome: TaskOutcome,
}

pub struct TaskRegistry {
    next_id: u64,
    tasks: JoinSet<TaskReport>,
    abort_handles: std::collections::HashMap<TaskId, AbortHandle>,
    join_id_to_identity: std::collections::HashMap<tokio::task::Id, (TaskId, String)>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        Self { next_id: 1, tasks: JoinSet::new(), abort_handles: std::collections::HashMap::new(), join_id_to_identity: std::collections::HashMap::new() }
    }

    pub fn spawn<F, T>(&mut self, name: impl Into<String>, future: F) -> TaskId
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let id = TaskId(self.next_id);
        self.next_id += 1;
        let name = name.into();
        let identity_name = name.clone();
        let abort_handle = self.tasks.spawn(async move {
            let outcome = match AssertUnwindSafe(future).catch_unwind().await {
                Ok(_) => TaskOutcome::Completed,
                Err(payload) => TaskOutcome::Panicked { message: panic_message(payload) },
            };
            TaskReport { id, name, outcome }
        });
        self.join_id_to_identity.insert(abort_handle.id(), (id, identity_name));
        self.abort_handles.insert(id, abort_handle);
        id
    }

    pub async fn join_next(&mut self) -> Option<TaskReport> {
        let result = self.tasks.join_next_with_id().await?;
        match result {
            Ok((task_id, report)) => {
                self.abort_handles.remove(&report.id);
                self.join_id_to_identity.remove(&task_id);
                Some(report)
            }
            Err(error) if error.is_cancelled() => self.cancelled_report_for_join_id(error.id()),
            Err(error) => self.panic_report_for_join_id(error.id(), error.to_string()),
        }
    }

    fn cancelled_report_for_join_id(&mut self, task_id: tokio::task::Id) -> Option<TaskReport> {
        let (id, name) = self.identity_for_join_id(task_id)?;
        Some(TaskReport { id, name, outcome: TaskOutcome::Cancelled })
    }

    fn panic_report_for_join_id(&mut self, task_id: tokio::task::Id, message: String) -> Option<TaskReport> {
        let (id, name) = self.identity_for_join_id(task_id)?;
        Some(TaskReport { id, name, outcome: TaskOutcome::Panicked { message } })
    }

    fn identity_for_join_id(&mut self, task_id: tokio::task::Id) -> Option<(TaskId, String)> {
        let (id, name) = self.join_id_to_identity.remove(&task_id)?;
        self.abort_handles.remove(&id);
        Some((id, name))
    }

    pub fn abort_all(&mut self) {
        for handle in self.abort_handles.values() {
            handle.abort();
        }
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "task panicked with non-string payload".into()
    }
}
```

Modify `src/app.rs` so existing read, mutation, and WebSocket spawns go through `self.task_registry.spawn("descriptive name", async move { ... })`, and the event loop polls `join_next` to record `TaskOutcome::Panicked` into Activity rather than detaching. Completion, cancellation, and panic reporting all use the coded `identity_for_join_id` path above, so they never synthesize unknown identities.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test effects::task_registry`

Expected: PASS with completed tasks and panics joined into reports.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/effects/task_registry.rs src/effects/mod.rs src/app.rs
git commit -m "feat: join owned background tasks"
```

### Task 10: Immediate all-table Live disable with clear UI explanation

**Files:**
- Modify: `src/app.rs`
- Modify: `src/ui/tabs/live.rs`
- Modify: `src/ui/components/help.rs`
- Test: `src/ui/tabs/live.rs`

- [ ] **Step 1 RED: Write the failing test**

Add tests to `src/ui/tabs/live.rs`:

```rust
#[cfg(test)]
mod phase_one_live_tests {
    use super::*;

    #[test]
    fn phase_one_live_surface_explains_disabled_state() {
        let text = phase_one_live_disabled_message();

        assert!(text.contains("Live updates are temporarily unavailable"));
        assert!(text.contains("manual refresh"));
        assert!(text.contains("Bounded scoped Live will return later"));
    }

    #[test]
    fn all_table_subscribe_command_is_not_available() {
        assert_eq!(live_subscription_availability(), LiveAvailability::TemporarilyUnavailable);
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test ui::tabs::live::phase_one_live_tests`

Expected: FAIL because the current live path still attempts `ws_subscribe_all_tables` or the disabled explanation helper does not exist.

- [ ] **Step 3 GREEN: Write minimal implementation**

Modify `src/ui/tabs/live.rs`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveAvailability {
    TemporarilyUnavailable,
}

pub fn live_subscription_availability() -> LiveAvailability {
    LiveAvailability::TemporarilyUnavailable
}

pub fn phase_one_live_disabled_message() -> &'static str {
    "Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable."
}
```

In the Live tab render function, draw `phase_one_live_disabled_message()` instead of a spinner or waiting state. Modify `src/app.rs` by removing the `handle_ws_event(WsEvent::Connected) -> ws_subscribe_all_tables().await` path and deleting or gating `ws_subscribe_all_tables`. Keep bounded `st_client` polling only if the UI labels it as metadata polling, not row Live.

Modify `src/ui/components/help.rs` to include the same wording.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test ui::tabs::live::phase_one_live_tests`

Expected: PASS with no automatic all-table subscription available in Phase 1.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/app.rs src/ui/tabs/live.rs src/ui/components/help.rs
git commit -m "fix: disable unbounded all-table live subscriptions"
```

### Task 11: `TerminalGuard` and `TerminalOps` partial/idempotent restoration

**Files:**
- Create: `src/terminal.rs`
- Modify: `src/main.rs`
- Test: `src/terminal.rs`

- [ ] **Step 1 RED: Write the failing test**

Add `mod terminal;` beside the other module declarations in `src/main.rs`:

```rust
mod terminal;
```

Create `src/terminal.rs` with tests first:

```rust
#[cfg(test)]
mod terminal_guard_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default, Clone)]
    struct FakeOps { calls: Rc<RefCell<Vec<&'static str>>>, fail_after_raw: bool }

    impl TerminalOps for FakeOps {
        type Error = &'static str;
        fn enable_raw_mode(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("enable_raw"); Ok(()) }
        fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("enter_alt"); if self.fail_after_raw { Err("alt failed") } else { Ok(()) } }
        fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("enable_mouse"); Ok(()) }
        fn hide_cursor(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("hide_cursor"); Ok(()) }
        fn show_cursor(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("show_cursor"); Ok(()) }
        fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("disable_mouse"); Ok(()) }
        fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("leave_alt"); Ok(()) }
        fn disable_raw_mode(&mut self) -> Result<(), Self::Error> { self.calls.borrow_mut().push("disable_raw"); Ok(()) }
    }

    #[test]
    fn partial_setup_restores_only_completed_steps() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let ops = FakeOps { calls: calls.clone(), fail_after_raw: true };

        let result = TerminalGuard::enter(ops);

        assert!(result.is_err());
        assert_eq!(&*calls.borrow(), &["enable_raw", "enter_alt", "disable_raw"]);
    }

    #[test]
    fn restore_is_idempotent() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let ops = FakeOps { calls: calls.clone(), fail_after_raw: false };
        let mut guard = TerminalGuard::enter(ops).unwrap();

        guard.restore();
        guard.restore();
        drop(guard);

        assert_eq!(calls.borrow().iter().filter(|call| **call == "disable_raw").count(), 1);
        assert_eq!(calls.borrow().iter().filter(|call| **call == "leave_alt").count(), 1);
        assert_eq!(calls.borrow().iter().filter(|call| **call == "disable_mouse").count(), 1);
        assert_eq!(calls.borrow().iter().filter(|call| **call == "show_cursor").count(), 1);
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test terminal::terminal_guard_tests`

Expected: FAIL with compiler errors containing `cannot find trait TerminalOps` or `cannot find type TerminalGuard`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this implementation above the tests in `src/terminal.rs`:

```rust
pub trait TerminalOps {
    type Error;
    fn enable_raw_mode(&mut self) -> Result<(), Self::Error>;
    fn enter_alternate_screen(&mut self) -> Result<(), Self::Error>;
    fn enable_mouse_capture(&mut self) -> Result<(), Self::Error>;
    fn hide_cursor(&mut self) -> Result<(), Self::Error>;
    fn show_cursor(&mut self) -> Result<(), Self::Error>;
    fn disable_mouse_capture(&mut self) -> Result<(), Self::Error>;
    fn leave_alternate_screen(&mut self) -> Result<(), Self::Error>;
    fn disable_raw_mode(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CrosstermTerminalOps;

impl CrosstermTerminalOps {
    pub fn new() -> Self {
        Self
    }
}

impl TerminalOps for CrosstermTerminalOps {
    type Error = std::io::Error;

    fn enable_raw_mode(&mut self) -> Result<(), Self::Error> {
        crossterm::terminal::enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
    }

    fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)
    }

    fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture)
    }

    fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> {
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)
    }

    fn disable_raw_mode(&mut self) -> Result<(), Self::Error> {
        crossterm::terminal::disable_raw_mode()
    }
}

pub struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw: bool,
    alternate: bool,
    mouse: bool,
    cursor_hidden: bool,
    restored: bool,
}

impl<O: TerminalOps> TerminalGuard<O> {
    pub fn enter(ops: O) -> Result<Self, O::Error> {
        let mut guard = Self { ops, raw: false, alternate: false, mouse: false, cursor_hidden: false, restored: false };
        guard.ops.enable_raw_mode()?;
        guard.raw = true;
        if let Err(error) = guard.ops.enter_alternate_screen() {
            guard.restore();
            return Err(error);
        }
        guard.alternate = true;
        if let Err(error) = guard.ops.enable_mouse_capture() {
            guard.restore();
            return Err(error);
        }
        guard.mouse = true;
        if let Err(error) = guard.ops.hide_cursor() {
            guard.restore();
            return Err(error);
        }
        guard.cursor_hidden = true;
        Ok(guard)
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        if self.cursor_hidden {
            let _ = self.ops.show_cursor();
            self.cursor_hidden = false;
        }
        if self.mouse {
            let _ = self.ops.disable_mouse_capture();
            self.mouse = false;
        }
        if self.alternate {
            let _ = self.ops.leave_alternate_screen();
            self.alternate = false;
        }
        if self.raw {
            let _ = self.ops.disable_raw_mode();
            self.raw = false;
        }
        self.restored = true;
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        self.restore();
    }
}
```

Modify `src/main.rs` to create `TerminalGuard::enter(CrosstermTerminalOps::new())?` before constructing the existing `CrosstermBackend<std::io::Stdout>`, pass that backend into the app as it does today, and rely on `Drop` for normal return, error return, and unwind through the guarded owner. Refactor the current `setup_terminal` helper so it only creates/clears the Ratatui terminal; remove its direct raw-mode, alternate-screen, mouse-capture, and cursor setup because the guard now owns those transitions. Remove the old manual `restore_terminal` call/path rather than restoring twice. Keep the guard binding alive until after `App::run` returns. `CrosstermTerminalOps` is stateless and reacquires `std::io::stdout()` for each terminal command, so the guard does not consume the stdout handle later owned by the Ratatui backend. The panic hook remains logging-only and does not manipulate the terminal from arbitrary background tasks.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test terminal::terminal_guard_tests`

Expected: PASS with partial setup and idempotent restoration covered.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/terminal.rs src/main.rs
git commit -m "feat: guard terminal restoration"
```

### Task 12: Unicode display-width truncation and saturating rectangles

**Files:**
- Create: `src/ui/text.rs`
- Modify: `src/ui/mod.rs`
- Modify: `src/ui/tabs/sql.rs`
- Modify: `src/ui/tabs/live.rs`
- Modify: `src/ui/layout.rs`
- Test: `src/ui/text.rs`

- [ ] **Step 1 RED: Write the failing test**

Create `src/ui/text.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn truncation_respects_unicode_display_width() {
        assert_eq!(truncate_display_width("abcdef", 4), "abc…");
        assert_eq!(truncate_display_width("数据表", 4), "数…");
        assert_eq!(truncate_display_width("a🦀b", 4), "a🦀…");
        assert!(truncate_display_width("e\u{301}clair", 3).ends_with('…'));
    }

    #[test]
    fn truncation_never_slices_inside_utf8() {
        for width in 0..8 {
            let truncated = truncate_display_width("é🦀数据", width);
            assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
        }
    }

    #[test]
    fn saturating_inner_rect_handles_tiny_areas() {
        assert_eq!(saturating_inner(Rect::new(0, 0, 1, 1), 2), Rect::new(2, 2, 0, 0));
        assert_eq!(saturating_inner(Rect::new(2, 3, 10, 5), 1), Rect::new(3, 4, 8, 3));
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test ui::text`

Expected: FAIL with `file not found for module text` or missing `truncate_display_width` and `saturating_inner`.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add `pub mod text;` to `src/ui/mod.rs`:

```rust
pub mod text;
```

Add this implementation above the tests in `src/ui/text.rs`:

```rust
use ratatui::layout::Rect;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn truncate_display_width(input: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(input) <= max_width {
        return input.to_string();
    }
    if max_width == 1 {
        return "…".into();
    }

    let mut output = String::new();
    let mut used = 0usize;
    let budget = max_width.saturating_sub(1);
    for ch in input.chars() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used.saturating_add(width) > budget {
            break;
        }
        output.push(ch);
        used += width;
    }
    output.push('…');
    output
}

pub fn saturating_inner(area: Rect, margin: u16) -> Rect {
    let double = margin.saturating_mul(2);
    Rect {
        x: area.x.saturating_add(margin),
        y: area.y.saturating_add(margin),
        width: area.width.saturating_sub(double),
        height: area.height.saturating_sub(double),
    }
}
```

Modify `src/ui/tabs/live.rs` to replace `inner.width as usize - 40` with `inner.width.saturating_sub(40) as usize` and then call `truncate_display_width`. Modify `src/ui/tabs/sql.rs` so SQL history and output labels call `truncate_display_width(label, available_width as usize)` instead of byte slicing. Modify `src/ui/layout.rs` so tiny rectangles render an empty or minimal safe view and no unchecked rectangle subtraction remains.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test ui::text`

Expected: PASS with Unicode-safe truncation and saturating rectangle behavior.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/ui/text.rs src/ui/mod.rs src/ui/tabs/sql.rs src/ui/tabs/live.rs src/ui/layout.rs
git commit -m "fix: make text and layout math panic safe"
```

### Task 13: Nav-up reload symmetry

**Files:**
- Modify: `src/app.rs`
- Test: `src/app.rs`

- [ ] **Step 1 RED: Write the failing test**

Add this pure behavior test to `src/app.rs`:

```rust
#[test]
fn database_nav_up_emits_schema_reload_when_selection_changes() {
    let old_index = 1usize;
    let new_index = 0usize;
    let databases = vec!["first".to_string(), "second".to_string()];

    let effect = schema_reload_effect_for_database_index_change(old_index, new_index, &databases);

    assert_eq!(effect, Some("first".to_string()));
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test app::database_nav_up_emits_schema_reload_when_selection_changes`

Expected: FAIL because upward database navigation currently mutates selection without using the shared reload effect function.

- [ ] **Step 3 GREEN: Write minimal implementation**

Add this real pure function and call it from `nav_up` after `database_prev()`:

```rust
fn schema_reload_effect_for_database_index_change(old_index: usize, new_index: usize, databases: &[String]) -> Option<String> {
    if old_index != new_index {
        databases.get(new_index).cloned()
    } else {
        None
    }
}
```

Change `nav_up` to async and mirror `nav_down`:

```rust
async fn nav_up(&mut self) {
    match self.state.focus {
        FocusPanel::Sidebar => match self.state.sidebar_focus {
            SidebarFocus::Databases => {
                let old = self.state.selected_database_idx;
                self.state.database_prev();
                if self.state.selected_database_idx != old {
                    self.load_schema().await;
                }
            }
            SidebarFocus::Tables => self.state.table_prev(),
        },
        _ => self.nav_up_inside_current_panel(),
    }
}
```

Update the key handler from `self.nav_up();` to `self.nav_up().await;`.

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test app::database_nav_up_emits_schema_reload_when_selection_changes`

Expected: PASS with nav-up and nav-down database selection both triggering schema reload on selection change.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add src/app.rs
git commit -m "fix: make navigate up reload selected resources"
```

### Task 14: README, help, and CHANGELOG truth gate

**Files:**
- Modify: `README.md`
- Modify: `src/ui/components/help.rs`
- Create: `CHANGELOG.md`
- Test: `src/ui/components/help.rs`

- [ ] **Step 1 RED: Write the failing test**

Add this test to `src/ui/components/help.rs`:

```rust
#[cfg(test)]
mod phase_one_truth_gate_tests {
    use super::*;
    use std::fs;

    #[test]
    fn phase_one_behavior_claims_are_truthful_in_docs_and_help() {
        let readme = fs::read_to_string("README.md").unwrap();
        let changelog = fs::read_to_string("CHANGELOG.md").unwrap();
        let help = phase_one_safety_help_text();

        for text in [&readme, &changelog, help] {
            assert!(text.contains("Live updates are temporarily unavailable"));
            assert!(text.contains("one-row spreadsheet Save"));
            assert!(text.contains("Unknown mutation outcomes are not automatically retried"));
            assert!(text.contains("Guided update/delete require declared primary keys and matching generations, with no unsafe override"));
            assert!(text.contains("Raw SQL is a separately labeled expert path"));
            assert!(text.contains("Raw SQL does not receive the guided CRUD guarantee and is never automatically retried"));
        }
    }
}
```

- [ ] **Step 2 RED: Run test to verify it fails**

Run: `cargo test ui::components::help::phase_one_truth_gate_tests`

Expected: FAIL because baseline has no `CHANGELOG.md`, or README, in-app help, or CHANGELOG still claim unbounded Live or do not describe the one-row save and mutation uncertainty behavior.

- [ ] **Step 3 GREEN: Write minimal implementation**

Modify `src/ui/components/help.rs`:

```rust
pub fn phase_one_safety_help_text() -> &'static str {
    "Phase 1 safety: Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable. Spreadsheet editing supports one-row spreadsheet Save only: changed cells in one row are saved as one guided WritePlan, and changing rows prompts Save, Discard, or Stay. Guided update/delete require declared primary keys and matching generations, with no unsafe override. Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried. Unknown mutation outcomes are not automatically retried; refresh the affected scope before another guided attempt."
}
```

Modify `README.md` feature text to include this exact paragraph:

```markdown
### Phase 1 safety behavior

Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable. Spreadsheet editing supports one-row spreadsheet Save: multiple changed cells in one row are saved as one guided write, while moving to another row prompts Save, Discard, or Stay. Guided update/delete require declared primary keys and matching generations, with no unsafe override. Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried. Unknown mutation outcomes are not automatically retried; refresh the affected scope before another guided attempt.
```

Create `CHANGELOG.md` with minimal initial content and the Phase 1 safety entry:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

## Unreleased

### Changed

- Live updates are temporarily unavailable while safety controls are tightened. Use manual refresh for table data. Bounded scoped Live will return later; broad automatic subscriptions are currently unavailable.
- Spreadsheet editing now uses one-row spreadsheet Save: multiple changed cells in one row form one guided `WritePlan`; moving to another row prompts Save, Discard, or Stay.
- Guided update/delete require declared primary keys and matching generations, with no unsafe override.
- Raw SQL is a separately labeled expert path; Raw SQL does not receive the guided CRUD guarantee and is never automatically retried.
- Unknown mutation outcomes are not automatically retried after transport ownership begins. Refresh the affected scope before another guided attempt.
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run: `cargo test ui::components::help::phase_one_truth_gate_tests`

Expected: PASS with README, in-app help, and the newly created CHANGELOG all matching Phase 1 behavior.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add README.md src/ui/components/help.rs CHANGELOG.md
git commit -m "docs: document phase one safety behavior"
```

### Task 15: Final fmt, clippy, full test, release build, and release gate

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Test: whole repository

- [ ] **Step 1 RED: Write the failing gate checklist**

Add this release-gate checklist section to `CHANGELOG.md` under the Phase 1 entry:

```markdown
### Phase 1 validation gate

- `cargo fmt --all -- --check` passes.
- `cargo clippy --all-targets --all-features --locked -- -D warnings` passes.
- `cargo test --all-features --locked` passes and includes the original 127-test baseline plus Phase 1 safety tests.
- `cargo build --release --locked` passes.
- README, in-app help, and CHANGELOG all state that Live updates are temporarily unavailable, one-row spreadsheet Save is the supported spreadsheet contract, guided update/delete require declared primary keys and matching generations with no unsafe override, Raw SQL is a separately labeled expert path without the guided CRUD guarantee, and Unknown mutation outcomes are not automatically retried.
```

- [ ] **Step 2 RED: Run gate commands to expose remaining failures**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo build --release --locked
```

Expected: Any remaining formatting, lint, test-count, or release-build failure is visible. If all pass immediately, record that the failing phase was already satisfied by earlier tasks and proceed to Step 3 without changing behavior.

- [ ] **Step 3 GREEN: Write minimal implementation for gate failures**

Apply only mechanical fixes needed by the gate outputs:

```rust
// Formatting: run cargo fmt and keep rustfmt-only changes.
// Clippy: replace needless clones with borrows, remove unused imports, and prefer is_some_and where suggested.
// Tests: adjust helper visibility under #[cfg(test)] only and keep production APIs stable.
// Release: ensure newly added modules are exported exactly once from mod.rs files.
```

Update `README.md` with the validation statement:

```markdown
Phase 1 release validation requires formatting, warning-denied Clippy, all tests with the 127-test baseline plus Phase 1 additions, and a release build before the phase is called complete.
```

- [ ] **Step 4 GREEN: Run test to verify it passes**

Run:

```bash
cargo fmt --all -- --check && \
cargo clippy --all-targets --all-features --locked -- -D warnings && \
cargo test --all-features --locked && \
cargo build --release --locked
```

Expected: PASS for formatting, warning-denied Clippy, all tests, and release build. Test output must show at least the original 127-test baseline plus the new Phase 1 tests.

- [ ] **Step 5 COMMIT: Commit**

```bash
git add README.md CHANGELOG.md src/effects/mod.rs src/effects/request.rs src/effects/write_ops.rs src/effects/task_registry.rs src/state/safety.rs src/state/mod.rs src/state/edit_mode.rs src/state/modal.rs src/app.rs src/main.rs src/terminal.rs src/ui/text.rs src/ui/mod.rs src/ui/tabs/sql.rs src/ui/tabs/live.rs src/ui/layout.rs src/ui/components/help.rs
git commit -m "chore: pass phase one safety release gate"
```

## Logical commit list

1. `feat: add scoped request context`
2. `feat: add write safety domain types`
3. `feat: validate table primary keys from raw rows`
4. `feat: build and revalidate guided write plans`
5. `fix: integrate pure guided write plan builder`
6. `feat: constrain spreadsheet saves to one row`
7. `feat: verify authoritative mutation outcomes`
8. `fix: guard async read results by request context`
9. `feat: join owned background tasks`
10. `fix: disable unbounded all-table live subscriptions`
11. `feat: guard terminal restoration`
12. `fix: make text and layout math panic safe`
13. `fix: make navigate up reload selected resources`
14. `docs: document phase one safety behavior`
15. `chore: pass phase one safety release gate`

## Self-review

- Spec coverage: Phase 1 declared-primary-key validation, malformed metadata, composite keys, SQL encoding, confirmation revalidation, one-row spreadsheet Save/Discard/Stay, mutation `DefinitelyNotSent`, `Conflict`, `Unknown`, and `CriticalSafetyError`, request stale guards, joined task panic reporting, immediate all-table Live disable, terminal restoration, Unicode truncation, saturating layout, nav-up reload symmetry, documentation truth, and final release gates are each mapped to explicit tasks above.
- Concrete-content scan: The plan contains concrete file paths, tests, commands, expected failures, implementation code or exact logic, passing commands, and commit commands for every task.
- Type consistency: Stable names are preserved exactly: `RequestId`, `RequestScope`, `RequestContext`, `WritePlanId`, `QualifiedTable`, `PrimaryKeyPart`, `CompletePrimaryKey`, `ColumnChange`, `GuidedMutation`, `WritePlan`, `MutationOutcome`, `TaskRegistry`, `TerminalGuard`, and `TerminalOps`. `RequestScope` variants are `DatabaseCatalog`, `Schema { database }`, `TableRows { database, table, view }`, `SqlWorkspace { database, workspace }`, `Logs { database }`, `Metrics { database }`, and `LiveClients { database }`. `GuidedMutation` contains only `Update` and `Delete`.

Plan complete and saved to `docs/superpowers/plans/2026-07-26-contextual-workbench-phase-1-safety.md`. Two execution options:

1. Subagent-Driven (recommended) - dispatch a fresh subagent per task, review between tasks, fast iteration.
2. Inline Execution - execute tasks in this session using executing-plans, batch execution with checkpoints.
