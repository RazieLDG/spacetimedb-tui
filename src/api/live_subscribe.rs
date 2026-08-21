//! Bounded live-subscription SQL.
//!
//! SpacetimeDB (and the JSON WebSocket client) reject oversized frames.
//! A `SELECT * FROM huge_table` snapshot is sent as one message, so Live
//! must subscribe to a small window and split that window into several
//! `SubscribeSingle` queries. HTTP browse (`LIMIT 200`) remains the
//! historical snapshot; Live only watches recent / new rows.

use crate::api::types::{QueryResult, TableInfo};

/// Default snapshot window: last two seconds of timestamped rows.
pub const DEFAULT_LIVE_WINDOW_US: i64 = 2_000_000;
/// Split a time/key window into this many SubscribeSingle queries so
/// each server message stays under the WebSocket size cap.
pub const LIVE_SUBSCRIBE_CHUNKS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveSubscribePlan {
    /// One or more bounded `SELECT * FROM t WHERE …` queries.
    Queries(Vec<String>),
    /// No safe bound exists yet (waiting on browse) or the table cannot
    /// be windowed. Caller must not send unbounded `SELECT *`.
    NotReady { reason: &'static str },
}

/// Build chunked, bounded subscription queries for `table`.
///
/// Preference order:
/// 1. Integer timestamp column (`*_us`, `*_at`, `ts`, `created`, …)
/// 2. Integer primary key, using the max value already on screen as a
///    high-water mark so the snapshot is "new rows only"
pub fn plan_live_subscribe(
    table: &TableInfo,
    now_us: i64,
    window_us: i64,
    browse: Option<&QueryResult>,
) -> LiveSubscribePlan {
    let Ok(table_ident) = quote_ident(&table.table_name) else {
        return LiveSubscribePlan::NotReady {
            reason: "invalid table name",
        };
    };

    if let Some(column) = timestamp_column(table) {
        let Ok(col_ident) = quote_ident(&column.col_name) else {
            return LiveSubscribePlan::NotReady {
                reason: "invalid timestamp column",
            };
        };
        let lower = now_us.saturating_sub(window_us.max(0));
        return LiveSubscribePlan::Queries(chunk_inequality_queries(
            &table_ident,
            &col_ident,
            lower,
            now_us,
        ));
    }

    if let Some(column) = integer_pk_column(table) {
        let Ok(col_ident) = quote_ident(&column.col_name) else {
            return LiveSubscribePlan::NotReady {
                reason: "invalid primary key column",
            };
        };
        let Some(high_water) = browse.and_then(|result| column_max_i64(result, &column.col_name))
        else {
            return LiveSubscribePlan::NotReady {
                reason: "waiting for table browse before live tail",
            };
        };
        return LiveSubscribePlan::Queries(vec![format!(
            "SELECT * FROM {table_ident} WHERE {col_ident} > {high_water}"
        )]);
    }

    LiveSubscribePlan::NotReady {
        reason: "table has no timestamp or integer primary key to bound live",
    }
}

fn quote_ident(name: &str) -> Result<String, ()> {
    if name.is_empty() || name.contains('\0') {
        return Err(());
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

fn type_name(col_type: &serde_json::Value) -> String {
    match col_type {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(o) if o.len() == 1 => {
            let Some((name, value)) = o.iter().next() else {
                return String::new();
            };
            if name == "AlgebraicType" {
                type_name(value)
            } else {
                name.clone()
            }
        }
        _ => String::new(),
    }
}

fn is_integer_type(tag: &str) -> bool {
    matches!(
        tag,
        "I8" | "I16" | "I32" | "I64" | "I128" | "U8" | "U16" | "U32" | "U64" | "U128"
    )
}

fn timestamp_column(table: &TableInfo) -> Option<&crate::api::types::ColumnInfo> {
    let mut best: Option<(i32, &crate::api::types::ColumnInfo)> = None;
    for column in &table.columns {
        if !is_integer_type(&type_name(&column.col_type)) {
            continue;
        }
        let name = column.col_name.to_ascii_lowercase();
        let score = if name.ends_with("_us") || name.ends_with("_micros") || name.ends_with("_µs")
        {
            4
        } else if name.contains("timestamp") || name == "ts" || name.ends_with("_at") {
            3
        } else if name.contains("created") || name.contains("updated") || name.contains("time") {
            2
        } else {
            0
        };
        if score == 0 {
            continue;
        }
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, column));
        }
    }
    best.map(|(_, column)| column)
}

fn integer_pk_column(table: &TableInfo) -> Option<&crate::api::types::ColumnInfo> {
    let pk = *table.primary_key_cols.first()?;
    let column = table
        .columns
        .iter()
        .find(|column| column.col_id == u32::from(pk))?;
    if is_integer_type(&type_name(&column.col_type)) {
        Some(column)
    } else {
        None
    }
}

fn column_max_i64(result: &QueryResult, column: &str) -> Option<i64> {
    let index = result.schema.iter().position(|col| col.name == column)?;
    result
        .rows
        .iter()
        .filter_map(|row| row.get(index).and_then(value_as_i64))
        .max()
}

fn value_as_i64(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_u64().and_then(|n| i64::try_from(n).ok())),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

/// Split `col > lower` into disjoint half-open chunks plus an open tail
/// (`col > last`) that receives new rows.
fn chunk_inequality_queries(
    table_ident: &str,
    col_ident: &str,
    lower: i64,
    now_us: i64,
) -> Vec<String> {
    let span = now_us.saturating_sub(lower).max(1);
    let chunks = LIVE_SUBSCRIBE_CHUNKS.max(1);
    let step = (span / chunks as i64).max(1);
    let mut queries = Vec::with_capacity(chunks);
    let mut start = lower;
    for index in 0..chunks {
        if index + 1 == chunks {
            queries.push(format!(
                "SELECT * FROM {table_ident} WHERE {col_ident} > {start}"
            ));
        } else {
            let end = start.saturating_add(step);
            queries.push(format!(
                "SELECT * FROM {table_ident} WHERE {col_ident} > {start} AND {col_ident} <= {end}"
            ));
            start = end;
        }
    }
    queries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::types::{ColumnInfo, SchemaElement};

    fn col(id: u32, name: &str, ty: &str) -> ColumnInfo {
        ColumnInfo {
            col_id: id,
            col_name: name.to_string(),
            col_type: serde_json::json!({ ty: [] }),
            is_autoinc: false,
        }
    }

    fn table(name: &str, columns: Vec<ColumnInfo>, pk: Vec<u16>) -> TableInfo {
        TableInfo {
            table_name: name.to_string(),
            product_type_ref: 0,
            table_type: "user".to_string(),
            table_access: "public".to_string(),
            columns,
            primary_key_cols: pk,
            indexes: vec![],
            constraints: vec![],
        }
    }

    #[test]
    fn timestamp_window_is_split_into_subscribe_single_chunks() {
        let table = table(
            "live_event",
            vec![col(0, "id", "String"), col(1, "created_at_us", "I64")],
            vec![0],
        );
        let now = 2_000_000;
        let plan = plan_live_subscribe(&table, now, 2_000_000, None);
        let LiveSubscribePlan::Queries(queries) = plan else {
            panic!("expected queries");
        };
        assert_eq!(queries.len(), LIVE_SUBSCRIBE_CHUNKS);
        assert!(queries[0].starts_with(r#"SELECT * FROM "live_event" WHERE "created_at_us" > 0"#));
        assert!(queries[0].contains("AND"));
        assert_eq!(
            queries.last().unwrap().as_str(),
            r#"SELECT * FROM "live_event" WHERE "created_at_us" > 1500000"#
        );
        assert!(
            !queries.iter().any(|q| q == r#"SELECT * FROM "live_event""#),
            "unbounded SELECT * is never emitted"
        );
    }

    #[test]
    fn integer_pk_waits_for_browse_then_tails_new_rows() {
        let table = table(
            "items",
            vec![col(0, "id", "U64"), col(1, "name", "String")],
            vec![0],
        );
        assert!(matches!(
            plan_live_subscribe(&table, 0, DEFAULT_LIVE_WINDOW_US, None),
            LiveSubscribePlan::NotReady { .. }
        ));

        let browse = QueryResult {
            schema: vec![
                SchemaElement {
                    name: "id".to_string(),
                    algebraic_type: serde_json::json!("U64"),
                },
                SchemaElement {
                    name: "name".to_string(),
                    algebraic_type: serde_json::json!("String"),
                },
            ],
            rows: vec![
                vec![serde_json::json!(10), serde_json::json!("a")],
                vec![serde_json::json!(41), serde_json::json!("b")],
            ],
            total_duration_micros: 1,
        };
        let plan = plan_live_subscribe(&table, 0, DEFAULT_LIVE_WINDOW_US, Some(&browse));
        assert_eq!(
            plan,
            LiveSubscribePlan::Queries(
                vec![r#"SELECT * FROM "items" WHERE "id" > 41"#.to_string()]
            )
        );
    }

    #[test]
    fn string_pk_without_timestamp_is_not_unbounded() {
        let table = table(
            "kv",
            vec![col(0, "key", "String"), col(1, "value", "String")],
            vec![0],
        );
        assert!(matches!(
            plan_live_subscribe(&table, 0, DEFAULT_LIVE_WINDOW_US, None),
            LiveSubscribePlan::NotReady { .. }
        ));
    }
}
