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

/// Phase 2 subscription transport: Live is disabled until Phase 4.
pub struct Phase2SubscriptionTransport;

impl SubscriptionTransport for Phase2SubscriptionTransport {
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
    EffectRunner<SpacetimeApiTransport, Phase2SubscriptionTransport, CrosstermTerminalOps>;

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
            Effect::PersistSession { snapshot } => ("persist session", self.sessions.save(snapshot)),
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
    use crate::app::event::AppEvent;

    #[test]
    fn bounded_event_channel_reports_capacity_and_refuses_overflow() {
        let channel = bounded_event_channel(2);

        assert_eq!(channel.capacity(), 2);
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_ok());
        assert!(channel.try_send(AppEvent::Tick).is_err());
    }
}
