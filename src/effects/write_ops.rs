use std::collections::HashSet;

use crate::api::types::{ColumnInfo, TableInfo};
use crate::state::safety::{
    ColumnChange, CompletePrimaryKey, GuidedMutation, MutationOutcome, PrimaryKeyError,
    PrimaryKeyPart, QualifiedTable, SqlValue, WritePlan, WritePlanId,
};

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeTag(String);

impl TypeTag {
    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Generations {
    pub schema: u64,
    pub row: u64,
    pub server_context: u64,
    pub database: u64,
}

#[derive(Clone, Debug)]
pub struct ConfirmationSnapshot<'a> {
    pub generations: Generations,
    pub table: &'a TableInfo,
    pub current_raw_row: &'a [serde_json::Value],
    pub row_locked: bool,
}

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

pub fn type_tag(declared_type: &serde_json::Value) -> Result<TypeTag, SqlEncodingError> {
    let tag = declared_type_name(declared_type).ok_or_else(|| SqlEncodingError::TypeMismatch {
        column_id: 0,
        expected: "supported scalar type".into(),
    })?;
    if is_supported_scalar_type(tag) {
        Ok(TypeTag(tag.to_owned()))
    } else {
        Err(SqlEncodingError::TypeMismatch {
            column_id: 0,
            expected: "supported scalar type".into(),
        })
    }
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

pub fn json_to_sql_literal(
    raw_value: &serde_json::Value,
    declared_type: &TypeTag,
) -> Result<String, SqlEncodingError> {
    let tag = declared_type.as_str();
    let (value, tagged) = value_for_tag(raw_value, tag)?;
    match tag {
        "Bool" => value
            .as_bool()
            .map(|value| if value { "TRUE" } else { "FALSE" }.to_owned())
            .ok_or_else(|| type_mismatch(tag)),
        "String" => value
            .as_str()
            .map(quote_string_literal)
            .ok_or_else(|| type_mismatch(tag)),
        "I8" | "I16" | "I32" | "I64" => encode_signed_64_literal(value, tag),
        "U8" | "U16" | "U32" | "U64" => encode_unsigned_64_literal(value, tag),
        "F32" | "F64" => encode_float_literal(value, tag, tagged),
        "I128" | "I256" | "U128" | "U256" => encode_large_integer_literal(value, tag),
        "Identity" | "ConnectionId" => encode_hex_literal(value, tag),
        other if other.contains("Address") => encode_hex_literal(value, tag),
        "Bytes" | "ByteArray" | "VecU8" => encode_bytes_literal(value, tag),
        _ => Err(type_mismatch(tag)),
    }
}

pub fn create_write_plan(
    id: WritePlanId,
    table: QualifiedTable,
    original_primary_key: CompletePrimaryKey,
    generations: Generations,
    mutation: GuidedMutation,
) -> WritePlan {
    WritePlan {
        id,
        table,
        original_primary_key,
        schema_generation: generations.schema,
        row_generation: generations.row,
        server_context_generation: generations.server_context,
        database_generation: generations.database,
        mutation,
    }
}

pub fn encode_identifier(identifier: &str) -> Result<String, SqlEncodingError> {
    if identifier.is_empty() {
        return Err(SqlEncodingError::EmptyIdentifier);
    }
    if identifier.contains('\0') {
        return Err(SqlEncodingError::NulInIdentifier);
    }
    Ok(format!("\"{}\"", identifier.replace('"', "\"\"")))
}

pub fn build_update_sql(plan: &WritePlan, table: &TableInfo) -> Result<String, SqlEncodingError> {
    let GuidedMutation::Update { changes } = &plan.mutation else {
        return Err(SqlEncodingError::WrongMutationKind);
    };
    if changes.is_empty() {
        return Err(SqlEncodingError::NoChangedFields);
    }

    let mut sorted_changes: Vec<&ColumnChange> = changes.iter().collect();
    sorted_changes.sort_by_key(|change| change.column_id);
    validate_column_changes_against_table(&sorted_changes, table)?;

    let mut assignments = Vec::with_capacity(sorted_changes.len());
    for change in sorted_changes {
        let column = column(table, change.column_id)?;
        let tag = column_type_tag(column)?;
        assignments.push(format!(
            "{} = {}",
            encode_identifier(&column.col_name)?,
            sql_value_to_literal(change.column_id, &change.new_value, &tag)?
        ));
    }

    Ok(format!(
        "UPDATE {} SET {} WHERE {}",
        encode_qualified_table(&plan.table)?,
        assignments.join(", "),
        encode_key_predicate(&plan.original_primary_key, table)?
    ))
}

pub fn build_delete_sql(plan: &WritePlan, table: &TableInfo) -> Result<String, SqlEncodingError> {
    if !matches!(plan.mutation, GuidedMutation::Delete) {
        return Err(SqlEncodingError::WrongMutationKind);
    }
    Ok(format!(
        "DELETE FROM {} WHERE {}",
        encode_qualified_table(&plan.table)?,
        encode_key_predicate(&plan.original_primary_key, table)?
    ))
}

pub fn revalidate_before_dispatch(
    plan: &WritePlan,
    snapshot: &ConfirmationSnapshot<'_>,
) -> Result<(), MutationOutcome> {
    if snapshot.generations.schema != plan.schema_generation {
        return definitely_not_sent("schema generation changed before confirmation");
    }
    if snapshot.generations.row != plan.row_generation {
        return definitely_not_sent("row generation changed before confirmation");
    }
    if snapshot.generations.server_context != plan.server_context_generation {
        return definitely_not_sent("server context generation changed before confirmation");
    }
    if snapshot.generations.database != plan.database_generation {
        return definitely_not_sent("database generation changed before confirmation");
    }
    if snapshot.row_locked {
        return definitely_not_sent("row locked before confirmation");
    }
    if snapshot.table.table_name != plan.table.table {
        return definitely_not_sent("table changed before confirmation");
    }

    let current_key = complete_primary_key_from_table_row(snapshot.table, snapshot.current_raw_row)
        .map_err(|_| MutationOutcome::DefinitelyNotSent {
            reason: "primary key changed before confirmation".into(),
        })?;
    if current_key != plan.original_primary_key {
        return definitely_not_sent("primary key changed before confirmation");
    }

    if let GuidedMutation::Update { changes } = &plan.mutation {
        let change_refs: Vec<&ColumnChange> = changes.iter().collect();
        validate_column_changes_against_table(&change_refs, snapshot.table).map_err(|_| {
            MutationOutcome::DefinitelyNotSent {
                reason: "changed value type no longer matches before confirmation".into(),
            }
        })?;

        for change in changes {
            if let Some(expected_old_value) = &change.old_value {
                let (column_index, column) = column_with_index(snapshot.table, change.column_id)
                    .map_err(|_| MutationOutcome::DefinitelyNotSent {
                        reason: "old value changed before confirmation".into(),
                    })?;
                let Some(raw_value) = snapshot.current_raw_row.get(column_index) else {
                    return definitely_not_sent("old value changed before confirmation");
                };
                let current_value = typed_sql_value_from_json(column, raw_value).map_err(|_| {
                    MutationOutcome::DefinitelyNotSent {
                        reason: "old value changed before confirmation".into(),
                    }
                })?;
                if &current_value != expected_old_value {
                    return definitely_not_sent("old value changed before confirmation");
                }
            }
        }
    }

    Ok(())
}

fn validate_column_changes_against_table(
    changes: &[&ColumnChange],
    table: &TableInfo,
) -> Result<(), SqlEncodingError> {
    for change in changes {
        let column = column(table, change.column_id)?;
        let tag = column_type_tag(column)?;
        if let Some(old_value) = &change.old_value {
            validate_sql_value_for_tag(change.column_id, old_value, &tag)?;
        }
        validate_sql_value_for_tag(change.column_id, &change.new_value, &tag)?;
    }
    Ok(())
}

fn definitely_not_sent<T>(reason: &str) -> Result<T, MutationOutcome> {
    Err(MutationOutcome::DefinitelyNotSent {
        reason: reason.into(),
    })
}

fn encode_qualified_table(table: &QualifiedTable) -> Result<String, SqlEncodingError> {
    match &table.schema {
        Some(schema) => Ok(format!(
            "{}.{}",
            encode_identifier(schema)?,
            encode_identifier(&table.table)?
        )),
        None => encode_identifier(&table.table),
    }
}

fn column(table: &TableInfo, column_id: u32) -> Result<&ColumnInfo, SqlEncodingError> {
    table
        .columns
        .iter()
        .find(|column| column.col_id == column_id)
        .ok_or(SqlEncodingError::UnknownColumnId(column_id))
}

fn column_with_index(
    table: &TableInfo,
    column_id: u32,
) -> Result<(usize, &ColumnInfo), SqlEncodingError> {
    table
        .columns
        .iter()
        .enumerate()
        .find(|(_, column)| column.col_id == column_id)
        .ok_or(SqlEncodingError::UnknownColumnId(column_id))
}

fn column_type_tag(column: &ColumnInfo) -> Result<TypeTag, SqlEncodingError> {
    type_tag(&column.col_type).map_err(|_| SqlEncodingError::TypeMismatch {
        column_id: column.col_id,
        expected: declared_type_name(&column.col_type)
            .unwrap_or("supported scalar type")
            .to_owned(),
    })
}

fn encode_key_predicate(
    key: &CompletePrimaryKey,
    table: &TableInfo,
) -> Result<String, SqlEncodingError> {
    let mut predicates = Vec::with_capacity(key.parts().len());
    for part in key.parts() {
        let column_id = u32::from(part.column_id);
        let column = column(table, column_id)?;
        let tag = column_type_tag(column)?;
        predicates.push(format!(
            "{} = {}",
            encode_identifier(&column.col_name)?,
            sql_value_to_literal(column_id, &part.value, &tag)?
        ));
    }
    Ok(predicates.join(" AND "))
}

fn sql_value_to_literal(
    column_id: u32,
    value: &SqlValue,
    tag: &TypeTag,
) -> Result<String, SqlEncodingError> {
    validate_sql_value_for_tag(column_id, value, tag)?;
    match value {
        SqlValue::Bool(true) => Ok("TRUE".into()),
        SqlValue::Bool(false) => Ok("FALSE".into()),
        SqlValue::I64(value) => Ok(value.to_string()),
        SqlValue::U64(value) => Ok(value.to_string()),
        SqlValue::F64Bits(bits) => {
            let value = f64::from_bits(*bits);
            if value.is_finite() {
                Ok(value.to_string())
            } else {
                Err(SqlEncodingError::FloatNotFinite)
            }
        }
        SqlValue::Text(value) if is_hex_tag(tag.as_str()) => encode_hex_string(value, tag.as_str()),
        SqlValue::Text(value) if is_large_integer_tag(tag.as_str()) => {
            encode_large_integer_string(value, tag.as_str())
        }
        SqlValue::Text(value) => Ok(quote_string_literal(value)),
        SqlValue::Bytes(bytes) => Ok(format!("0x{}", bytes_to_hex(bytes))),
        SqlValue::Null => Err(SqlEncodingError::NullLiteralRejected),
        SqlValue::Unrepresentable(_) => Err(type_mismatch_for_column(column_id, tag.as_str())),
    }
}

fn validate_sql_value_for_tag(
    column_id: u32,
    value: &SqlValue,
    tag: &TypeTag,
) -> Result<(), SqlEncodingError> {
    let matches_tag = match (tag.as_str(), value) {
        (_, SqlValue::Null) => return Err(SqlEncodingError::NullLiteralRejected),
        (_, SqlValue::Unrepresentable(_)) => false,
        ("Bool", SqlValue::Bool(_)) => true,
        ("String", SqlValue::Text(_)) => true,
        ("I8" | "I16" | "I32" | "I64", SqlValue::I64(value)) => {
            integer_fits_signed_tag(*value, tag.as_str())
        }
        ("U8" | "U16" | "U32" | "U64", SqlValue::U64(value)) => {
            integer_fits_unsigned_tag(*value, tag.as_str())
        }
        ("F32" | "F64", SqlValue::F64Bits(bits)) => {
            return validate_float_bits_for_tag(column_id, *bits, tag.as_str());
        }
        (tag, SqlValue::Text(value)) if is_large_integer_tag(tag) => {
            large_integer_string_fits_tag(value, tag)
        }
        (tag, SqlValue::Text(_)) if is_hex_tag(tag) => true,
        ("Bytes" | "ByteArray" | "VecU8", SqlValue::Bytes(_)) => true,
        _ => false,
    };
    if matches_tag {
        Ok(())
    } else {
        Err(type_mismatch_for_column(column_id, tag.as_str()))
    }
}

fn validate_float_bits_for_tag(
    column_id: u32,
    bits: u64,
    tag: &str,
) -> Result<(), SqlEncodingError> {
    let value = f64::from_bits(bits);
    if !value.is_finite() {
        return Err(SqlEncodingError::FloatNotFinite);
    }
    if tag == "F32" {
        let narrowed = value as f32;
        if !narrowed.is_finite() || f64::from(narrowed).to_bits() != bits {
            return Err(type_mismatch_for_column(column_id, tag));
        }
    }
    Ok(())
}

fn value_for_tag<'a>(
    raw_value: &'a serde_json::Value,
    tag: &str,
) -> Result<(&'a serde_json::Value, bool), SqlEncodingError> {
    if raw_value.is_null() {
        return Err(SqlEncodingError::NullLiteralRejected);
    }
    if let serde_json::Value::Object(fields) = raw_value {
        if fields.len() != 1 {
            return Err(type_mismatch(tag));
        }
        let (actual_tag, inner_value) = fields.iter().next().ok_or_else(|| type_mismatch(tag))?;
        if actual_tag == tag || tag_aliases(actual_tag, tag) {
            return Ok((inner_value, true));
        }
        return Err(type_mismatch(tag));
    }
    Ok((raw_value, false))
}

fn tag_aliases(actual_tag: &str, expected_tag: &str) -> bool {
    matches!(
        (actual_tag, expected_tag),
        ("__identity__", "Identity") | ("__connection_id__", "ConnectionId")
    )
}

fn quote_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn encode_signed_64_literal(
    value: &serde_json::Value,
    tag: &str,
) -> Result<String, SqlEncodingError> {
    let value = value.as_i64().ok_or_else(|| type_mismatch(tag))?;
    if integer_fits_signed_tag(value, tag) {
        Ok(value.to_string())
    } else {
        Err(type_mismatch(tag))
    }
}

fn encode_unsigned_64_literal(
    value: &serde_json::Value,
    tag: &str,
) -> Result<String, SqlEncodingError> {
    let value = value.as_u64().ok_or_else(|| type_mismatch(tag))?;
    if integer_fits_unsigned_tag(value, tag) {
        Ok(value.to_string())
    } else {
        Err(type_mismatch(tag))
    }
}

fn integer_fits_signed_tag(value: i64, tag: &str) -> bool {
    let (min, max) = match tag {
        "I8" => (i64::from(i8::MIN), i64::from(i8::MAX)),
        "I16" => (i64::from(i16::MIN), i64::from(i16::MAX)),
        "I32" => (i64::from(i32::MIN), i64::from(i32::MAX)),
        "I64" => (i64::MIN, i64::MAX),
        _ => return false,
    };
    (min..=max).contains(&value)
}

fn integer_fits_unsigned_tag(value: u64, tag: &str) -> bool {
    let max = match tag {
        "U8" => u64::from(u8::MAX),
        "U16" => u64::from(u16::MAX),
        "U32" => u64::from(u32::MAX),
        "U64" => u64::MAX,
        _ => return false,
    };
    value <= max
}

fn encode_float_literal(
    value: &serde_json::Value,
    tag: &str,
    tagged: bool,
) -> Result<String, SqlEncodingError> {
    if tag == "F32" {
        if tagged {
            let bits = f32_bits_from_json(value, tag)?;
            let decoded = f32::from_bits(bits);
            if decoded.is_finite() {
                return Ok(f64::from(decoded).to_string());
            }
            return Err(SqlEncodingError::FloatNotFinite);
        }
        let value = finite_f32_from_json_number(value, tag)?;
        return Ok(f64::from(value).to_string());
    }

    if tagged {
        let bits = value.as_u64().ok_or_else(|| type_mismatch(tag))?;
        validate_float_bits_for_tag(0, bits, tag)?;
        return Ok(f64::from_bits(bits).to_string());
    }
    let value = value.as_f64().ok_or_else(|| type_mismatch(tag))?;
    if value.is_finite() {
        Ok(value.to_string())
    } else {
        Err(SqlEncodingError::FloatNotFinite)
    }
}

fn encode_large_integer_literal(
    value: &serde_json::Value,
    tag: &str,
) -> Result<String, SqlEncodingError> {
    match value {
        serde_json::Value::String(value) => encode_large_integer_string(value, tag),
        serde_json::Value::Number(value) if tag.starts_with('U') => value
            .as_u64()
            .map(|value| value.to_string())
            .ok_or_else(|| type_mismatch(tag)),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(|value| value.to_string())
            .ok_or_else(|| type_mismatch(tag)),
        _ => Err(type_mismatch(tag)),
    }
}

fn encode_large_integer_string(value: &str, tag: &str) -> Result<String, SqlEncodingError> {
    if large_integer_string_fits_tag(value, tag) {
        Ok(value.to_owned())
    } else {
        Err(type_mismatch(tag))
    }
}

fn encode_hex_literal(value: &serde_json::Value, tag: &str) -> Result<String, SqlEncodingError> {
    let value = value.as_str().ok_or_else(|| type_mismatch(tag))?;
    encode_hex_string(value, tag)
}

fn encode_hex_string(value: &str, tag: &str) -> Result<String, SqlEncodingError> {
    let body = value.strip_prefix("0x").unwrap_or(value);
    if !body.is_empty() && body.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Ok(format!("0x{body}"))
    } else {
        Err(type_mismatch(tag))
    }
}

fn encode_bytes_literal(value: &serde_json::Value, tag: &str) -> Result<String, SqlEncodingError> {
    let bytes = bytes_from_json_value(value).ok_or_else(|| type_mismatch(tag))?;
    Ok(format!("0x{}", bytes_to_hex(&bytes)))
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn valid_decimal_integer(value: &str, signed: bool) -> bool {
    let digits = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit())
}

fn f32_bits_from_json(value: &serde_json::Value, tag: &str) -> Result<u32, SqlEncodingError> {
    let bits = value.as_u64().ok_or_else(|| type_mismatch(tag))?;
    u32::try_from(bits).map_err(|_| type_mismatch(tag))
}

fn finite_f32_from_json_number(
    value: &serde_json::Value,
    tag: &str,
) -> Result<f32, SqlEncodingError> {
    let value = value.as_f64().ok_or_else(|| type_mismatch(tag))?;
    if !value.is_finite() {
        return Err(SqlEncodingError::FloatNotFinite);
    }
    let value = value as f32;
    if value.is_finite() {
        Ok(value)
    } else {
        Err(SqlEncodingError::FloatNotFinite)
    }
}

fn bytes_from_json_value(value: &serde_json::Value) -> Option<Vec<u8>> {
    if let Some(value) = value.as_str() {
        return decode_hex_bytes(value);
    }
    let values = value.as_array()?;
    let mut bytes = Vec::with_capacity(values.len());
    for value in values {
        let byte = value.as_u64().and_then(|value| u8::try_from(value).ok())?;
        bytes.push(byte);
    }
    Some(bytes)
}

fn decode_hex_bytes(value: &str) -> Option<Vec<u8>> {
    let body = match value.strip_prefix("0x") {
        Some(body) => body,
        None => value,
    };
    if body.is_empty() || body.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(body.len() / 2);
    for pair in body.as_bytes().chunks(2) {
        if pair.len() != 2 {
            return None;
        }
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn large_integer_string_fits_tag(value: &str, tag: &str) -> bool {
    if let Some(body) = value.strip_prefix("0x") {
        return positive_hex_integer_fits_tag(body, tag);
    }

    match tag {
        "U128" => valid_decimal_integer(value, false) && value.parse::<u128>().is_ok(),
        "I128" => valid_decimal_integer(value, true) && value.parse::<i128>().is_ok(),
        "U256" => decimal_magnitude_fits(value, false, U256_MAX_DECIMAL, U256_MAX_DECIMAL),
        "I256" => decimal_magnitude_fits(value, true, I256_MAX_DECIMAL, I256_MIN_MAG_DECIMAL),
        _ => false,
    }
}

fn positive_hex_integer_fits_tag(body: &str, tag: &str) -> bool {
    if body.is_empty() || !body.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return false;
    }
    let (max_digits, max_hex) = match tag {
        "U128" => (32, U128_MAX_HEX),
        "I128" => (32, I128_MAX_HEX),
        "U256" => (64, U256_MAX_HEX),
        "I256" => (64, I256_MAX_HEX),
        _ => return false,
    };
    body.len() <= max_digits && hex_magnitude_at_most(body, max_hex)
}

fn decimal_magnitude_fits(
    value: &str,
    signed: bool,
    max_positive: &str,
    max_negative: &str,
) -> bool {
    if !valid_decimal_integer(value, signed) {
        return false;
    }
    let (negative, digits) = match value.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, value),
    };
    if negative && !signed {
        return false;
    }
    let max = if negative { max_negative } else { max_positive };
    decimal_magnitude_at_most(digits, max)
}

fn decimal_magnitude_at_most(value: &str, max: &str) -> bool {
    let value = trim_leading_zeroes(value);
    let max = trim_leading_zeroes(max);
    value.len() < max.len() || (value.len() == max.len() && value <= max)
}

fn hex_magnitude_at_most(value: &str, max: &str) -> bool {
    let value = trim_leading_zeroes(value);
    let max = trim_leading_zeroes(max);
    if value.len() != max.len() {
        return value.len() < max.len();
    }
    for (left, right) in value.bytes().zip(max.bytes()) {
        let Some(left) = hex_nibble(left) else {
            return false;
        };
        let Some(right) = hex_nibble(right) else {
            return false;
        };
        if left != right {
            return left < right;
        }
    }
    true
}

fn trim_leading_zeroes(value: &str) -> &str {
    let trimmed = value.trim_start_matches('0');
    if trimmed.is_empty() {
        "0"
    } else {
        trimmed
    }
}

const U256_MAX_DECIMAL: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";
const I256_MAX_DECIMAL: &str =
    "57896044618658097711785492504343953926634992332820282019728792003956564819967";
const I256_MIN_MAG_DECIMAL: &str =
    "57896044618658097711785492504343953926634992332820282019728792003956564819968";
const U128_MAX_HEX: &str = "ffffffffffffffffffffffffffffffff";
const I128_MAX_HEX: &str = "7fffffffffffffffffffffffffffffff";
const U256_MAX_HEX: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const I256_MAX_HEX: &str = "7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

fn is_large_integer_tag(tag: &str) -> bool {
    matches!(tag, "I128" | "I256" | "U128" | "U256")
}

fn is_hex_tag(tag: &str) -> bool {
    tag == "Identity" || tag == "ConnectionId" || tag.contains("Address")
}

fn type_mismatch(tag: &str) -> SqlEncodingError {
    type_mismatch_for_column(0, tag)
}

fn type_mismatch_for_column(column_id: u32, tag: &str) -> SqlEncodingError {
    SqlEncodingError::TypeMismatch {
        column_id,
        expected: tag.into(),
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

    if value.is_object()
        || (value.is_array() && !matches!(declared, "Bytes" | "ByteArray" | "VecU8"))
    {
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

    let (actual_tag, inner_value) = fields.iter().next()?;
    if tag_matches_declared(actual_tag, declared) {
        Some(inner_value)
    } else {
        None
    }
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

    !tag_matches_declared(tag, declared) && is_recognized_scalar_type_or_alias(tag)
}

fn tag_matches_declared(actual_tag: &str, declared: &str) -> bool {
    actual_tag == declared || tag_aliases(actual_tag, declared)
}

fn is_recognized_scalar_type_or_alias(type_name: &str) -> bool {
    is_supported_scalar_type(type_name) || matches!(type_name, "__identity__" | "__connection_id__")
}

fn is_supported_scalar_type(type_name: &str) -> bool {
    matches!(
        type_name,
        "Bool"
            | "I8"
            | "I16"
            | "I32"
            | "I64"
            | "U8"
            | "U16"
            | "U32"
            | "U64"
            | "F32"
            | "F64"
            | "String"
            | "Identity"
            | "ConnectionId"
            | "Address"
            | "U128"
            | "U256"
            | "I128"
            | "I256"
            | "Bytes"
            | "ByteArray"
            | "VecU8"
    ) || type_name.contains("Address")
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
        "U8" | "U16" | "U32" => value
            .as_u64()
            .filter(|value| integer_fits_unsigned_tag(*value, declared))
            .map(SqlValue::U64)
            .ok_or(error_for_mode(mode)),
        "I8" | "I16" | "I32" | "I64" => value
            .as_i64()
            .filter(|value| integer_fits_signed_tag(*value, declared))
            .map(SqlValue::I64)
            .ok_or(error_for_mode(mode)),
        "F32" | "F64" => float_sql_value_from_json(value, declared, mode),
        "Identity" | "ConnectionId" => value
            .as_str()
            .filter(|value| encode_hex_string(value, declared).is_ok())
            .map(|value| SqlValue::Text(value.to_owned()))
            .ok_or(error_for_mode(mode)),
        declared if declared.contains("Address") => value
            .as_str()
            .filter(|value| encode_hex_string(value, declared).is_ok())
            .map(|value| SqlValue::Text(value.to_owned()))
            .ok_or(error_for_mode(mode)),
        "U128" | "U256" | "I128" | "I256" => {
            large_integer_sql_value_from_json(value, declared, mode)
        }
        "Bytes" | "ByteArray" | "VecU8" => bytes_sql_value_from_json(value, mode),
        _ => Err(DeclaredValueError::Unrepresentable),
    }
}

fn float_sql_value_from_json(
    value: &serde_json::Value,
    declared: &str,
    mode: TaggedValueMode,
) -> Result<SqlValue, DeclaredValueError> {
    if declared == "F32" {
        let value = match mode {
            TaggedValueMode::Tagged => {
                let bits = value
                    .as_u64()
                    .and_then(|bits| u32::try_from(bits).ok())
                    .ok_or(error_for_mode(mode))?;
                f32::from_bits(bits)
            }
            TaggedValueMode::Untagged => {
                let value = value.as_f64().ok_or(error_for_mode(mode))?;
                if !value.is_finite() {
                    return Err(DeclaredValueError::Unrepresentable);
                }
                value as f32
            }
        };
        if value.is_finite() {
            return Ok(SqlValue::F64Bits(f64::from(value).to_bits()));
        }
        return Err(DeclaredValueError::Unrepresentable);
    }

    let bits = match value {
        serde_json::Value::Number(number) if mode == TaggedValueMode::Tagged => number.as_u64(),
        serde_json::Value::Number(number) => number.as_f64().map(f64::to_bits),
        _ => None,
    }
    .ok_or(error_for_mode(mode))?;
    if f64::from_bits(bits).is_finite() {
        Ok(SqlValue::F64Bits(bits))
    } else {
        Err(DeclaredValueError::Unrepresentable)
    }
}

fn large_integer_sql_value_from_json(
    value: &serde_json::Value,
    declared: &str,
    mode: TaggedValueMode,
) -> Result<SqlValue, DeclaredValueError> {
    let encoded = match value {
        serde_json::Value::String(value) => encode_large_integer_string(value, declared).ok(),
        serde_json::Value::Number(value) if declared.starts_with('U') => {
            value.as_u64().map(|value| value.to_string())
        }
        serde_json::Value::Number(value) => value.as_i64().map(|value| value.to_string()),
        _ => None,
    };
    encoded.map(SqlValue::Text).ok_or(error_for_mode(mode))
}

fn bytes_sql_value_from_json(
    value: &serde_json::Value,
    mode: TaggedValueMode,
) -> Result<SqlValue, DeclaredValueError> {
    bytes_from_json_value(value)
        .map(SqlValue::Bytes)
        .ok_or(error_for_mode(mode))
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

    fn scalar_col(id: u32, name: &str, type_name: &str) -> ColumnInfo {
        ColumnInfo {
            col_id: id,
            col_name: name.into(),
            col_type: serde_json::json!({ "AlgebraicType": { type_name: {} } }),
            is_autoinc: false,
        }
    }

    fn scalar_table(type_name: &str) -> TableInfo {
        TableInfo {
            table_name: "numbers".into(),
            product_type_ref: 0,
            table_type: "user".into(),
            table_access: "public".into(),
            columns: vec![
                scalar_col(1, "id", type_name),
                scalar_col(2, "value", type_name),
            ],
            primary_key_cols: vec![1],
            indexes: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn bytes_arrays_parse_to_bytes_and_encode_lowercase_even_hex_sql() {
        for type_name in ["Bytes", "ByteArray", "VecU8"] {
            let table_info = scalar_table(type_name);
            let tag = type_tag(&table_info.columns[0].col_type).unwrap();
            let raw = serde_json::json!([0, 1, 15, 255]);
            assert_eq!(
                typed_sql_value_from_json(&table_info.columns[0], &raw).unwrap(),
                SqlValue::Bytes(vec![0, 1, 15, 255])
            );
            assert_eq!(json_to_sql_literal(&raw, &tag).unwrap(), "0x00010fff");

            let tagged = serde_json::json!({ type_name: [171, 205] });
            assert_eq!(
                typed_sql_value_from_json(&table_info.columns[0], &tagged).unwrap(),
                SqlValue::Bytes(vec![0xab, 0xcd])
            );
            assert_eq!(json_to_sql_literal(&tagged, &tag).unwrap(), "0xabcd");

            assert_eq!(
                json_to_sql_literal(&serde_json::json!("0xABcd"), &tag).unwrap(),
                "0xabcd"
            );
            assert_eq!(
                json_to_sql_literal(&serde_json::json!("0xabc"), &tag),
                Err(SqlEncodingError::TypeMismatch {
                    column_id: 0,
                    expected: type_name.into()
                })
            );

            for invalid in [
                serde_json::json!([256]),
                serde_json::json!([-1]),
                serde_json::json!([1.5]),
                serde_json::json!(["1"]),
            ] {
                assert_eq!(
                    typed_sql_value_from_json(&table_info.columns[0], &invalid),
                    Err(PrimaryKeyError::TypeMismatch {
                        column_id: 1,
                        expected: type_name.into()
                    })
                );
                assert_eq!(
                    json_to_sql_literal(&invalid, &tag),
                    Err(SqlEncodingError::TypeMismatch {
                        column_id: 0,
                        expected: type_name.into()
                    })
                );
            }
        }

        let unrelated_array = serde_json::json!([1, 2]);
        assert_eq!(
            typed_sql_value_from_json(&scalar_col(1, "id", "U64"), &unrelated_array),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 1 })
        );
    }

    #[test]
    fn f32_uses_u32_raw_bits_and_stored_f64bits_must_widen_exact_f32() {
        let table_info = scalar_table("F32");
        let f32_tag = type_tag(&table_info.columns[0].col_type).unwrap();
        let f64_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "F64": {} } })).unwrap();
        let one_point_five_bits = 1.5f32.to_bits();
        let widened_one_point_five = f64::from(1.5f32).to_bits();

        assert_eq!(
            typed_sql_value_from_json(
                &table_info.columns[0],
                &serde_json::json!({ "F32": one_point_five_bits })
            )
            .unwrap(),
            SqlValue::F64Bits(widened_one_point_five)
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "F32": one_point_five_bits }), &f32_tag)
                .unwrap(),
            "1.5"
        );

        let rounded = 16_777_217.0_f64;
        assert_eq!(
            typed_sql_value_from_json(&table_info.columns[0], &serde_json::json!(rounded)).unwrap(),
            SqlValue::F64Bits(f64::from(rounded as f32).to_bits())
        );

        assert_eq!(
            typed_sql_value_from_json(
                &table_info.columns[0],
                &serde_json::json!({ "F32": f32::INFINITY.to_bits() })
            ),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 1 })
        );
        assert_eq!(
            json_to_sql_literal(
                &serde_json::json!({ "F32": f32::INFINITY.to_bits() }),
                &f32_tag
            ),
            Err(SqlEncodingError::FloatNotFinite)
        );

        let above_u32 = u64::from(u32::MAX) + 1;
        assert_eq!(
            typed_sql_value_from_json(
                &table_info.columns[0],
                &serde_json::json!({ "F32": above_u32 })
            ),
            Err(PrimaryKeyError::UnrepresentableValue { column_id: 1 })
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "F32": above_u32 }), &f32_tag),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 0,
                expected: "F32".into()
            })
        );

        assert_eq!(
            validate_sql_value_for_tag(77, &SqlValue::F64Bits(widened_one_point_five), &f32_tag),
            Ok(())
        );
        assert_eq!(
            validate_sql_value_for_tag(
                77,
                &SqlValue::F64Bits((f64::from(f32::MAX) * 2.0).to_bits()),
                &f32_tag
            ),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 77,
                expected: "F32".into()
            })
        );
        assert_eq!(
            validate_sql_value_for_tag(77, &SqlValue::F64Bits(0.1f64.to_bits()), &f32_tag),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 77,
                expected: "F32".into()
            })
        );
        assert_eq!(
            validate_sql_value_for_tag(77, &SqlValue::F64Bits(0.1f64.to_bits()), &f64_tag),
            Ok(())
        );
    }

    #[test]
    fn large_integer_widths_are_enforced_for_raw_literals_stored_sql_and_revalidation() {
        use crate::state::safety::{ColumnChange, GuidedMutation, MutationOutcome, QualifiedTable};

        const U128_MAX: &str = "340282366920938463463374607431768211455";
        const U128_OVER: &str = "340282366920938463463374607431768211456";
        const I128_MIN: &str = "-170141183460469231731687303715884105728";
        const I128_MAX: &str = "170141183460469231731687303715884105727";
        const I128_BELOW: &str = "-170141183460469231731687303715884105729";
        const I128_ABOVE: &str = "170141183460469231731687303715884105728";
        const U256_MAX: &str =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        const U256_OVER: &str =
            "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        const I256_MIN: &str =
            "-57896044618658097711785492504343953926634992332820282019728792003956564819968";
        const I256_MAX: &str =
            "57896044618658097711785492504343953926634992332820282019728792003956564819967";
        const I256_BELOW: &str =
            "-57896044618658097711785492504343953926634992332820282019728792003956564819969";
        const I256_ABOVE: &str =
            "57896044618658097711785492504343953926634992332820282019728792003956564819968";

        for (type_name, valid_values, invalid_values) in [
            (
                "U128",
                vec!["0", U128_MAX],
                vec!["-1", U128_OVER, "0x100000000000000000000000000000000"],
            ),
            (
                "I128",
                vec![I128_MIN, I128_MAX, "0x7fffffffffffffffffffffffffffffff"],
                vec![I128_BELOW, I128_ABOVE, "0x80000000000000000000000000000000"],
            ),
            (
                "U256",
                vec![
                    "0",
                    U256_MAX,
                    "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                ],
                vec![
                    "-1",
                    U256_OVER,
                    "0x10000000000000000000000000000000000000000000000000000000000000000",
                ],
            ),
            (
                "I256",
                vec![
                    I256_MIN,
                    I256_MAX,
                    "0x7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                ],
                vec![
                    I256_BELOW,
                    I256_ABOVE,
                    "0x8000000000000000000000000000000000000000000000000000000000000000",
                ],
            ),
        ] {
            let table_info = scalar_table(type_name);
            let tag = type_tag(&table_info.columns[0].col_type).unwrap();
            for value in valid_values {
                let raw = serde_json::json!(value);
                assert_eq!(
                    typed_sql_value_from_json(&table_info.columns[0], &raw).unwrap(),
                    SqlValue::Text(value.into())
                );
                assert_eq!(json_to_sql_literal(&raw, &tag).unwrap(), value);
                assert_eq!(
                    validate_sql_value_for_tag(2, &SqlValue::Text(value.into()), &tag),
                    Ok(())
                );
            }
            for value in invalid_values {
                let raw = serde_json::json!(value);
                assert_eq!(
                    typed_sql_value_from_json(&table_info.columns[0], &raw),
                    Err(PrimaryKeyError::TypeMismatch {
                        column_id: 1,
                        expected: type_name.into()
                    })
                );
                assert_eq!(
                    json_to_sql_literal(&raw, &tag),
                    Err(SqlEncodingError::TypeMismatch {
                        column_id: 0,
                        expected: type_name.into()
                    })
                );
                assert_eq!(
                    validate_sql_value_for_tag(2, &SqlValue::Text(value.into()), &tag),
                    Err(SqlEncodingError::TypeMismatch {
                        column_id: 2,
                        expected: type_name.into()
                    })
                );
            }
        }

        let table_info = scalar_table("U256");
        let key =
            complete_primary_key_from_table_row(&table_info, &[serde_json::json!("1")]).unwrap();
        let invalid_plan = create_write_plan(
            WritePlanId(951),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "numbers".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 2,
                    old_value: Some(SqlValue::Text("1".into())),
                    new_value: SqlValue::Text(U256_OVER.into()),
                }],
            },
        );
        assert_eq!(
            build_update_sql(&invalid_plan, &table_info),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 2,
                expected: "U256".into()
            })
        );
        assert_eq!(
            revalidate_before_dispatch(
                &invalid_plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &table_info,
                    current_raw_row: &[serde_json::json!("1"), serde_json::json!("1")],
                    row_locked: false,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "changed value type no longer matches before confirmation".into()
            })
        );
    }

    #[test]
    fn narrow_integer_widths_are_enforced_for_typed_raw_json_literals_stored_values_and_sql() {
        use crate::state::safety::{ColumnChange, GuidedMutation, QualifiedTable, WritePlanId};

        let cases: &[(&str, i128, i128, i128)] = &[
            ("U8", 0, u8::MAX as i128, u8::MAX as i128 + 1),
            ("U16", 0, u16::MAX as i128, u16::MAX as i128 + 1),
            ("U32", 0, u32::MAX as i128, u32::MAX as i128 + 1),
            ("I8", i8::MIN as i128, i8::MAX as i128, i8::MAX as i128 + 1),
            (
                "I16",
                i16::MIN as i128,
                i16::MAX as i128,
                i16::MAX as i128 + 1,
            ),
            (
                "I32",
                i32::MIN as i128,
                i32::MAX as i128,
                i32::MAX as i128 + 1,
            ),
        ];

        for &(type_name, min, max, too_large) in cases {
            let table_info = scalar_table(type_name);
            let tag = type_tag(&table_info.columns[0].col_type).unwrap();
            let lower_pass = serde_json::json!(min);
            let upper_pass = serde_json::json!(max);
            let upper_fail = serde_json::json!(too_large);

            assert!(typed_sql_value_from_json(&table_info.columns[0], &lower_pass).is_ok());
            assert!(typed_sql_value_from_json(&table_info.columns[0], &upper_pass).is_ok());
            assert_eq!(
                typed_sql_value_from_json(&table_info.columns[0], &upper_fail),
                Err(PrimaryKeyError::TypeMismatch {
                    column_id: 1,
                    expected: type_name.into()
                })
            );
            assert_eq!(
                json_to_sql_literal(&upper_pass, &tag).unwrap(),
                max.to_string()
            );
            assert_eq!(
                json_to_sql_literal(&upper_fail, &tag),
                Err(SqlEncodingError::TypeMismatch {
                    column_id: 0,
                    expected: type_name.into()
                })
            );

            let key =
                complete_primary_key_from_table_row(&table_info, &[lower_pass.clone()]).unwrap();
            let valid_stored = if type_name.starts_with('U') {
                SqlValue::U64(max as u64)
            } else {
                SqlValue::I64(max as i64)
            };
            let invalid_stored = if type_name.starts_with('U') {
                SqlValue::U64(too_large as u64)
            } else {
                SqlValue::I64(too_large as i64)
            };
            let valid_plan = create_write_plan(
                WritePlanId(901),
                QualifiedTable {
                    database: "db".into(),
                    schema: None,
                    table: "numbers".into(),
                },
                key.clone(),
                Generations {
                    schema: 1,
                    row: 2,
                    server_context: 3,
                    database: 4,
                },
                GuidedMutation::Update {
                    changes: vec![ColumnChange {
                        column_id: 2,
                        old_value: None,
                        new_value: valid_stored,
                    }],
                },
            );
            assert!(build_update_sql(&valid_plan, &table_info)
                .unwrap()
                .contains(&format!("\"value\" = {max}")));

            let invalid_plan = create_write_plan(
                WritePlanId(902),
                valid_plan.table.clone(),
                key,
                Generations {
                    schema: 1,
                    row: 2,
                    server_context: 3,
                    database: 4,
                },
                GuidedMutation::Update {
                    changes: vec![ColumnChange {
                        column_id: 2,
                        old_value: None,
                        new_value: invalid_stored,
                    }],
                },
            );
            assert_eq!(
                build_update_sql(&invalid_plan, &table_info),
                Err(SqlEncodingError::TypeMismatch {
                    column_id: 2,
                    expected: type_name.into()
                })
            );
        }

        for &(type_name, below) in &[
            ("I8", i8::MIN as i128 - 1),
            ("I16", i16::MIN as i128 - 1),
            ("I32", i32::MIN as i128 - 1),
        ] {
            let table_info = scalar_table(type_name);
            let tag = type_tag(&table_info.columns[0].col_type).unwrap();
            let below = serde_json::json!(below);
            assert_eq!(
                typed_sql_value_from_json(&table_info.columns[0], &below),
                Err(PrimaryKeyError::TypeMismatch {
                    column_id: 1,
                    expected: type_name.into()
                })
            );
            assert_eq!(
                json_to_sql_literal(&below, &tag),
                Err(SqlEncodingError::TypeMismatch {
                    column_id: 0,
                    expected: type_name.into()
                })
            );
        }
    }

    #[test]
    fn identity_and_connection_id_raw_aliases_parse_for_pk_revalidation_and_hex_sql() {
        use crate::state::safety::{ColumnChange, GuidedMutation};

        let mut identity_table = TableInfo {
            table_name: "sessions".into(),
            product_type_ref: 0,
            table_type: "user".into(),
            table_access: "public".into(),
            columns: vec![
                scalar_col(1, "id", "Identity"),
                scalar_col(2, "owner", "Identity"),
            ],
            primary_key_cols: vec![1],
            indexes: vec![],
            constraints: vec![],
        };
        let key = complete_primary_key_from_table_row(
            &identity_table,
            &[
                serde_json::json!({ "__identity__": "1234" }),
                serde_json::json!("abcd"),
            ],
        )
        .unwrap();
        assert_eq!(key.parts()[0].value, SqlValue::Text("1234".into()));
        let delete_plan = create_write_plan(
            WritePlanId(910),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "sessions".into(),
            },
            key.clone(),
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Delete,
        );
        assert_eq!(
            build_delete_sql(&delete_plan, &identity_table).unwrap(),
            "DELETE FROM \"sessions\" WHERE \"id\" = 0x1234"
        );

        let update_plan = create_write_plan(
            WritePlanId(911),
            delete_plan.table.clone(),
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 2,
                    old_value: Some(SqlValue::Text("abcd".into())),
                    new_value: SqlValue::Text("dcba".into()),
                }],
            },
        );
        assert_eq!(
            revalidate_before_dispatch(
                &update_plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &identity_table,
                    current_raw_row: &[
                        serde_json::json!({ "__identity__": "1234" }),
                        serde_json::json!({ "__identity__": "abcd" }),
                    ],
                    row_locked: false,
                }
            ),
            Ok(())
        );

        identity_table.columns[0] = scalar_col(1, "id", "ConnectionId");
        identity_table.columns[1] = scalar_col(2, "owner", "ConnectionId");
        let connection_key = complete_primary_key_from_table_row(
            &identity_table,
            &[
                serde_json::json!({ "__connection_id__": "cafe" }),
                serde_json::json!("beef"),
            ],
        )
        .unwrap();
        assert_eq!(
            connection_key.parts()[0].value,
            SqlValue::Text("cafe".into())
        );
        let connection_plan = create_write_plan(
            WritePlanId(912),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "sessions".into(),
            },
            connection_key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Delete,
        );
        assert_eq!(
            build_delete_sql(&connection_plan, &identity_table).unwrap(),
            "DELETE FROM \"sessions\" WHERE \"id\" = 0xcafe"
        );
        assert_eq!(
            typed_sql_value_from_json(
                &identity_table.columns[0],
                &serde_json::json!({ "Identity": "cafe" })
            ),
            Err(PrimaryKeyError::TypeMismatch {
                column_id: 1,
                expected: "ConnectionId".into()
            })
        );
    }

    #[test]
    fn confirmation_revalidates_update_values_against_current_snapshot_column_types() {
        use crate::state::safety::{ColumnChange, GuidedMutation, MutationOutcome};

        let original_table = table(vec![7]);
        let key = complete_primary_key_from_table_row(
            &original_table,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();
        let plan = create_write_plan(
            WritePlanId(920),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 9,
                    old_value: Some(SqlValue::Text("Ada".into())),
                    new_value: SqlValue::Text("Grace".into()),
                }],
            },
        );

        let mut changed_type = original_table;
        changed_type.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "Bool": {} } });
        let current = [
            serde_json::json!(42),
            serde_json::json!("eu"),
            serde_json::json!(true),
        ];

        assert_eq!(
            revalidate_before_dispatch(
                &plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &changed_type,
                    current_raw_row: &current,
                    row_locked: false,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "changed value type no longer matches before confirmation".into()
            })
        );
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

    #[test]
    fn update_sql_uses_original_complete_key_and_u32_column_changes() {
        use crate::state::safety::{ColumnChange, GuidedMutation, QualifiedTable, WritePlanId};

        let table_info = table(vec![2, 7]);
        let key = complete_primary_key_from_table_row(
            &table_info,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();
        let plan = create_write_plan(
            WritePlanId(11),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 9,
                    old_value: Some(SqlValue::Text("Ada".into())),
                    new_value: SqlValue::Text("Grace".into()),
                }],
            },
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
        let key = complete_primary_key_from_table_row(
            &table_info,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();
        let plan = create_write_plan(
            WritePlanId(12),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Delete,
        );

        assert_eq!(
            build_delete_sql(&plan, &table_info).unwrap(),
            "DELETE FROM \"inventory\" WHERE \"account\" = 42 AND \"region\" = 'eu'"
        );
    }

    #[test]
    fn shared_encoder_preserves_identity_connection_bigint_quote_and_identifier_semantics() {
        let identity =
            type_tag(&serde_json::json!({ "AlgebraicType": { "Identity": {} } })).unwrap();
        let connection =
            type_tag(&serde_json::json!({ "AlgebraicType": { "ConnectionId": {} } })).unwrap();
        let u128_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "U128": {} } })).unwrap();
        let u256_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "U256": {} } })).unwrap();
        let string_tag =
            type_tag(&serde_json::json!({ "AlgebraicType": { "String": {} } })).unwrap();

        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "Identity": "0x1234" }), &identity).unwrap(),
            "0x1234"
        );
        assert_eq!(
            json_to_sql_literal(
                &serde_json::json!({ "ConnectionId": "0xabcd" }),
                &connection
            )
            .unwrap(),
            "0xabcd"
        );
        assert_eq!(
            json_to_sql_literal(
                &serde_json::json!({ "U128": "340282366920938463463374607431768211455" }),
                &u128_tag
            )
            .unwrap(),
            "340282366920938463463374607431768211455"
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "U256": "0x01" }), &u256_tag).unwrap(),
            "0x01"
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!("O'Brien"), &string_tag).unwrap(),
            "'O''Brien'"
        );
        assert_eq!(
            encode_identifier("weird\"name").unwrap(),
            "\"weird\"\"name\""
        );
    }

    #[test]
    fn changes_are_validated_against_declared_column_type_before_sql_encoding() {
        use crate::state::safety::{ColumnChange, GuidedMutation, QualifiedTable, WritePlanId};

        let mut table_info = table(vec![7]);
        table_info.columns[2].col_type = serde_json::json!({ "AlgebraicType": { "String": {} } });
        let key = complete_primary_key_from_table_row(
            &table_info,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();
        let plan = create_write_plan(
            WritePlanId(14),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 9,
                    old_value: Some(SqlValue::Text("Ada".into())),
                    new_value: SqlValue::Bool(true),
                }],
            },
        );

        assert_eq!(
            build_update_sql(&plan, &table_info),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 9,
                expected: "String".into()
            })
        );
    }

    #[test]
    fn confirmation_revalidation_rejects_stale_row_before_dispatch() {
        use crate::state::safety::{GuidedMutation, MutationOutcome, QualifiedTable, WritePlanId};

        let table_info = table(vec![7]);
        let key =
            complete_primary_key_from_table_row(&table_info, &[serde_json::json!(42)]).unwrap();
        let plan = create_write_plan(
            WritePlanId(13),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key.clone(),
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Delete,
        );

        assert_eq!(
            revalidate_before_dispatch(
                &plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 99,
                        server_context: 3,
                        database: 4,
                    },
                    table: &table_info,
                    current_raw_row: &[serde_json::json!(42)],
                    row_locked: false,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "row generation changed before confirmation".into()
            })
        );
    }

    #[test]
    fn update_sql_rejects_wrong_mutation_empty_changes_unknown_columns_and_escapes_identifiers() {
        use crate::state::safety::{ColumnChange, GuidedMutation, QualifiedTable, WritePlanId};

        let mut table_info = table(vec![7]);
        table_info.table_name = "ignored".into();
        table_info.columns[1].col_name = "reg\"ion".into();
        table_info.columns[2].col_name = "na\"me".into();
        let key = complete_primary_key_from_table_row(
            &table_info,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();

        let delete_plan = create_write_plan(
            WritePlanId(20),
            QualifiedTable {
                database: "db".into(),
                schema: Some("public\"schema".into()),
                table: "inv\"entory".into(),
            },
            key.clone(),
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Delete,
        );
        assert_eq!(
            build_update_sql(&delete_plan, &table_info),
            Err(SqlEncodingError::WrongMutationKind)
        );

        let empty_plan = create_write_plan(
            WritePlanId(21),
            delete_plan.table.clone(),
            key.clone(),
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update { changes: vec![] },
        );
        assert_eq!(
            build_update_sql(&empty_plan, &table_info),
            Err(SqlEncodingError::NoChangedFields)
        );

        let unknown_plan = create_write_plan(
            WritePlanId(22),
            empty_plan.table.clone(),
            key.clone(),
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 404,
                    old_value: None,
                    new_value: SqlValue::Text("Grace".into()),
                }],
            },
        );
        assert_eq!(
            build_update_sql(&unknown_plan, &table_info),
            Err(SqlEncodingError::UnknownColumnId(404))
        );

        let escaped_plan = create_write_plan(
            WritePlanId(23),
            empty_plan.table,
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 9,
                    old_value: Some(SqlValue::Text("Ada".into())),
                    new_value: SqlValue::Text("Grace".into()),
                }],
            },
        );
        assert_eq!(
            build_update_sql(&escaped_plan, &table_info).unwrap(),
            "UPDATE \"public\"\"schema\".\"inv\"\"entory\" SET \"na\"\"me\" = 'Grace' WHERE \"account\" = 42"
        );
    }

    #[test]
    fn encoder_rejects_non_finite_bits_invalid_identity_and_invalid_large_int_tags() {
        let f64_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "F64": {} } })).unwrap();
        let identity =
            type_tag(&serde_json::json!({ "AlgebraicType": { "Identity": {} } })).unwrap();
        let u128_tag = type_tag(&serde_json::json!({ "AlgebraicType": { "U128": {} } })).unwrap();

        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "F64": f64::NAN.to_bits() }), &f64_tag),
            Err(SqlEncodingError::FloatNotFinite)
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "Identity": "not-hex" }), &identity),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 0,
                expected: "Identity".into()
            })
        );
        assert_eq!(
            json_to_sql_literal(&serde_json::json!({ "U128": "12abc" }), &u128_tag),
            Err(SqlEncodingError::TypeMismatch {
                column_id: 0,
                expected: "U128".into()
            })
        );
    }

    #[test]
    fn revalidation_rejects_all_generation_row_lock_pk_and_old_value_drift_categories() {
        use crate::state::safety::{
            ColumnChange, GuidedMutation, MutationOutcome, QualifiedTable, WritePlanId,
        };

        let table_info = table(vec![7]);
        let key = complete_primary_key_from_table_row(
            &table_info,
            &[
                serde_json::json!(42),
                serde_json::json!("eu"),
                serde_json::json!("Ada"),
            ],
        )
        .unwrap();
        let plan = create_write_plan(
            WritePlanId(30),
            QualifiedTable {
                database: "db".into(),
                schema: None,
                table: "inventory".into(),
            },
            key,
            Generations {
                schema: 1,
                row: 2,
                server_context: 3,
                database: 4,
            },
            GuidedMutation::Update {
                changes: vec![ColumnChange {
                    column_id: 9,
                    old_value: Some(SqlValue::Text("Ada".into())),
                    new_value: SqlValue::Text("Grace".into()),
                }],
            },
        );
        let current = [
            serde_json::json!(42),
            serde_json::json!("eu"),
            serde_json::json!("Ada"),
        ];

        for (generations, reason) in [
            (
                Generations {
                    schema: 9,
                    row: 2,
                    server_context: 3,
                    database: 4,
                },
                "schema generation changed before confirmation",
            ),
            (
                Generations {
                    schema: 1,
                    row: 2,
                    server_context: 9,
                    database: 4,
                },
                "server context generation changed before confirmation",
            ),
            (
                Generations {
                    schema: 1,
                    row: 2,
                    server_context: 3,
                    database: 9,
                },
                "database generation changed before confirmation",
            ),
        ] {
            assert_eq!(
                revalidate_before_dispatch(
                    &plan,
                    &ConfirmationSnapshot {
                        generations,
                        table: &table_info,
                        current_raw_row: &current,
                        row_locked: false,
                    }
                ),
                Err(MutationOutcome::DefinitelyNotSent {
                    reason: reason.into()
                })
            );
        }

        assert_eq!(
            revalidate_before_dispatch(
                &plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &table_info,
                    current_raw_row: &current,
                    row_locked: true,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "row locked before confirmation".into()
            })
        );

        let pk_drift = [
            serde_json::json!(43),
            serde_json::json!("eu"),
            serde_json::json!("Ada"),
        ];
        assert_eq!(
            revalidate_before_dispatch(
                &plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &table_info,
                    current_raw_row: &pk_drift,
                    row_locked: false,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "primary key changed before confirmation".into()
            })
        );

        let old_value_drift = [
            serde_json::json!(42),
            serde_json::json!("eu"),
            serde_json::json!("Changed"),
        ];
        assert_eq!(
            revalidate_before_dispatch(
                &plan,
                &ConfirmationSnapshot {
                    generations: Generations {
                        schema: 1,
                        row: 2,
                        server_context: 3,
                        database: 4,
                    },
                    table: &table_info,
                    current_raw_row: &old_value_drift,
                    row_locked: false,
                }
            ),
            Err(MutationOutcome::DefinitelyNotSent {
                reason: "old value changed before confirmation".into()
            })
        );
    }
}
