//! Typed command registry: stable command IDs, key bindings, palette search,
//! availability policy, and help line generation.
//!
//! Keyboard, palette, help, and later mouse input all resolve through this
//! registry with no competing hard-coded availability checks.

use crate::state::workbench::WorkbenchMode;
use crossterm::event::{KeyCode, KeyModifiers};

// ---------------------------------------------------------------------------
// CommandId
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Context and availability types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusContext {
    Explorer,
    Workspace,
    Inspector,
    Activity,
    Palette,
    Help,
    Modal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisabledReason {
    NoActiveDatabase,
    NoActiveResource,
    SchemaNotCurrent,
    Offline,
    LiveUnavailable,
    WritePlanUnavailable,
    WrongMode,
    WrongFocus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    Available,
    Disabled(DisabledReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvailabilityRule {
    Always,
    RequiresActiveDatabase,
    RequiresActiveResource,
    RequiresOnline,
    RequiresLiveAvailable,
    RequiresWritePlan,
}

// ---------------------------------------------------------------------------
// Key bindings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub fn ctrl(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::CONTROL,
        }
    }

    pub fn plain(c: char) -> Self {
        Self {
            code: KeyCode::Char(c),
            modifiers: KeyModifiers::NONE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub key: KeyChord,
    pub display: &'static str,
    pub migration_alias: bool,
}

// ---------------------------------------------------------------------------
// CommandSpec
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// CommandContext
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandContext {
    pub mode: WorkbenchMode,
    pub focus: FocusContext,
    pub has_active_database: bool,
    pub has_active_resource: bool,
    pub schema_current: bool,
    pub connection_online: bool,
    pub live_available: bool,
    pub write_plan_available: bool,
}

// ---------------------------------------------------------------------------
// HelpLine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpLine {
    pub command_id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
    pub primary_binding: Option<String>,
}

// ---------------------------------------------------------------------------
// PaletteEntry and ContextualHelpLine (Task 3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    pub command_id: CommandId,
    pub label: &'static str,
    pub description: &'static str,
    pub enabled: bool,
    pub disabled_reason: Option<DisabledReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextualHelpLine {
    pub command_id: CommandId,
    pub label: &'static str,
    pub primary_binding: Option<String>,
    pub disabled_reason: Option<DisabledReason>,
}

// ---------------------------------------------------------------------------
// Static spec tables
// ---------------------------------------------------------------------------

const ALL_MODES: &[WorkbenchMode] = &[
    WorkbenchMode::Data,
    WorkbenchMode::Observe,
    WorkbenchMode::Operate,
];

const ALL_FOCUS: &[FocusContext] = &[
    FocusContext::Explorer,
    FocusContext::Workspace,
    FocusContext::Inspector,
    FocusContext::Activity,
    FocusContext::Palette,
    FocusContext::Help,
    FocusContext::Modal,
];

const WORKBENCH_FOCUS: &[FocusContext] = &[
    FocusContext::Explorer,
    FocusContext::Workspace,
    FocusContext::Inspector,
    FocusContext::Activity,
];

const MODAL_OR_INSPECTOR_FOCUS: &[FocusContext] =
    &[FocusContext::Modal, FocusContext::Inspector];

const NO_BINDINGS: &[Binding] = &[];

const OPEN_PALETTE_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Char('p'),
        modifiers: KeyModifiers::CONTROL,
    },
    display: "Ctrl+P",
    migration_alias: false,
}];

const HELP_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Char('?'),
        modifiers: KeyModifiers::NONE,
    },
    display: "?",
    migration_alias: false,
}];

const ESC_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
    },
    display: "Esc",
    migration_alias: false,
}];

const TAB_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::NONE,
    },
    display: "Tab",
    migration_alias: false,
}];

const BACKTAB_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::BackTab,
        modifiers: KeyModifiers::SHIFT,
    },
    display: "Shift+Tab",
    migration_alias: false,
}];

const REFRESH_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Char('r'),
        modifiers: KeyModifiers::CONTROL,
    },
    display: "Ctrl+R",
    migration_alias: false,
}];

const TOGGLE_LIVE_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Char('l'),
        modifiers: KeyModifiers::CONTROL,
    },
    display: "Ctrl+L",
    migration_alias: false,
}];

const CONFIRM_WRITE_PLAN_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Enter,
        modifiers: KeyModifiers::CONTROL,
    },
    display: "Ctrl+Enter",
    migration_alias: false,
}];

const QUIT_BINDINGS: &[Binding] = &[Binding {
    key: KeyChord {
        code: KeyCode::Char('q'),
        modifiers: KeyModifiers::NONE,
    },
    display: "q",
    migration_alias: true,
}];

/// The single source of truth for every migrated user command.
pub const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec { id: CommandId::OpenCommandPalette, label: "Open command palette", description: "Search and run available commands for the current context.", bindings: OPEN_PALETTE_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::OpenContextualHelp, label: "Open contextual help", description: "Show shortcuts and actions available in the current context.", bindings: HELP_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::CloseSurfaceOrNavigateUp, label: "Close surface or navigate up", description: "Close the top modal surface or move one semantic navigation level up.", bindings: ESC_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::FocusNextPane, label: "Focus next pane", description: "Move focus to the next available workbench pane.", bindings: TAB_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::FocusPreviousPane, label: "Focus previous pane", description: "Move focus to the previous available workbench pane.", bindings: BACKTAB_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::SelectDatabase, label: "Select database", description: "Select a database from the catalog.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::SelectResource, label: "Select resource", description: "Select a table, reducer, or module resource.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::RefreshActiveResource, label: "Refresh active resource", description: "Reload the current database resource.", bindings: REFRESH_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::ToggleLive, label: "Toggle live updates", description: "Toggle live updates for the scoped active resource only; do not enable all-table Live behavior.", bindings: TOGGLE_LIVE_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresLiveAvailable },
    CommandSpec { id: CommandId::ConfirmWritePlan, label: "Confirm write plan", description: "Confirm the reviewed Phase 1 WritePlan for an explicit mutation with unresolved mutation uncertainty preserved.", bindings: CONFIRM_WRITE_PLAN_BINDINGS, modes: ALL_MODES, focus: MODAL_OR_INSPECTOR_FOCUS, availability: AvailabilityRule::RequiresWritePlan },
    CommandSpec { id: CommandId::RestoreSession, label: "Restore session", description: "Restore the configured previous database session.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::GotoTables, label: "Go to tables", description: "Show table resources.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoSql, label: "Go to SQL", description: "Show the SQL workspace.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoLogs, label: "Go to logs", description: "Show log output.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::GotoMetrics, label: "Go to metrics", description: "Show metrics output.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::GotoModule, label: "Go to module", description: "Show module details.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresActiveDatabase },
    CommandSpec { id: CommandId::GotoLive, label: "Go to live", description: "Show the scoped live resource workspace.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresLiveAvailable },
    CommandSpec { id: CommandId::RefreshCurrentView, label: "Refresh current view", description: "Palette-only legacy alias for refreshing the current view through the canonical refresh path.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::RequiresOnline },
    CommandSpec { id: CommandId::ReconnectWebSocket, label: "Reconnect WebSocket", description: "Reconnect the WebSocket adapter.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::ToggleHelp, label: "Toggle help", description: "Palette-only legacy alias for contextual help; the ? key belongs to OpenContextualHelp.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
    CommandSpec { id: CommandId::ExportCsv, label: "Export CSV", description: "Export the active grid as CSV.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::ExportJson, label: "Export JSON", description: "Export the active grid as JSON.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::CopyCell, label: "Copy cell", description: "Copy the focused grid cell.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::CopyRow, label: "Copy row", description: "Copy the focused grid row.", bindings: NO_BINDINGS, modes: ALL_MODES, focus: WORKBENCH_FOCUS, availability: AvailabilityRule::RequiresActiveResource },
    CommandSpec { id: CommandId::Quit, label: "Quit", description: "Quit the application.", bindings: QUIT_BINDINGS, modes: ALL_MODES, focus: ALL_FOCUS, availability: AvailabilityRule::Always },
];

// ---------------------------------------------------------------------------
// Availability evaluation
// ---------------------------------------------------------------------------

pub fn evaluate_availability(rule: AvailabilityRule, context: &CommandContext) -> Availability {
    match rule {
        AvailabilityRule::Always => Availability::Available,
        AvailabilityRule::RequiresActiveDatabase if !context.has_active_database => {
            Availability::Disabled(DisabledReason::NoActiveDatabase)
        }
        AvailabilityRule::RequiresActiveResource if !context.has_active_resource => {
            Availability::Disabled(DisabledReason::NoActiveResource)
        }
        AvailabilityRule::RequiresOnline if !context.connection_online => {
            Availability::Disabled(DisabledReason::Offline)
        }
        AvailabilityRule::RequiresLiveAvailable if !context.live_available => {
            Availability::Disabled(DisabledReason::LiveUnavailable)
        }
        AvailabilityRule::RequiresWritePlan if !context.write_plan_available => {
            Availability::Disabled(DisabledReason::WritePlanUnavailable)
        }
        _ => Availability::Available,
    }
}

// ---------------------------------------------------------------------------
// Fuzzy subsequence scoring
// ---------------------------------------------------------------------------

fn subsequence_score(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut total_gap = 0usize;
    let mut last_match = None;
    let mut chars = needle.chars();
    let mut wanted = chars.next()?;
    for (index, actual) in haystack.chars().enumerate() {
        if actual == wanted {
            if let Some(previous) = last_match {
                total_gap += index.saturating_sub(previous + 1);
            } else {
                total_gap += index;
            }
            last_match = Some(index);
            if let Some(next) = chars.next() {
                wanted = next;
            } else {
                return Some(total_gap);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// CommandRegistry
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone, Copy)]
pub struct CommandRegistry;

impl CommandRegistry {
    pub fn by_id(&self, id: CommandId) -> Option<&'static CommandSpec> {
        COMMAND_SPECS.iter().find(|spec| spec.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static CommandSpec> {
        COMMAND_SPECS.iter()
    }

    pub fn resolve_key(&self, key: KeyChord) -> Option<CommandId> {
        COMMAND_SPECS.iter().find_map(|spec| {
            spec.bindings
                .iter()
                .any(|binding| binding.key == key)
                .then_some(spec.id)
        })
    }

    pub fn resolve_palette(&self, query: &str) -> Option<CommandId> {
        let normalized = query.trim().to_ascii_lowercase();
        COMMAND_SPECS
            .iter()
            .find_map(|spec| (spec.label.to_ascii_lowercase() == normalized).then_some(spec.id))
    }

    pub fn search_palette(&self, query: &str) -> Vec<CommandId> {
        let normalized = query.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return self.iter().map(|spec| spec.id).collect();
        }

        let mut scored = self
            .iter()
            .enumerate()
            .filter_map(|(index, spec)| {
                let haystack = format!("{} {}", spec.label, spec.description).to_ascii_lowercase();
                subsequence_score(&haystack, &normalized).map(|score| (score, index, spec.id))
            })
            .collect::<Vec<_>>();
        scored.sort_by_key(|(score, index, _)| (*score, *index));
        scored.into_iter().map(|(_, _, id)| id).collect()
    }

    pub fn palette_score(&self, id: CommandId, query: &str) -> usize {
        let Some(spec) = self.by_id(id) else {
            return usize::MAX;
        };
        let haystack = format!("{} {}", spec.label, spec.description).to_ascii_lowercase();
        subsequence_score(&haystack, &query.trim().to_ascii_lowercase()).unwrap_or(usize::MAX)
    }

    pub fn help_lines(&self) -> Vec<HelpLine> {
        COMMAND_SPECS
            .iter()
            .map(|spec| HelpLine {
                command_id: spec.id,
                label: spec.label,
                description: spec.description,
                primary_binding: spec.bindings.first().map(|binding| binding.display.to_string()),
            })
            .collect()
    }

    pub fn availability(&self, id: CommandId, context: &CommandContext) -> Availability {
        let Some(spec) = self.by_id(id) else {
            return Availability::Disabled(DisabledReason::WrongMode);
        };
        if !spec.modes.contains(&context.mode) {
            return Availability::Disabled(DisabledReason::WrongMode);
        }
        if !spec.focus.contains(&context.focus) {
            return Availability::Disabled(DisabledReason::WrongFocus);
        }
        evaluate_availability(spec.availability, context)
    }

    pub fn palette_entries(&self, context: &CommandContext) -> Vec<PaletteEntry> {
        COMMAND_SPECS
            .iter()
            .map(|spec| {
                let availability = self.availability(spec.id, context);
                PaletteEntry {
                    command_id: spec.id,
                    label: spec.label,
                    description: spec.description,
                    enabled: availability == Availability::Available,
                    disabled_reason: match availability {
                        Availability::Available => None,
                        Availability::Disabled(reason) => Some(reason),
                    },
                }
            })
            .collect()
    }

    pub fn contextual_help(&self, context: &CommandContext) -> Vec<ContextualHelpLine> {
        COMMAND_SPECS
            .iter()
            .map(|spec| {
                let availability = self.availability(spec.id, context);
                ContextualHelpLine {
                    command_id: spec.id,
                    label: spec.label,
                    primary_binding: spec
                        .bindings
                        .first()
                        .map(|binding| binding.display.to_string()),
                    disabled_reason: match availability {
                        Availability::Available => None,
                        Availability::Disabled(reason) => Some(reason),
                    },
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Shared action bridge
// ---------------------------------------------------------------------------

/// Create the shared `Action::Invoke` for a command. Keyboard, palette, help,
/// and mouse adapters all call this or produce the identical value.
pub fn invoke(id: CommandId) -> crate::app::event::Action {
    crate::app::event::Action::Invoke(id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::workbench::WorkbenchMode;

    const MIGRATED_COMMAND_INVENTORY: &[CommandId] = &[
        CommandId::OpenCommandPalette,
        CommandId::OpenContextualHelp,
        CommandId::CloseSurfaceOrNavigateUp,
        CommandId::FocusNextPane,
        CommandId::FocusPreviousPane,
        CommandId::SelectDatabase,
        CommandId::SelectResource,
        CommandId::RefreshActiveResource,
        CommandId::ToggleLive,
        CommandId::ConfirmWritePlan,
        CommandId::RestoreSession,
        CommandId::GotoTables,
        CommandId::GotoSql,
        CommandId::GotoLogs,
        CommandId::GotoMetrics,
        CommandId::GotoModule,
        CommandId::GotoLive,
        CommandId::RefreshCurrentView,
        CommandId::ReconnectWebSocket,
        CommandId::ToggleHelp,
        CommandId::ExportCsv,
        CommandId::ExportJson,
        CommandId::CopyCell,
        CommandId::CopyRow,
        CommandId::Quit,
    ];

    #[test]
    fn ctrl_p_keyboard_palette_and_help_resolve_same_command() {
        let registry = CommandRegistry::default();
        let command = registry.by_id(CommandId::OpenCommandPalette).unwrap();

        assert_eq!(command.id, CommandId::OpenCommandPalette);
        assert_eq!(
            registry.resolve_key(KeyChord::ctrl('p')),
            Some(CommandId::OpenCommandPalette)
        );
        assert_eq!(
            registry.resolve_palette("open command palette"),
            Some(CommandId::OpenCommandPalette)
        );
        assert!(registry.help_lines().iter().any(|line| {
            line.command_id == CommandId::OpenCommandPalette
                && line.primary_binding.as_deref() == Some("Ctrl+P")
        }));
    }

    #[test]
    fn disabled_commands_report_stable_reason() {
        let registry = CommandRegistry::default();
        let context = CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: false,
            has_active_resource: false,
            schema_current: false,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        };

        assert_eq!(
            registry.availability(CommandId::RefreshActiveResource, &context),
            Availability::Disabled(DisabledReason::NoActiveResource)
        );
    }

    #[test]
    fn migration_inventory_matches_registry_once() {
        let mut inventory = MIGRATED_COMMAND_INVENTORY.to_vec();
        inventory.sort_by_key(|id| *id as u16);
        inventory.dedup();
        assert_eq!(
            inventory.len(),
            MIGRATED_COMMAND_INVENTORY.len(),
            "migration inventory has duplicates"
        );

        let mut registry_ids = COMMAND_SPECS.iter().map(|spec| spec.id).collect::<Vec<_>>();
        registry_ids.sort_by_key(|id| *id as u16);
        registry_ids.dedup();

        let mut expected = MIGRATED_COMMAND_INVENTORY.to_vec();
        expected.sort_by_key(|id| *id as u16);

        assert_eq!(
            registry_ids, expected,
            "registry must contain exactly the migrated command inventory"
        );
    }

    #[test]
    fn key_bindings_have_one_canonical_owner() {
        let mut owners = std::collections::HashMap::new();
        for spec in COMMAND_SPECS {
            for binding in spec.bindings {
                assert_eq!(
                    owners.insert(binding.key, spec.id),
                    None,
                    "binding {binding:?} has multiple owners"
                );
            }
        }
        assert_eq!(
            owners.get(&KeyChord::plain('?')),
            Some(&CommandId::OpenContextualHelp)
        );
        assert_eq!(
            owners.get(&KeyChord::ctrl('r')),
            Some(&CommandId::RefreshActiveResource)
        );
    }

    #[test]
    fn palette_search_preserves_fuzzy_subsequence_contract() {
        let registry = CommandRegistry::default();
        let all = registry.iter().map(|spec| spec.id).collect::<Vec<_>>();

        assert_eq!(registry.search_palette(""), all);
        assert_eq!(
            registry.search_palette("sql").first(),
            Some(&CommandId::GotoSql)
        );
        assert!(registry.search_palette("zzz").is_empty());
        assert_eq!(registry.search_palette("go"), registry.search_palette("GO"));
    }

    #[test]
    fn palette_search_is_deterministic_by_score_then_registry_order() {
        let registry = CommandRegistry::default();
        let first = registry.search_palette("go");
        let second = registry.search_palette("go");

        assert_eq!(first, second);
        assert!(first.windows(2).all(|window| {
            registry.palette_score(window[0], "go") <= registry.palette_score(window[1], "go")
        }));
    }

    #[test]
    fn every_command_has_one_spec_and_one_typed_availability_rule() {
        let registry = CommandRegistry::default();

        for id in MIGRATED_COMMAND_INVENTORY {
            let specs = COMMAND_SPECS
                .iter()
                .filter(|spec| spec.id == *id)
                .collect::<Vec<_>>();
            assert_eq!(specs.len(), 1, "{id:?} must have exactly one CommandSpec");
            let spec = specs[0];
            // Use a focus that is valid for this command's spec.
            let focus = spec.focus[0];
            let context = CommandContext {
                mode: WorkbenchMode::Data,
                focus,
                has_active_database: true,
                has_active_resource: true,
                schema_current: true,
                connection_online: true,
                live_available: true,
                write_plan_available: true,
            };
            assert_eq!(
                registry.availability(*id, &context),
                evaluate_availability(spec.availability, &context)
            );
        }
    }

    #[test]
    fn palette_entries_are_generated_from_registered_commands() {
        let registry = CommandRegistry::default();
        let entries = registry.palette_entries(&CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: true,
            schema_current: true,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        });

        assert!(entries.iter().any(|entry| {
            entry.command_id == CommandId::RefreshActiveResource
                && entry.enabled
                && entry.label == "Refresh active resource"
        }));
    }

    #[test]
    fn help_marks_disabled_commands_with_registry_reason() {
        let registry = CommandRegistry::default();
        let lines = registry.contextual_help(&CommandContext {
            mode: WorkbenchMode::Data,
            focus: FocusContext::Workspace,
            has_active_database: true,
            has_active_resource: false,
            schema_current: true,
            connection_online: true,
            live_available: false,
            write_plan_available: false,
        });

        assert!(lines.iter().any(|line| {
            line.command_id == CommandId::RefreshActiveResource
                && line.disabled_reason == Some(DisabledReason::NoActiveResource)
        }));
    }

    #[test]
    fn phase2_command_descriptions_do_not_claim_new_layout() {
        let registry = CommandRegistry::default();
        let forbidden = [
            "Explorer",
            "Inspector drawer",
            "DATA OBSERVE OPERATE layout is live",
        ];

        for line in registry.help_lines() {
            for phrase in forbidden {
                assert!(
                    !line.description.contains(phrase),
                    "{phrase} appeared in {:?}",
                    line
                );
            }
        }
    }
}
