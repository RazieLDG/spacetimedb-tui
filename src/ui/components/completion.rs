//! SQL auto-completion helpers.
//!
//! The SQL console supports Tab-completion: the user types a prefix like
//! `SEL`, `us`, or `use.na`, and pressing `Tab` expands it to the longest
//! unambiguous match from the set of available candidates (SQL keywords,
//! table names, and column names from the current schema).
//!
//! This module is deliberately dependency-free so it can be unit-tested
//! without the full TUI / state machinery.

/// The SQL keywords we offer for completion. Kept lower-case here and
/// preserved as-is when emitted — the completer matches case-insensitively
/// but returns the candidate in its canonical form.
pub const SQL_KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "IS",
    "NULL",
    "LIMIT",
    "OFFSET",
    "ORDER BY",
    "GROUP BY",
    "HAVING",
    "JOIN",
    "LEFT JOIN",
    "INNER JOIN",
    "ON",
    "AS",
    "INSERT",
    "INTO",
    "VALUES",
    "UPDATE",
    "SET",
    "DELETE",
    "CREATE",
    "TABLE",
    "DROP",
    "INDEX",
    "ASC",
    "DESC",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "DISTINCT",
    "TRUE",
    "FALSE",
];

/// Result of a single Tab-completion attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResult {
    /// No candidate matched the prefix — nothing to do.
    NoMatch,
    /// Exactly one candidate matched; commit it directly.
    Unique(String),
    /// Multiple candidates matched. The user should see the list, and
    /// the input should be extended to the longest common prefix
    /// (which may equal the original prefix if there is nothing to
    /// share beyond it).
    Multiple {
        common_prefix: String,
        candidates: Vec<String>,
    },
}

/// Compute a [`CompletionResult`] for a case-insensitive `prefix` against
/// a flat `candidates` slice. Candidates are returned in the order they
/// first appear in the input; duplicates are skipped.
pub fn complete(prefix: &str, candidates: &[&str]) -> CompletionResult {
    let prefix_lower = prefix.to_ascii_lowercase();

    let mut matched: Vec<String> = Vec::new();
    for &c in candidates {
        if c.to_ascii_lowercase().starts_with(&prefix_lower) && !matched.iter().any(|m| m == c) {
            matched.push(c.to_string());
        }
    }

    match matched.len() {
        0 => CompletionResult::NoMatch,
        1 => CompletionResult::Unique(matched.into_iter().next().unwrap()),
        _ => {
            let common_prefix = longest_common_prefix_ci(&matched);
            CompletionResult::Multiple {
                common_prefix,
                candidates: matched,
            }
        }
    }
}

/// Longest common prefix across all `items`, compared case-insensitively
/// but returning the characters from the *first* item so that the
/// canonical casing wins (e.g. `SELECT` rather than `select`).
fn longest_common_prefix_ci(items: &[String]) -> String {
    if items.is_empty() {
        return String::new();
    }
    let first = &items[0];
    let first_lower: Vec<char> = first.chars().map(|c| c.to_ascii_lowercase()).collect();

    let mut common_len = first_lower.len();
    for item in items.iter().skip(1) {
        let item_lower: Vec<char> = item.chars().map(|c| c.to_ascii_lowercase()).collect();
        let mut n = 0;
        while n < common_len && n < item_lower.len() && first_lower[n] == item_lower[n] {
            n += 1;
        }
        common_len = n;
        if common_len == 0 {
            break;
        }
    }

    first.chars().take(common_len).collect()
}

/// Build the full candidate list for a SQL prompt: keywords + the table
/// names + every column across every table in the supplied iterator.
///
/// This is intentionally a flat list — we don't try to be context-aware
/// (e.g. "only suggest columns after SELECT"). That pays off in simplicity
/// and avoids a real parser; the user just gets "everything that could
/// come next".
pub fn build_candidates<'a, I, T>(tables: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a T>,
    T: TableLike + 'a,
{
    let mut out: Vec<String> = SQL_KEYWORDS.iter().map(|k| (*k).to_string()).collect();
    for table in tables {
        out.push(table.name().to_string());
        for col in table.column_names() {
            out.push(col);
        }
    }
    // Stable de-duplication preserving order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|item| seen.insert(item.to_ascii_lowercase()));
    out
}

/// Abstraction over `TableInfo` so tests don't depend on the full
/// `crate::api::types` module.
pub trait TableLike {
    fn name(&self) -> &str;
    fn column_names(&self) -> Vec<String>;
}

impl TableLike for crate::api::types::TableInfo {
    fn name(&self) -> &str {
        &self.table_name
    }
    fn column_names(&self) -> Vec<String> {
        self.columns.iter().map(|c| c.col_name.clone()).collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeTable {
        name: &'static str,
        cols: &'static [&'static str],
    }
    impl TableLike for FakeTable {
        fn name(&self) -> &str {
            self.name
        }
        fn column_names(&self) -> Vec<String> {
            self.cols.iter().map(|c| c.to_string()).collect()
        }
    }

    #[test]
    fn complete_no_match() {
        let cs = ["SELECT", "FROM"];
        assert_eq!(complete("xyz", &cs), CompletionResult::NoMatch);
    }

    #[test]
    fn complete_unique() {
        let cs = ["SELECT", "FROM", "WHERE"];
        assert_eq!(
            complete("sel", &cs),
            CompletionResult::Unique("SELECT".to_string())
        );
    }

    #[test]
    fn complete_multiple_extends_to_common_prefix() {
        let cs = ["users", "user_id", "user_name"];
        let result = complete("us", &cs);
        match result {
            CompletionResult::Multiple {
                common_prefix,
                candidates,
            } => {
                assert_eq!(common_prefix, "user");
                assert_eq!(candidates.len(), 3);
            }
            other => panic!("expected Multiple, got {other:?}"),
        }
    }

    #[test]
    fn complete_case_insensitive() {
        let cs = ["SELECT", "FROM"];
        assert_eq!(
            complete("SEL", &cs),
            CompletionResult::Unique("SELECT".to_string())
        );
        assert_eq!(
            complete("sel", &cs),
            CompletionResult::Unique("SELECT".to_string())
        );
    }

    #[test]
    fn complete_skips_duplicates() {
        let cs = ["SELECT", "SELECT", "FROM"];
        assert_eq!(
            complete("sel", &cs),
            CompletionResult::Unique("SELECT".to_string())
        );
    }

    #[test]
    fn build_candidates_includes_keywords_and_schema() {
        let tables = [
            FakeTable {
                name: "users",
                cols: &["id", "name"],
            },
            FakeTable {
                name: "orders",
                cols: &["id", "user_id"],
            },
        ];
        let cs = build_candidates(&tables);
        assert!(cs.iter().any(|c| c == "SELECT"));
        assert!(cs.iter().any(|c| c == "users"));
        assert!(cs.iter().any(|c| c == "orders"));
        assert!(cs.iter().any(|c| c == "user_id"));
        // `id` appears in both tables — should only be listed once.
        let id_count = cs.iter().filter(|c| c.as_str() == "id").count();
        assert_eq!(id_count, 1);
    }

    #[test]
    fn longest_common_prefix_preserves_first_casing() {
        let items = vec!["SELECT".to_string(), "select_all".to_string()];
        assert_eq!(longest_common_prefix_ci(&items), "SELECT");
    }
}
