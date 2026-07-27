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
    /// Null is rejected as a primary key value; kept for exhaustive
    /// matching in validation code.
    #[allow(dead_code)]
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
                SqlValue::Null => {
                    return Err(PrimaryKeyError::NullValue {
                        column_id: part.column_id,
                    });
                }
                SqlValue::Unrepresentable(_) => {
                    return Err(PrimaryKeyError::UnrepresentableValue {
                        column_id: part.column_id,
                    });
                }
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
    CriticalSafetyError { reason: String },
    DefinitelyNotSent { reason: String },
    SentAndConfirmed { affected_rows: Option<u64> },
    Conflict { reason: String },
    Unknown { reason: String },
}

// ---------------------------------------------------------------------------
// Phase 2: SafetyState aggregate
// ---------------------------------------------------------------------------

/// An unresolved mutation whose outcome is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationUncertainty {
    pub plan_id: WritePlanId,
    pub table: QualifiedTable,
    pub reason: String,
}

/// Aggregate safety state: pending write plans and unresolved mutations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SafetyState {
    pub pending_write_plans: Vec<WritePlan>,
    pub unresolved_mutations: Vec<MutationUncertainty>,
}

#[allow(dead_code)]
impl SafetyState {
    /// Record a mutation outcome. Only `Unknown` outcomes are tracked as
    /// unresolved uncertainty; all other outcomes are terminal.
    pub fn record_mutation_outcome(
        &mut self,
        plan_id: WritePlanId,
        table: QualifiedTable,
        outcome: MutationOutcome,
    ) {
        if let MutationOutcome::Unknown { reason } = outcome {
            self.unresolved_mutations.push(MutationUncertainty {
                plan_id,
                table,
                reason,
            });
        }
    }

    pub fn unresolved_mutation_count(&self) -> usize {
        self.unresolved_mutations.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_primary_key_parts_are_schema_ordered_not_declared_ordered() {
        let key = CompletePrimaryKey::from_schema_ordered_parts(vec![
            PrimaryKeyPart {
                column_id: 0u16,
                value: SqlValue::I64(10),
            },
            PrimaryKeyPart {
                column_id: 1u16,
                value: SqlValue::Text("blue".into()),
            },
        ])
        .unwrap();

        assert_eq!(key.parts()[0].column_id, 0);
        assert_eq!(key.parts()[1].column_id, 1);
    }

    #[test]
    fn complete_primary_key_rejects_empty_duplicate_null_and_unrepresentable_parts() {
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![]),
            Err(PrimaryKeyError::NoDeclaredPrimaryKey)
        );
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![
                PrimaryKeyPart {
                    column_id: 2u16,
                    value: SqlValue::I64(1),
                },
                PrimaryKeyPart {
                    column_id: 2u16,
                    value: SqlValue::I64(2),
                },
            ]),
            Err(PrimaryKeyError::DuplicateColumnId(2))
        );
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart {
                column_id: 0u16,
                value: SqlValue::Null,
            }]),
            Err(PrimaryKeyError::NullValue { column_id: 0 })
        );
        assert_eq!(
            CompletePrimaryKey::from_schema_ordered_parts(vec![PrimaryKeyPart {
                column_id: 0u16,
                value: SqlValue::Unrepresentable("array".into()),
            }]),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 0 })
        );
    }

    #[test]
    fn mutation_outcome_variants_are_constructible_for_transport_classification() {
        assert_eq!(
            MutationOutcome::DefinitelyNotSent {
                reason: "local stale state".into(),
            },
            MutationOutcome::DefinitelyNotSent {
                reason: "local stale state".into(),
            }
        );
        assert_eq!(
            MutationOutcome::SentAndConfirmed {
                affected_rows: Some(1),
            },
            MutationOutcome::SentAndConfirmed {
                affected_rows: Some(1),
            }
        );
        assert_eq!(
            MutationOutcome::Conflict {
                reason: "old key still present".into(),
            },
            MutationOutcome::Conflict {
                reason: "old key still present".into(),
            }
        );
        assert_eq!(
            MutationOutcome::Unknown {
                reason: "verification unavailable".into(),
            },
            MutationOutcome::Unknown {
                reason: "verification unavailable".into(),
            }
        );
        assert_eq!(
            MutationOutcome::CriticalSafetyError {
                reason: "invariant violated".into(),
            },
            MutationOutcome::CriticalSafetyError {
                reason: "invariant violated".into(),
            }
        );
    }

    #[test]
    fn records_only_unknown_mutation_outcomes_as_uncertainty() {
        let mut state = SafetyState::default();
        let plan_id = WritePlanId(7);
        let table = QualifiedTable {
            database: "inventory".to_string(),
            schema: None,
            table: "items".to_string(),
        };

        state.record_mutation_outcome(
            plan_id,
            table.clone(),
            MutationOutcome::DefinitelyNotSent {
                reason: "client rejected before send".to_string(),
            },
        );
        state.record_mutation_outcome(
            plan_id,
            table.clone(),
            MutationOutcome::SentAndConfirmed {
                affected_rows: Some(1),
            },
        );
        state.record_mutation_outcome(
            plan_id,
            table.clone(),
            MutationOutcome::Unknown {
                reason: "connection dropped before acknowledgement".to_string(),
            },
        );

        assert_eq!(state.unresolved_mutation_count(), 1);
        assert_eq!(state.unresolved_mutations[0].plan_id, plan_id);
        assert_eq!(state.unresolved_mutations[0].table, table);
    }
}
