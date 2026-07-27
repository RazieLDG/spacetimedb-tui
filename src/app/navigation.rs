//! Semantic navigation helpers that update `NavigationState` and emit
//! required load effects.
//!
//! All navigation goes through these functions so that effects are scoped,
//! request contexts are fresh, and session persistence is emitted atomically.

use crate::app::event::{Effect, TableTarget, Transition};
use crate::effects::request::RequestScope;
use crate::state::app_state::AppState;

fn session_snapshot(state: &AppState) -> crate::user_config::SessionState {
    crate::user_config::SessionState {
        last_database: state.navigation.active_database.clone(),
        last_table: state.navigation.active_resource.clone(),
        last_tab: Some(match state.workbench.workspace {
            crate::state::workbench::Workspace::Tables => 0,
            crate::state::workbench::Workspace::Sql => 1,
            crate::state::workbench::Workspace::Logs => 2,
            crate::state::workbench::Workspace::Metrics => 3,
            crate::state::workbench::Workspace::Module => 4,
            crate::state::workbench::Workspace::Live => 5,
        }),
    }
}

pub fn select_database(state: &mut AppState, database: String) -> Transition {
    state.navigation.active_database = Some(database.clone());
    state.navigation.active_resource = None;
    Transition::effects(vec![
        Effect::LoadSchema {
            database: database.clone(),
            context: state
                .requests
                .next_context(RequestScope::Schema { database }),
        },
        Effect::PersistSession {
            snapshot: session_snapshot(state),
        },
    ])
}

pub fn select_resource(state: &mut AppState, database: String, resource: String) -> Transition {
    state.navigation.active_database = Some(database.clone());
    state.navigation.active_resource = Some(resource.clone());
    Transition::effects(vec![
        Effect::LoadTableRows {
            target: TableTarget {
                database: database.clone(),
                table: resource.clone(),
            },
            context: state.requests.next_context(RequestScope::TableRows {
                database,
                table: resource,
                view: "browse".into(),
            }),
        },
        Effect::PersistSession {
            snapshot: session_snapshot(state),
        },
    ])
}

pub fn navigate_up(state: &mut AppState) -> Transition {
    if state.navigation.active_resource.take().is_some() {
        if let Some(database) = state.navigation.active_database.clone() {
            return Transition::effects(vec![
                Effect::LoadSchema {
                    database: database.clone(),
                    context: state
                        .requests
                        .next_context(RequestScope::Schema { database }),
                },
                Effect::PersistSession {
                    snapshot: session_snapshot(state),
                },
            ]);
        }
    }
    state.navigation.active_database = None;
    Transition::effects(vec![
        Effect::LoadCatalog {
            context: state
                .requests
                .next_context(RequestScope::DatabaseCatalog),
        },
        Effect::PersistSession {
            snapshot: session_snapshot(state),
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::Effect;
    use crate::effects::request::RequestScope;
    use crate::state::app_state::AppState;

    #[test]
    fn select_resource_updates_navigation_and_emits_scoped_load_once() {
        let mut state = AppState::new("http://localhost:3000".to_string());

        let transition =
            select_resource(&mut state, "inventory".to_string(), "items".to_string());

        assert_eq!(
            state.navigation.active_database.as_deref(),
            Some("inventory")
        );
        assert_eq!(state.navigation.active_resource.as_deref(), Some("items"));
        assert!(matches!(
            transition.effects.as_slice(),
            [Effect::LoadTableRows { target, context }, Effect::PersistSession { snapshot }]
                if target.database == "inventory"
                && target.table == "items"
                && context.scope() == &RequestScope::TableRows {
                    database: "inventory".into(),
                    table: "items".into(),
                    view: "browse".into(),
                }
                && snapshot.last_database.as_deref() == Some("inventory")
                && snapshot.last_table.as_deref() == Some("items")
                && snapshot.last_tab == Some(0)
        ));
    }

    #[test]
    fn navigate_up_from_resource_clears_resource_and_emits_schema_reload() {
        let mut state = AppState::new("http://localhost:3000".to_string());
        state.navigation.active_database = Some("inventory".to_string());
        state.navigation.active_resource = Some("items".to_string());

        let transition = navigate_up(&mut state);

        assert_eq!(
            state.navigation.active_database.as_deref(),
            Some("inventory")
        );
        assert_eq!(state.navigation.active_resource, None);
        assert!(matches!(
            transition.effects.as_slice(),
            [Effect::LoadSchema { database, context }, Effect::PersistSession { snapshot }]
                if database == "inventory"
                && context.scope() == &RequestScope::Schema { database: "inventory".into() }
                && snapshot.last_database.as_deref() == Some("inventory")
                && snapshot.last_table.is_none()
                && snapshot.last_tab == Some(0)
        ));
    }
}
