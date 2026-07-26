use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RequestId(u64);

impl RequestId {
    pub fn from_u64(value: u64) -> Self {
        Self(value)
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RequestScope {
    DatabaseCatalog,
    Schema {
        database: String,
    },
    TableRows {
        database: String,
        table: String,
        view: String,
    },
    SqlWorkspace {
        database: String,
        workspace: String,
    },
    Logs {
        database: String,
    },
    Metrics {
        database: String,
    },
    LiveClients {
        database: String,
    },
}

impl fmt::Display for RequestScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::DatabaseCatalog => "database catalog",
            Self::Schema { .. } => "schema",
            Self::TableRows { .. } => "table rows",
            Self::SqlWorkspace { .. } => "SQL workspace",
            Self::Logs { .. } => "logs",
            Self::Metrics { .. } => "metrics",
            Self::LiveClients { .. } => "live clients",
        };
        f.write_str(label)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestContext {
    pub id: RequestId,
    pub scope: RequestScope,
    pub generation: u64,
}

impl RequestContext {
    pub fn new(id: RequestId, scope: RequestScope, generation: u64) -> Self {
        Self {
            id,
            scope,
            generation,
        }
    }

    pub fn scope(&self) -> &RequestScope {
        &self.scope
    }

    pub fn accepts(&self, delivered: &RequestContext) -> bool {
        self.id == delivered.id
            && self.scope == delivered.scope
            && self.generation == delivered.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_context_matches_only_same_id_scope_and_generation() {
        let current = RequestContext::new(
            RequestId::from_u64(7),
            RequestScope::TableRows {
                database: "db".into(),
                table: "inventory".into(),
                view: "main".into(),
            },
            3,
        );

        assert!(current.accepts(&current));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(8),
            current.scope.clone(),
            3,
        )));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(7),
            RequestScope::TableRows {
                database: "db".into(),
                table: "orders".into(),
                view: "main".into(),
            },
            3,
        )));
        assert!(!current.accepts(&RequestContext::new(
            RequestId::from_u64(7),
            current.scope.clone(),
            4,
        )));
    }

    #[test]
    fn request_scope_distinguishes_all_phase_one_read_scopes() {
        let scopes = [
            RequestScope::DatabaseCatalog,
            RequestScope::Schema {
                database: "db".into(),
            },
            RequestScope::TableRows {
                database: "db".into(),
                table: "t".into(),
                view: "main".into(),
            },
            RequestScope::SqlWorkspace {
                database: "db".into(),
                workspace: "scratch".into(),
            },
            RequestScope::Logs {
                database: "db".into(),
            },
            RequestScope::Metrics {
                database: "db".into(),
            },
            RequestScope::LiveClients {
                database: "db".into(),
            },
        ];

        for (left_index, left) in scopes.iter().enumerate() {
            for (right_index, right) in scopes.iter().enumerate() {
                assert_eq!(left == right, left_index == right_index);
            }
        }
    }

    #[test]
    fn request_context_exposes_scope_by_accessor_without_moving_it() {
        let context = RequestContext::new(
            RequestId::from_u64(9),
            RequestScope::TableRows {
                database: "secret-db".into(),
                table: "orders".into(),
                view: "current".into(),
            },
            4,
        );

        assert!(
            matches!(context.scope(), RequestScope::TableRows { view, .. } if view == "current")
        );
        assert_eq!(context.id.get(), 9);
    }

    #[test]
    fn request_scope_display_uses_deterministic_non_sensitive_labels() {
        assert_eq!(
            RequestScope::DatabaseCatalog.to_string(),
            "database catalog"
        );
        assert_eq!(
            RequestScope::Schema {
                database: "secret-db".into()
            }
            .to_string(),
            "schema"
        );
        assert_eq!(
            RequestScope::TableRows {
                database: "secret-db".into(),
                table: "orders".into(),
                view: "current".into()
            }
            .to_string(),
            "table rows"
        );
        assert_eq!(
            RequestScope::SqlWorkspace {
                database: "secret-db".into(),
                workspace: "scratch".into()
            }
            .to_string(),
            "SQL workspace"
        );
        assert_eq!(
            RequestScope::Logs {
                database: "secret-db".into()
            }
            .to_string(),
            "logs"
        );
        assert_eq!(
            RequestScope::Metrics {
                database: "secret-db".into()
            }
            .to_string(),
            "metrics"
        );
        assert_eq!(
            RequestScope::LiveClients {
                database: "secret-db".into()
            }
            .to_string(),
            "live clients"
        );
    }
}
