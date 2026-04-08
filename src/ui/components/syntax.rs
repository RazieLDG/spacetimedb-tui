//! Tiny SQL tokenizer used for syntax highlighting in the SQL console.
//!
//! This is deliberately not a parser: it only needs to split the input
//! into coloured spans (keywords, identifiers, strings, numbers,
//! punctuation, whitespace). Multi-word keywords like `ORDER BY` are not
//! recognised — each bare identifier is classified on its own and
//! `ORDER` + `BY` both render as keywords anyway.

use std::ops::Range;

/// A single syntactic category for a slice of the SQL input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// SQL keyword (case-insensitive match against [`KEYWORDS`]).
    Keyword,
    /// Regular identifier / table / column reference.
    Identifier,
    /// Single or double-quoted string literal.
    StringLit,
    /// Numeric literal (int or float).
    Number,
    /// Any punctuation / operator character.
    Punct,
    /// Whitespace run — kept so the renderer can reproduce the input
    /// byte-for-byte without gaps.
    Whitespace,
}

/// A tokenised slice of the input referenced by byte range.
///
/// `Range<usize>` doesn't implement `Copy`, so `Token` can't either;
/// that's fine — the tokenizer output is produced once per render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub range: Range<usize>,
}

/// Uppercase list of single-word SQL keywords recognised by the
/// highlighter. Kept in sync with [`super::completion::SQL_KEYWORDS`]
/// where it makes sense (multi-word entries like `"ORDER BY"` are not
/// repeated here because the tokenizer only sees one word at a time).
pub const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIMIT", "OFFSET", "ORDER",
    "BY", "GROUP", "HAVING", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "ON",
    "AS", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "TABLE", "DROP",
    "INDEX", "ASC", "DESC", "COUNT", "SUM", "AVG", "MIN", "MAX", "DISTINCT", "TRUE", "FALSE",
    "CASE", "WHEN", "THEN", "ELSE", "END", "LIKE", "BETWEEN", "UNION", "ALL",
];

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_keyword(word: &str) -> bool {
    let upper: String = word.chars().map(|c| c.to_ascii_uppercase()).collect();
    KEYWORDS.contains(&upper.as_str())
}

/// Tokenise `input` into a flat stream of [`Token`]s that cover the
/// entire string without gaps (so the renderer can concatenate them
/// back into the original text, styled by token kind).
pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let start = i;
        let b = bytes[i];

        if b.is_ascii_whitespace() {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::Whitespace,
                range: start..i,
            });
            continue;
        }

        if is_ident_start(b) {
            i += 1;
            while i < bytes.len() && is_ident_cont(bytes[i]) {
                i += 1;
            }
            let word = &input[start..i];
            let kind = if is_keyword(word) {
                TokenKind::Keyword
            } else {
                TokenKind::Identifier
            };
            out.push(Token {
                kind,
                range: start..i,
            });
            continue;
        }

        if b.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            out.push(Token {
                kind: TokenKind::Number,
                range: start..i,
            });
            continue;
        }

        if b == b'\'' || b == b'"' {
            let quote = b;
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                // Naive escape handling: skip the char after a backslash.
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < bytes.len() {
                i += 1; // consume closing quote
            }
            out.push(Token {
                kind: TokenKind::StringLit,
                range: start..i,
            });
            continue;
        }

        // Fallback: any other single byte is treated as punctuation.
        i += 1;
        out.push(Token {
            kind: TokenKind::Punct,
            range: start..i,
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Vec<(TokenKind, &str)> {
        tokenize(input)
            .into_iter()
            .map(|t| (t.kind, &input[t.range]))
            .collect()
    }

    #[test]
    fn tokenize_simple_select() {
        let out = kinds("SELECT id FROM users");
        assert_eq!(
            out,
            vec![
                (TokenKind::Keyword, "SELECT"),
                (TokenKind::Whitespace, " "),
                (TokenKind::Identifier, "id"),
                (TokenKind::Whitespace, " "),
                (TokenKind::Keyword, "FROM"),
                (TokenKind::Whitespace, " "),
                (TokenKind::Identifier, "users"),
            ]
        );
    }

    #[test]
    fn tokenize_case_insensitive_keywords() {
        let out = kinds("select");
        assert_eq!(out, vec![(TokenKind::Keyword, "select")]);
    }

    #[test]
    fn tokenize_string_and_number() {
        let out = kinds("WHERE name = 'Alice' AND age > 30");
        assert!(out.contains(&(TokenKind::StringLit, "'Alice'")));
        assert!(out.contains(&(TokenKind::Number, "30")));
    }

    #[test]
    fn tokenize_covers_full_input() {
        // The round-trip of range slices must reproduce the input.
        let input = "SELECT * FROM users WHERE id = 42;";
        let rebuilt: String = tokenize(input)
            .into_iter()
            .map(|t| input[t.range].to_string())
            .collect();
        assert_eq!(rebuilt, input);
    }

    #[test]
    fn tokenize_handles_unterminated_string_gracefully() {
        let out = kinds("SELECT 'foo");
        // Unterminated string should not panic and should be tagged as StringLit.
        assert_eq!(out.last(), Some(&(TokenKind::StringLit, "'foo")));
    }
}
