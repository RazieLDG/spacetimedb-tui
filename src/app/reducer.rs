//! Pure reducer: `reduce(&mut AppState, AppEvent) -> Transition`.
//!
//! No network, terminal, or time side effects. All side effects are expressed
//! as `Effect` values in the returned `Transition`.

use crate::app::command::CommandId;
use crate::app::event::{Action, AppEvent, Effect, ReadResult, TableTarget, Transition};
use crate::app::policy::{command_availability, command_context_from_state};
use crate::effects::request::{RequestContext, RequestScope};
use crate::state::app_state::AppState;

pub fn reduce(state: &mut AppState, event: AppEvent) -> Transition {
    match event {
        AppEvent::Action(Action::Invoke(command)) => invoke_command(state, command),
        AppEvent::Action(Action::SelectDatabase { database }) => {
            crate::app::navigation::select_database(state, database)
        }
        AppEvent::Action(Action::SelectResource { database, resource }) => {
            crate::app::navigation::select_resource(state, database, resource)
        }
        AppEvent::Action(Action::NavigateUp) => crate::app::navigation::navigate_up(state),
        AppEvent::ScopedReadCompleted { context, result } => {
            if state.requests.is_current(&context) {
                apply_scoped_read_completion(state, context, result);
            }
            Transition::none()
        }
        AppEvent::ScopedReadFailed {
            context,
            failure,
            retry: _,
        } => {
            if state.requests.is_current(&context) {
                apply_scoped_read_failure(state, context, failure);
            }
            // Phase 2 retains the typed retry identity on the event but does
            // not schedule retries until Phase 4 installs retry policy.
            Transition::none()
        }
        _ => Transition::none(),
    }
}

fn invoke_command(state: &mut AppState, command: CommandId) -> Transition {
    let context = command_context_from_state(state);
    if command_availability(command, &context) != crate::app::command::Availability::Available {
        return Transition::none();
    }
    match command {
        CommandId::RefreshActiveResource | CommandId::RefreshCurrentView => {
            match (
                &state.navigation.active_database,
                &state.navigation.active_resource,
            ) {
                (Some(database), Some(resource)) => Transition::effects(vec![
                    Effect::LoadTableRows {
                        target: TableTarget {
                            database: database.clone(),
                            table: resource.clone(),
                        },
                        context: state.requests.next_context(RequestScope::TableRows {
                            database: database.clone(),
                            table: resource.clone(),
                            view: "browse".into(),
                        }),
                    },
                ]),
                _ => Transition::none(),
            }
        }
        _ => Transition::none(),
    }
}

fn apply_scoped_read_completion(state: &mut AppState, context: RequestContext, result: ReadResult) {
    use std::time::Instant;

    let now = Instant::now();
    let scope = context.scope.clone();
    match (scope, result) {
        (RequestScope::DatabaseCatalog, ReadResult::Catalog(databases)) => {
            state.resources.catalog = std::mem::take(&mut state.resources.catalog)
                .apply_success(&context, databases, now);
        }
        (RequestScope::Schema { .. }, ReadResult::Schema(schema)) => {
            state.resources.schema = std::mem::take(&mut state.resources.schema)
                .apply_success(&context, schema, now);
        }
        (RequestScope::TableRows { .. }, ReadResult::TableRows(rows))
        | (RequestScope::SqlWorkspace { .. }, ReadResult::TableRows(rows)) => {
            state.resources.table_rows = std::mem::take(&mut state.resources.table_rows)
                .apply_success(&context, rows, now);
        }
        (RequestScope::Logs { .. }, ReadResult::Logs(logs)) => {
            state.resources.logs =
                std::mem::take(&mut state.resources.logs).apply_success(&context, logs, now);
        }
        (RequestScope::Metrics, ReadResult::Metrics(metrics)) => {
            state.resources.metrics = std::mem::take(&mut state.resources.metrics)
                .apply_success(&context, metrics, now);
        }
        _ => {}
    }
}

fn apply_scoped_read_failure(
    state: &mut AppState,
    context: RequestContext,
    failure: crate::app::event::ReadFailure,
) {
    use crate::state::resources::AppError;

    let scope = context.scope.clone();
    let error = AppError {
        message: match failure {
            crate::app::event::ReadFailure::Transport(message) => message,
        },
    };
    match scope {
        RequestScope::DatabaseCatalog => {
            state.resources.catalog = std::mem::take(&mut state.resources.catalog)
                .apply_error(&context, error);
        }
        RequestScope::Schema { .. } => {
            state.resources.schema =
                std::mem::take(&mut state.resources.schema).apply_error(&context, error);
        }
        RequestScope::TableRows { .. } | RequestScope::SqlWorkspace { .. } => {
            state.resources.table_rows = std::mem::take(&mut state.resources.table_rows)
                .apply_error(&context, error);
        }
        RequestScope::Logs { .. } => {
            state.resources.logs =
                std::mem::take(&mut state.resources.logs).apply_error(&context, error);
        }
        RequestScope::Metrics => {
            state.resources.metrics =
                std::mem::take(&mut state.resources.metrics).apply_error(&context, error);
        }
        RequestScope::LiveClients { .. } => {
            state.resources.live_clients = std::mem::take(&mut state.resources.live_clients)
                .apply_error(&context, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::command::CommandId;
    use crate::app::event::{Action, AppEvent, Effect, Transition};
    use crate::effects::request::RequestScope;
    use crate::state::app_state::AppState;
    use crate::state::workbench::WorkbenchMode;

    #[test]
    fn reducer_evaluates_policy_before_emitting_refresh_effect() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());
        state.workbench.mode = WorkbenchMode::Data;

        let transition = reduce(
            &mut state,
            AppEvent::Action(Action::Invoke(CommandId::RefreshActiveResource)),
        );

        assert_eq!(
            state.navigation.active_database.as_deref(),
            Some("inventory")
        );
        assert_eq!(state.navigation.active_resource.as_deref(), Some("items"));
        assert!(matches!(
            transition,
            Transition { effects } if matches!(
                effects.as_slice(),
                [Effect::LoadTableRows { target, context }]
                    if target.database == "inventory"
                    && target.table == "items"
                    && context.scope() == &RequestScope::TableRows {
                        database: "inventory".into(),
                        table: "items".into(),
                        view: "browse".into(),
                    }
            )
        ));
    }

    #[test]
    fn reducer_blocks_unavailable_command_without_effects() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.workbench.mode = WorkbenchMode::Data;

        let transition = reduce(
            &mut state,
            AppEvent::Action(Action::Invoke(CommandId::RefreshActiveResource)),
        );

        assert_eq!(transition.effects, Vec::new());
    }

    #[test]
    fn command_invoke_helper_creates_the_shared_action() {
        assert_eq!(
            crate::app::command::invoke(CommandId::GotoSql),
            Action::Invoke(CommandId::GotoSql)
        );
    }
}
