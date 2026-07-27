// Phase 2/3 foundation: wired into production event loop in Phase 3.
#![allow(dead_code)]

//! Shared command availability and safety policy helpers.
//!
//! The reducer and all input adapters evaluate command availability through
//! this module, ensuring a single policy path with no competing hard-coded
//! availability checks.

use crate::app::command::{Availability, CommandContext, CommandId, CommandRegistry, FocusContext};
use crate::state::app_state::AppState;

/// Phase 2 does not change the visible layout.
pub const PHASE2_LAYOUT_CHANGE_ALLOWED: bool = false;

pub fn focus_context_from_pane(pane: crate::state::workbench::WorkbenchPane) -> FocusContext {
    match pane {
        crate::state::workbench::WorkbenchPane::Explorer => FocusContext::Explorer,
        crate::state::workbench::WorkbenchPane::Workspace => FocusContext::Workspace,
        crate::state::workbench::WorkbenchPane::Inspector => FocusContext::Inspector,
        crate::state::workbench::WorkbenchPane::Activity => FocusContext::Activity,
    }
}

pub fn command_context_from_state(state: &AppState) -> CommandContext {
    let schema_current = matches!(
        state.resources.schema,
        crate::state::resources::LoadState::Ready { .. }
            | crate::state::resources::LoadState::Refreshing { .. }
    );
    CommandContext {
        mode: state.workbench.mode,
        focus: focus_context_from_pane(state.workbench.focused_pane),
        has_active_database: state.navigation.active_database.is_some(),
        has_active_resource: state.navigation.active_resource.is_some(),
        schema_current,
        connection_online: matches!(
            state.activity.connection,
            crate::state::activity::ConnectionState::Connected
        ),
        live_available: false,
        write_plan_available: !state.safety.pending_write_plans.is_empty(),
    }
}

pub fn command_availability(id: CommandId, context: &CommandContext) -> Availability {
    CommandRegistry.availability(id, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::command::DisabledReason;

    #[test]
    fn command_context_maps_from_single_phase2_state_owners() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.workbench.focused_pane = crate::state::workbench::WorkbenchPane::Workspace;

        let context = command_context_from_state(&state);

        assert!(context.has_active_database);
        assert_eq!(context.mode, state.workbench.mode);
        assert_eq!(context.focus, FocusContext::Workspace);
        assert!(!context.live_available);
    }

    #[test]
    fn toggle_live_remains_disabled_until_phase4_capability_probe() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());
        let context = command_context_from_state(&state);

        assert_eq!(
            command_availability(CommandId::ToggleLive, &context),
            Availability::Disabled(DisabledReason::LiveUnavailable)
        );
    }

    #[test]
    fn policy_and_registry_agree_on_refresh_availability() {
        let state = AppState::new("http://localhost:3000".to_string());
        let context = command_context_from_state(&state);

        assert_eq!(
            command_availability(CommandId::RefreshActiveResource, &context),
            Availability::Disabled(DisabledReason::NoActiveResource)
        );
    }
}
