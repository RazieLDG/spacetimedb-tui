use std::collections::HashSet;

use crate::api::types::{ColumnInfo, TableInfo};
use crate::state::safety::{CompletePrimaryKey, PrimaryKeyError, PrimaryKeyPart, SqlValue};

pub fn typed_sql_value_from_json(
    column: &ColumnInfo,
    value: &serde_json::Value,
) -> Result<SqlValue, PrimaryKeyError> {
    let column_id = column_id_u16(column)?;
    if value.is_null() {
        return Err(PrimaryKeyError::NullValue { column_id });
    }

    let declared = declared_type_name(&column.col_type)
        .ok_or(PrimaryKeyError::UnrepresentableValue { column_id })?;

    sql_value_from_json_for_declared_type(value, declared).map_err(|error| match error {
        DeclaredValueError::TypeMismatch => PrimaryKeyError::TypeMismatch {
            column_id,
            expected: declared.to_owned(),
        },
        DeclaredValueError::Unrepresentable => PrimaryKeyError::UnrepresentableValue { column_id },
    })
}

pub fn complete_primary_key_from_table_row(
    table: &TableInfo,
    row: &[serde_json::Value],
) -> Result<CompletePrimaryKey, PrimaryKeyError> {
    if table.primary_key_cols.is_empty() {
        return Err(PrimaryKeyError::NoDeclaredPrimaryKey);
    }

    let mut declared = HashSet::with_capacity(table.primary_key_cols.len());
    for &column_id in &table.primary_key_cols {
        if !declared.insert(column_id) {
            return Err(PrimaryKeyError::DuplicateColumnId(column_id));
        }
    }

    let mut parts = Vec::with_capacity(table.primary_key_cols.len());
    let mut found = HashSet::with_capacity(table.primary_key_cols.len());
    for (column_index, column) in table.columns.iter().enumerate() {
        let column_id = column_id_u16(column)?;
        if declared.contains(&column_id) {
            found.insert(column_id);
            let raw_value = row
                .get(column_index)
                .ok_or(PrimaryKeyError::MissingValue { column_id })?;
            let value = typed_sql_value_from_json(column, raw_value)?;
            parts.push(PrimaryKeyPart { column_id, value });
        }
    }

    for &column_id in &table.primary_key_cols {
        if !found.contains(&column_id) {
            return Err(PrimaryKeyError::UnknownColumnId(column_id));
        }
    }

    CompletePrimaryKey::from_schema_ordered_parts(parts)
}

fn column_id_u16(column: &ColumnInfo) -> Result<u16, PrimaryKeyError> {
    u16::try_from(column.col_id).map_err(|_| PrimaryKeyError::ColumnIdOutOfRange {
        column_id: column.col_id,
    })
}

fn declared_type_name(type_value: &serde_json::Value) -> Option<&str> {
    match type_value {
        serde_json::Value::String(name) => Some(name.as_str()),
        serde_json::Value::Object(fields) if fields.len() == 1 => {
            let (name, value) = fields.iter().next()?;
            if name == "AlgebraicType" {
                declared_type_name(value)
            } else {
                Some(name.as_str())
            }
        }
        _ => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
enum DeclaredValueError {
    TypeMismatch,
    Unrepresentable,
}

fn sql_value_from_json_for_declared_type(
    value: &serde_json::Value,
    declared: &str,
) -> Result<SqlValue, DeclaredValueError> {
    if let Some(tagged_value) = tagged_value_for_declared_type(value, declared) {
        return sql_value_from_untagged_json_for_declared_type(
            tagged_value,
            declared,
            TaggedValueMode::Tagged,
        );
    }

    if has_recognized_mismatched_scalar_tag(value, declared) {
        return Err(DeclaredValueError::TypeMismatch);
    }

    if value.is_object() || value.is_array() {
        return Err(DeclaredValueError::Unrepresentable);
    }

    sql_value_from_untagged_json_for_declared_type(value, declared, TaggedValueMode::Untagged)
}

fn tagged_value_for_declared_type<'a>(
    value: &'a serde_json::Value,
    declared: &str,
) -> Option<&'a serde_json::Value> {
    let fields = value.as_object()?;
    if fields.len() != 1 {
        return None;
    }

    fields.get(declared)
}

fn has_recognized_mismatched_scalar_tag(value: &serde_json::Value, declared: &str) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    if fields.len() != 1 {
        return false;
    }

    let Some(tag) = fields.keys().next() else {
        return false;
    };

    tag != declared && is_supported_scalar_type(tag)
}

fn is_supported_scalar_type(type_name: &str) -> bool {
    matches!(type_name, "Bool" | "String" | "U64")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TaggedValueMode {
    Tagged,
    Untagged,
}

fn sql_value_from_untagged_json_for_declared_type(
    value: &serde_json::Value,
    declared: &str,
    mode: TaggedValueMode,
) -> Result<SqlValue, DeclaredValueError> {
    match declared {
        "Bool" => value
            .as_bool()
            .map(SqlValue::Bool)
            .ok_or(error_for_mode(mode)),
        "String" => value
            .as_str()
            .map(|text| SqlValue::Text(text.to_owned()))
            .ok_or(error_for_mode(mode)),
        "U64" => value
            .as_u64()
            .map(SqlValue::U64)
            .ok_or(error_for_mode(mode)),
        _ => Err(DeclaredValueError::Unrepresentable),
    }
}

fn error_for_mode(mode: TaggedValueMode) -> DeclaredValueError {
    match mode {
        TaggedValueMode::Tagged => DeclaredValueError::Unrepresentable,
        TaggedValueMode::Untagged => DeclaredValueError::TypeMismatch,
    }
}

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
        ColumnInfo {
            col_id: id,
            col_name: name.into(),
            col_type,
            is_autoinc: false,
        }
    }

    fn table(primary_key_cols: Vec<u16>) -> TableInfo {
        TableInfo {
            table_name: "inventory".into(),
            product_type_ref: 0,
            table_type: "user".into(),
            table_access: "public".into(),
            columns: vec![
                col(7, "account", "U64"),
                col(2, "region", "String"),
                col(9, "name", "String"),
            ],
            primary_key_cols,
            indexes: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn declared_primary_key_resolves_parts_in_schema_order() {
        let key = complete_primary_key_from_table_row(
            &table(vec![2, 7]),
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();

        assert_eq!(key.parts()[0].column_id, 7);
        assert_eq!(key.parts()[0].value, SqlValue::U64(42));
        assert_eq!(key.parts()[1].column_id, 2);
        assert_eq!(key.parts()[1].value, SqlValue::Text("eu".into()));
    }

    #[test]
    fn declared_primary_key_rejects_no_duplicate_unknown_missing_null_and_unrepresentable_values() {
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![]), &[]),
            Err(PrimaryKeyError::NoDeclaredPrimaryKey)
        );
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![7, 7]), &[]),
            Err(PrimaryKeyError::DuplicateColumnId(7))
        );
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![99]), &[]),
            Err(PrimaryKeyError::UnknownColumnId(99))
        );
        let mut out_of_range = table(vec![7]);
        out_of_range.columns[0].col_id = u32::from(u16::MAX) + 1;
        assert_eq!(
            complete_primary_key_from_table_row(&out_of_range, &[]),
            Err(PrimaryKeyError::ColumnIdOutOfRange {
                column_id: u32::from(u16::MAX) + 1,
            })
        );
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![9]), &[serde_json::json!(1)]),
            Err(PrimaryKeyError::MissingValue { column_id: 9 })
        );
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![7]), &[serde_json::Value::Null]),
            Err(PrimaryKeyError::NullValue { column_id: 7 })
        );
        assert_eq!(
            complete_primary_key_from_table_row(&table(vec![7]), &[serde_json::json!([1, 2])]),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 })
        );
    }

    #[test]
    fn declared_column_types_reject_mismatched_json_before_write_plan() {
        let mut typed = table(vec![7]);
        typed.columns[0].col_type = serde_json::json!({ "AlgebraicType": { "U64": {} } });
        typed.columns[1].col_type = serde_json::json!({ "AlgebraicType": { "Bool": {} } });
        typed.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "String": {} } });

        assert_eq!(
            typed_sql_value_from_json(&typed.columns[0], &serde_json::json!("not a number")),
            Err(PrimaryKeyError::TypeMismatch {
                column_id: 7,
                expected: "U64".into(),
            })
        );
        assert_eq!(
            typed_sql_value_from_json(&typed.columns[1], &serde_json::json!(1)),
            Err(PrimaryKeyError::TypeMismatch {
                column_id: 2,
                expected: "Bool".into(),
            })
        );
        assert_eq!(
            typed_sql_value_from_json(&typed.columns[2], &serde_json::json!(false)),
            Err(PrimaryKeyError::TypeMismatch {
                column_id: 9,
                expected: "String".into(),
            })
        );
        assert_eq!(
            typed_sql_value_from_json(
                &typed.columns[0],
                &serde_json::json!({ "U64": "not numeric" })
            ),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 })
        );
    }

    #[test]
    fn declared_type_shapes_support_strings_single_key_objects_and_algebraic_type_wrappers() {
        let mut typed = table(vec![7]);
        typed.columns[0].col_type = serde_json::json!("U64");
        typed.columns[1].col_type = serde_json::json!({ "String": {} });
        typed.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "String": {} } });

        assert_eq!(
            typed_sql_value_from_json(&typed.columns[0], &serde_json::json!(42)).unwrap(),
            SqlValue::U64(42)
        );
        assert_eq!(
            typed_sql_value_from_json(&typed.columns[1], &serde_json::json!("eu")).unwrap(),
            SqlValue::Text("eu".into())
        );
        assert_eq!(
            typed_sql_value_from_json(&typed.columns[2], &serde_json::json!({ "String": "Ada" }))
                .unwrap(),
            SqlValue::Text("Ada".into())
        );
    }

    #[test]
    fn malformed_tagged_scalar_returns_unrepresentable() {
        let typed = table(vec![7]);

        assert_eq!(
            typed_sql_value_from_json(
                &typed.columns[0],
                &serde_json::json!({ "U64": "not numeric" })
            ),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 })
        );
    }

    #[test]
    fn recognized_mismatched_tagged_scalar_returns_type_mismatch() {
        let typed = table(vec![7]);

        assert_eq!(
            typed_sql_value_from_json(&typed.columns[0], &serde_json::json!({ "String": "42" })),
            Err(PrimaryKeyError::TypeMismatch {
                column_id: 7,
                expected: "U64".into(),
            })
        );
    }

    #[test]
    fn untagged_u64_max_converts_to_sql_u64() {
        let typed = table(vec![7]);

        assert_eq!(
            typed_sql_value_from_json(&typed.columns[0], &serde_json::json!(u64::MAX)).unwrap(),
            SqlValue::U64(u64::MAX)
        );
    }

    #[test]
    fn unknown_and_complex_object_shapes_remain_unrepresentable() {
        let typed = table(vec![7]);

        assert_eq!(
            typed_sql_value_from_json(&typed.columns[0], &serde_json::json!({ "Unknown": 42 })),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 })
        );
        assert_eq!(
            typed_sql_value_from_json(
                &typed.columns[0],
                &serde_json::json!({ "String": "42", "U64": 42 })
            ),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 7 })
        );
    }
}
