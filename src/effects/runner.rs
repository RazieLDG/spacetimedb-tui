// Foundation: wired into production event loop in a future milestone.
#![allow(dead_code)]

//! Effect runner: bounded AppEvent channel, production transport adapters,
//! and the owned effect dispatch boundary.
//!
//! All safety-relevant async read work is spawned through `TaskRegistry`.
//! No read future is returned to callers or detached.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::api::SpacetimeClient;
use crate::app::event::{AppEvent, Effect, ReadFailure, ReadOperation, ReadResult};
use crate::effects::ports::{ApiTransport, SessionStore, SubscriptionTransport};
use crate::effects::request::RequestContext;
use crate::effects::task_registry::TaskRegistry;
use crate::effects::write_ops::encode_identifier;
use crate::terminal::{CrosstermTerminalOps, TerminalOps};

// ---------------------------------------------------------------------------
// Bounded event channel
// ---------------------------------------------------------------------------

pub struct BoundedEventChannel {
    sender: mpsc::Sender<AppEvent>,
    receiver: mpsc::Receiver<AppEvent>,
    capacity: usize,
}

impl BoundedEventChannel {
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn sender(&self) -> mpsc::Sender<AppEvent> {
        self.sender.clone()
    }

    pub fn split(self) -> (mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>) {
        (self.sender, self.receiver)
    }

    #[allow(clippy::result_large_err)]
    pub fn try_send(&self, event: AppEvent) -> Result<(), mpsc::error::TrySendError<AppEvent>> {
        self.sender.try_send(event)
    }

    pub async fn recv(&mut self) -> Option<AppEvent> {
        self.receiver.recv().await
    }
}

/// Create a bounded AppEvent channel. The bounded Tokio MPSC channel is used
/// explicitly so backpressure and overflow are testable. No unbounded AppEvent
/// channel is introduced.
pub fn bounded_event_channel(capacity: usize) -> BoundedEventChannel {
    let (sender, receiver) = mpsc::channel(capacity);
    BoundedEventChannel {
        sender,
        receiver,
        capacity,
    }
}

// ---------------------------------------------------------------------------
// Production transport adapters
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SpacetimeApiTransport {
    client: SpacetimeClient,
}

impl SpacetimeApiTransport {
    pub fn new(client: SpacetimeClient) -> Self {
        Self { client }
    }
}

impl ApiTransport for SpacetimeApiTransport {
    fn execute_read(
        &self,
        operation: ReadOperation,
        context: RequestContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + 'static>> {
        let client = self.client.clone();
        let retry = operation.clone();
        Box::pin(async move {
            let result = match operation.clone() {
                ReadOperation::Catalog => client.list_databases().await.map(ReadResult::Catalog),
                ReadOperation::Schema { database } => {
                    client.get_schema(&database).await.map(ReadResult::Schema)
                }
                ReadOperation::TableRows { target } => match encode_identifier(&target.table) {
                    Ok(table) => client
                        .query_sql(
                            &target.database,
                            &format!("SELECT * FROM {table} LIMIT 200"),
                        )
                        .await
                        .map(ReadResult::TableRows),
                    Err(error) => {
                        return AppEvent::ScopedReadFailed {
                            context,
                            failure: ReadFailure::Transport(error.to_string()),
                            retry: None,
                        };
                    }
                },
            };
            match result {
                Ok(result) => AppEvent::ScopedReadCompleted { context, result },
                Err(error) => AppEvent::ScopedReadFailed {
                    context,
                    failure: ReadFailure::Transport(error.to_string()),
                    retry: Some(retry),
                },
            }
        })
    }
}

/// Subscription transport: Live is disabled until scoped subscriptions are available.
pub struct DisabledSubscriptionTransport;

impl SubscriptionTransport for DisabledSubscriptionTransport {
    type Request = std::convert::Infallible;

    fn connect(
        &self,
        request: Self::Request,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + '_>> {
        match request {}
    }
}

/// Local session persistence store.
pub struct LocalSessionStore;

impl SessionStore for LocalSessionStore {
    fn save(
        &self,
        snapshot: crate::user_config::SessionState,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AppEvent> + Send + 'static>> {
        Box::pin(async move {
            snapshot.save();
            AppEvent::Tick
        })
    }
}

// ---------------------------------------------------------------------------
// EffectRunner
// ---------------------------------------------------------------------------

pub type AppEffectRunner =
    EffectRunner<SpacetimeApiTransport, DisabledSubscriptionTransport, CrosstermTerminalOps>;

pub struct EffectRunner<A, S, T> {
    pub api: A,
    pub subscriptions: S,
    pub terminal: T,
    pub tasks: TaskRegistry,
    pub sessions: Arc<dyn SessionStore>,
    event_sender: mpsc::Sender<AppEvent>,
}

impl<A, S, T> EffectRunner<A, S, T>
where
    A: ApiTransport,
    S: SubscriptionTransport,
    T: TerminalOps,
{
    pub fn new(
        api: A,
        subscriptions: S,
        terminal: T,
        tasks: TaskRegistry,
        sessions: Arc<dyn SessionStore>,
        event_sender: mpsc::Sender<AppEvent>,
    ) -> Self {
        Self {
            api,
            subscriptions,
            terminal,
            tasks,
            sessions,
            event_sender,
        }
    }

    /// Dispatch an effect by spawning it through the task registry.
    /// Returns the task id so the caller can track completion.
    pub fn dispatch(&mut self, effect: Effect) -> Option<crate::effects::task_registry::TaskId> {
        let (name, future) = match effect {
            Effect::LoadCatalog { context } => (
                "read catalog",
                self.api.execute_read(ReadOperation::Catalog, context),
            ),
            Effect::LoadSchema { database, context } => (
                "read schema",
                self.api
                    .execute_read(ReadOperation::Schema { database }, context),
            ),
            Effect::LoadTableRows { target, context } => (
                "read table rows",
                self.api
                    .execute_read(ReadOperation::TableRows { target }, context),
            ),
            Effect::PersistSession { snapshot } => {
                ("persist session", self.sessions.save(snapshot))
            }
        };
        let sender = self.event_sender.clone();
        Some(self.tasks.spawn(name, async move {
            let event = future.await;
            let _ = sender.send(event).await;
        }))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::event::{AppEvent, Effect, ReadOperation, TableTarget};
    use crate::effects::request::{RequestContext, RequestId, RequestScope};
    use crate::effects::task_registry::TaskRegistry;
    use crate::terminal::TerminalOps;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    // -- Test fixtures --

    #[derive(Default)]
    pub struct RecordingApiTransport {
        pub requests: Mutex<Vec<(ReadOperation, RequestContext)>>,
    }

    impl ApiTransport for RecordingApiTransport {
        fn execute_read(
            &self,
            operation: ReadOperation,
            context: RequestContext,
        ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>> {
            self.requests.lock().unwrap().push((operation, context));
            Box::pin(async { AppEvent::Tick })
        }
    }

    pub struct NoopSubscriptionTransport;

    impl SubscriptionTransport for NoopSubscriptionTransport {
        type Request = ();

        fn connect(
            &self,
            _request: Self::Request,
        ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + '_>> {
            Box::pin(async { AppEvent::Tick })
        }
    }

    #[derive(Default)]
    pub struct RecordingSessionStore {
        pub snapshots: Mutex<Vec<crate::user_config::SessionState>>,
    }

    impl SessionStore for RecordingSessionStore {
        fn save(
            &self,
            snapshot: crate::user_config::SessionState,
        ) -> Pin<Box<dyn Future<Output = AppEvent> + Send + 'static>> {
            self.snapshots.lock().unwrap().push(snapshot);
            Box::pin(async { AppEvent::Tick })
        }
    }

    pub struct NoopTerminalOps;

    impl TerminalOps for NoopTerminalOps {
        type Error = std::convert::Infallible;

        fn enable_raw_mode(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn enter_alternate_screen(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn enable_mouse_capture(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn hide_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn show_cursor(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn disable_mouse_capture(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn leave_alternate_screen(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
        fn disable_raw_mode(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    // -- Tests --

    #[test]
    fn bounded_event_channel_reports_capacity_and_refuses_overflow() {
        let channel = bounded_event_channel(2);

        assert_eq!(channel.capacity(), 2);
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_err());
    }

    #[tokio::test]
    async fn runner_dispatches_load_active_resource_through_typed_api_transport_port() {
        let api = RecordingApiTransport::default();
        let mut channel = bounded_event_channel(1);
        let mut runner = EffectRunner::new(
            api,
            NoopSubscriptionTransport,
            NoopTerminalOps,
            TaskRegistry::new(),
            Arc::new(RecordingSessionStore::default()),
            channel.sender(),
        );
        let target = TableTarget {
            database: "inventory".to_string(),
            table: "items".to_string(),
        };
        let context = RequestContext::new(
            RequestId::from_u64(1),
            RequestScope::TableRows {
                database: "inventory".into(),
                table: "items".into(),
                view: "browse".into(),
            },
            7,
        );

        let task_id = runner
            .dispatch(Effect::LoadTableRows {
                target: target.clone(),
                context: context.clone(),
            })
            .unwrap();
        let event = channel.recv().await.unwrap();
        let report = runner.tasks.join_next().await.unwrap();
        let requests = runner.api.requests.lock().unwrap();

        assert_eq!(report.id, task_id);
        assert_eq!(
            report.outcome,
            crate::effects::task_registry::TaskOutcome::Completed
        );
        assert!(matches!(event, AppEvent::Tick));
        assert_eq!(
            *requests,
            vec![(ReadOperation::TableRows { target }, context)]
        );
        assert_eq!(
            requests[0].1.scope(),
            &RequestScope::TableRows {
                database: "inventory".into(),
                table: "items".into(),
                view: "browse".into(),
            }
        );
        assert_eq!(requests[0].1.generation, 7);
    }

    #[tokio::test]
    async fn runner_maps_catalog_schema_and_persist_session_without_string_dispatch() {
        let api = RecordingApiTransport::default();
        let mut channel = bounded_event_channel(4);
        let sessions = Arc::new(RecordingSessionStore::default());
        let mut runner = EffectRunner::new(
            api,
            NoopSubscriptionTransport,
            NoopTerminalOps,
            TaskRegistry::new(),
            sessions.clone(),
            channel.sender(),
        );
        let catalog_context =
            RequestContext::new(RequestId::from_u64(2), RequestScope::DatabaseCatalog, 1);
        let schema_context = RequestContext::new(
            RequestId::from_u64(3),
            RequestScope::Schema {
                database: "inventory".into(),
            },
            4,
        );

        let catalog_task = runner
            .dispatch(Effect::LoadCatalog {
                context: catalog_context.clone(),
            })
            .unwrap();
        let schema_task = runner
            .dispatch(Effect::LoadSchema {
                database: "inventory".into(),
                context: schema_context.clone(),
            })
            .unwrap();
        let snapshot = crate::user_config::SessionState {
            last_database: Some("inventory".into()),
            last_table: Some("items".into()),
            last_tab: Some(0),
        };
        let persist_task = runner
            .dispatch(Effect::PersistSession {
                snapshot: snapshot.clone(),
            })
            .unwrap();
        let first_event = channel.recv().await.unwrap();
        let second_event = channel.recv().await.unwrap();
        let third_event = channel.recv().await.unwrap();
        let first_report = runner.tasks.join_next().await.unwrap();
        let second_report = runner.tasks.join_next().await.unwrap();
        let third_report = runner.tasks.join_next().await.unwrap();
        for event in [first_event, second_event, third_event] {
            assert!(matches!(event, AppEvent::Tick));
        }
        assert_eq!(
            [
                first_report.outcome,
                second_report.outcome,
                third_report.outcome
            ],
            [
                crate::effects::task_registry::TaskOutcome::Completed,
                crate::effects::task_registry::TaskOutcome::Completed,
                crate::effects::task_registry::TaskOutcome::Completed,
            ]
        );
        assert_ne!(catalog_task, schema_task);
        assert_ne!(persist_task, catalog_task);
        assert_eq!(sessions.snapshots.lock().unwrap().as_slice(), &[snapshot]);

        let requests = runner.api.requests.lock().unwrap();
        assert_eq!(
            *requests,
            vec![
                (ReadOperation::Catalog, catalog_context),
                (
                    ReadOperation::Schema {
                        database: "inventory".into()
                    },
                    schema_context
                ),
            ]
        );
    }
}
