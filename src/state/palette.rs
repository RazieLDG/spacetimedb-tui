//! Command palette state.
//!
//! The palette is a Ctrl+P-triggered fuzzy-search overlay that lets
//! the user invoke any registered command without remembering the
//! key binding. It's deliberately separate from `Modal` because:
//!
//! - It needs its own filtered-list cursor that survives keystrokes
//!   (the modal overlay always re-derives state from the form).
//! - The command set is static, not parameterised on a per-call
//!   basis.

use crate::ui::components::input::InputState;

/// A single registered command.
///
/// Commands are referred to from the dispatcher in `app.rs` by their
/// [`Command::id`], so adding a new entry is a two-step process:
///   1. add the variant to [`Command`] and the description here, and
///   2. add a match arm in `App::dispatch_command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
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

impl Command {
    /// All commands in the order they should appear in the palette
    /// when no filter is active.
    pub const ALL: &'static [Command] = &[
        Command::GotoTables,
        Command::GotoSql,
        Command::GotoLogs,
        Command::GotoMetrics,
        Command::GotoModule,
        Command::GotoLive,
        Command::RefreshCurrentView,
        Command::ReconnectWebSocket,
        Command::ToggleHelp,
        Command::ExportCsv,
        Command::ExportJson,
        Command::CopyCell,
        Command::CopyRow,
        Command::Quit,
    ];

    /// Human-readable label shown in the palette list.
    pub fn label(self) -> &'static str {
        match self {
            Command::GotoTables => "Go to tab: Tables",
            Command::GotoSql => "Go to tab: SQL",
            Command::GotoLogs => "Go to tab: Logs",
            Command::GotoMetrics => "Go to tab: Metrics",
            Command::GotoModule => "Go to tab: Module",
            Command::GotoLive => "Go to tab: Live",
            Command::RefreshCurrentView => "Refresh current view",
            Command::ReconnectWebSocket => "Reconnect WebSocket subscription",
            Command::ToggleHelp => "Toggle help overlay",
            Command::ExportCsv => "Export current results as CSV",
            Command::ExportJson => "Export current results as JSON",
            Command::CopyCell => "Copy selected cell to clipboard",
            Command::CopyRow => "Copy selected row to clipboard",
            Command::Quit => "Quit the application",
        }
    }

    /// Optional shortcut hint shown right-aligned in the palette row,
    /// e.g. "Ctrl+R" or "y". Returns an empty string when there is no
    /// matching key binding.
    pub fn shortcut(self) -> &'static str {
        match self {
            Command::GotoTables => "1",
            Command::GotoSql => "2",
            Command::GotoLogs => "3",
            Command::GotoMetrics => "4",
            Command::GotoModule => "5",
            Command::GotoLive => "6",
            Command::RefreshCurrentView => "r",
            Command::ReconnectWebSocket => "Ctrl+R",
            Command::ToggleHelp => "?",
            Command::ExportCsv => "e",
            Command::ExportJson => "E",
            Command::CopyCell => "y",
            Command::CopyRow => "Y",
            Command::Quit => "q",
        }
    }
}

/// Live state of the command palette overlay.
#[derive(Debug, Clone, Default)]
pub struct CommandPalette {
    /// Single-line text input the user is typing into.
    pub query: InputState,
    /// Index into the *filtered* result list.
    pub selected: usize,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the filtered, scored command list for the current
    /// query. Empty query → return every command in declaration
    /// order. Otherwise rank by a tiny case-insensitive subsequence
    /// score (better matches sort first).
    pub fn filter(&self) -> Vec<Command> {
        let q = self.query.value.to_ascii_lowercase();
        if q.is_empty() {
            return Command::ALL.to_vec();
        }
        let mut scored: Vec<(usize, Command)> = Command::ALL
            .iter()
            .filter_map(|c| fuzzy_score(&c.label().to_ascii_lowercase(), &q).map(|s| (s, *c)))
            .collect();
        // Lower score = better match.
        scored.sort_by_key(|(s, _)| *s);
        scored.into_iter().map(|(_, c)| c).collect()
    }

    /// Move the cursor down within the current filtered list, with
    /// clamp at the end (no wrap-around — wrap is surprising in
    /// fuzzy lists).
    pub fn next(&mut self, list_len: usize) {
        if list_len == 0 {
            return;
        }
        self.selected = (self.selected + 1).min(list_len - 1);
    }

    pub fn prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Currently-highlighted command in the filtered list, or `None`
    /// when nothing matches.
    pub fn current(&self) -> Option<Command> {
        let list = self.filter();
        list.get(self.selected).copied()
    }
}

/// Tiny fuzzy-match scorer.
///
/// Walks `needle` (the query) through `haystack` (the label),
/// counting how many characters had to be skipped between matches.
/// Returns `None` if any needle char isn't present in order; lower
/// scores = better matches.
fn fuzzy_score(haystack: &str, needle: &str) -> Option<usize> {
    let mut h_iter = haystack.chars().peekable();
    let mut score = 0usize;
    let mut consecutive_miss = 0usize;
    for nc in needle.chars() {
        loop {
            match h_iter.next() {
                Some(hc) if hc == nc => {
                    score += consecutive_miss;
                    consecutive_miss = 0;
                    break;
                }
                Some(_) => {
                    consecutive_miss += 1;
                }
                None => return None,
            }
        }
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_score_subsequence_match() {
        // "sql" hits S-Q-L in "go to tab: sql" with no skips between
        // matches, so the score should be the chars skipped *before*
        // the first match.
        let s = fuzzy_score("go to tab: sql", "sql").unwrap();
        // 11 chars before 's', then sql contiguous → 11
        assert_eq!(s, 11);
    }

    #[test]
    fn fuzzy_score_no_match_returns_none() {
        assert!(fuzzy_score("hello world", "xyz").is_none());
    }

    #[test]
    fn fuzzy_score_exact_match_is_zero() {
        assert_eq!(fuzzy_score("hello", "hello"), Some(0));
    }

    #[test]
    fn filter_empty_query_returns_all_commands() {
        let p = CommandPalette::new();
        assert_eq!(p.filter().len(), Command::ALL.len());
    }

    #[test]
    fn filter_finds_commands_by_subsequence() {
        let mut p = CommandPalette::new();
        p.query.set("sql");
        let results = p.filter();
        assert!(results.contains(&Command::GotoSql));
    }

    #[test]
    fn filter_returns_empty_when_no_match() {
        let mut p = CommandPalette::new();
        p.query.set("zzz");
        assert!(p.filter().is_empty());
    }

    #[test]
    fn next_clamps_at_end() {
        let mut p = CommandPalette::new();
        p.next(3);
        p.next(3);
        p.next(3);
        p.next(3);
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn prev_clamps_at_zero() {
        let mut p = CommandPalette::new();
        p.prev();
        p.prev();
        assert_eq!(p.selected, 0);
    }
}
