use std::{collections::HashMap, collections::VecDeque, future::Future, panic::AssertUnwindSafe};

use futures_util::FutureExt;
use tokio::{sync::mpsc, task::AbortHandle};

/// Stable, monotonically increasing identifier for a spawned task.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TaskId(u64);

/// How a spawned task finished.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    /// The task's future ran to completion without panicking.
    Completed,
    /// The task was aborted before it finished.
    Cancelled,
    /// The task's future panicked; `message` is the recovered panic payload.
    Panicked { message: String },
}

/// The terminal report for a single spawned task, carrying its original
/// identity so callers never have to synthesize an "unknown" task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskReport {
    pub id: TaskId,
    pub name: String,
    pub outcome: TaskOutcome,
}

/// Owns background tasks so their completion, panic, and cancellation are
/// always joined and reported rather than silently detached.
///
/// Each spawned future reports exactly one [`TaskReport`]:
/// - normal completion and recovered panics are reported from inside the task
///   via an unbounded channel;
/// - cancellation ([`Self::abort_all`]) is reported by the registry itself,
///   which synthesizes [`TaskOutcome::Cancelled`] reports with the original
///   task identity.
///
/// This deliberately uses only the stable Tokio API (an unbounded report
/// channel plus [`AbortHandle`]) instead of the `tokio_unstable`
/// `join_next_with_id`, so it builds without `RUSTFLAGS=--cfg tokio_unstable`.
pub struct TaskRegistry {
    next_id: u64,
    report_rx: mpsc::UnboundedReceiver<TaskReport>,
    report_tx: mpsc::UnboundedSender<TaskReport>,
    /// Identity and abort handle for tasks that have not yet reported.
    pending: HashMap<TaskId, (String, AbortHandle)>,
    /// Locally queued reports (produced by [`Self::abort_all`]).
    queued: VecDeque<TaskReport>,
}

impl TaskRegistry {
    pub fn new() -> Self {
        let (report_tx, report_rx) = mpsc::unbounded_channel();
        Self {
            next_id: 1,
            report_rx,
            report_tx,
            pending: HashMap::new(),
            queued: VecDeque::new(),
        }
    }

    /// Spawn `future` on the Tokio runtime under a human-readable `name`.
    ///
    /// Returns a [`TaskId`] that will appear on the eventual [`TaskReport`].
    pub fn spawn<F, T>(&mut self, name: impl Into<String>, future: F) -> TaskId
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let id = TaskId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let name = name.into();
        let identity_name = name.clone();
        let report_tx = self.report_tx.clone();

        let handle = tokio::spawn(async move {
            let outcome = match AssertUnwindSafe(future).catch_unwind().await {
                Ok(_) => TaskOutcome::Completed,
                Err(payload) => TaskOutcome::Panicked {
                    message: panic_message(payload),
                },
            };
            let _ = report_tx.send(TaskReport { id, name, outcome });
        });

        self.pending
            .insert(id, (identity_name, handle.abort_handle()));
        id
    }

    /// Await the next terminal report.
    ///
    /// Returns `None` only when no task is pending and no report is queued.
    /// While tasks are still running this awaits the next completion, panic,
    /// or cancellation report. Each task yields exactly one report: a report
    /// whose id is no longer pending (already reported, e.g. a completion that
    /// raced an abort-synthesized cancellation) is skipped.
    pub async fn join_next(&mut self) -> Option<TaskReport> {
        if let Some(report) = self.try_join_next() {
            return Some(report);
        }
        loop {
            if self.pending.is_empty() {
                return None;
            }
            let report = self.report_rx.recv().await?;
            if self.pending.remove(&report.id).is_some() {
                return Some(report);
            }
            // Report for a task that was already reported; skip it so each
            // task yields exactly one report.
        }
    }

    /// Non-blocking variant of [`Self::join_next`]: return a queued report if
    /// one is ready, otherwise `None`. Intended for the UI event loop.
    pub fn try_join_next(&mut self) -> Option<TaskReport> {
        loop {
            if let Some(report) = self.queued.pop_front() {
                self.pending.remove(&report.id);
                return Some(report);
            }
            match self.report_rx.try_recv() {
                Ok(report) if self.pending.remove(&report.id).is_some() => {
                    return Some(report);
                }
                // Report for a task that was already reported; skip it.
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    /// Abort every task that has not yet reported. Each aborted task produces
    /// a [`TaskOutcome::Cancelled`] report with its original identity.
    ///
    /// Any reports that already arrived on the channel are drained first so
    /// that a task which completed just before the abort is not double-reported.
    pub fn abort_all(&mut self) {
        // Drain already-arrived reports so we don't synthesize a duplicate
        // Cancelled for a task that finished moments before the abort.
        while let Ok(report) = self.report_rx.try_recv() {
            self.pending.remove(&report.id);
            self.queued.push_back(report);
        }
        // Abort remaining tasks and synthesize Cancelled reports.
        for (id, (name, handle)) in self.pending.drain() {
            handle.abort();
            self.queued.push_back(TaskReport {
                id,
                name,
                outcome: TaskOutcome::Cancelled,
            });
        }
    }

    /// Number of tasks spawned but not yet reported.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "task panicked with non-string payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completed_task_is_joined_and_reported() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("read rows", async { 42usize });

        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "read rows");
        assert_eq!(report.outcome, TaskOutcome::Completed);
    }

    #[tokio::test]
    async fn panic_is_joined_and_reported_instead_of_detached() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("boom", async { panic!("task panic for test") });

        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "boom");
        assert_ne!(report.name, "unknown");
        assert!(matches!(report.outcome, TaskOutcome::Panicked { .. }));
    }

    #[tokio::test]
    async fn abort_all_drains_cancelled_tasks_with_original_identity() {
        let mut registry = TaskRegistry::new();
        let id = registry.spawn("slow read", async { std::future::pending::<()>().await });

        registry.abort_all();
        let report = registry.join_next().await.unwrap();

        assert_eq!(report.id, id);
        assert_eq!(report.name, "slow read");
        assert_ne!(report.name, "unknown");
        assert_eq!(report.outcome, TaskOutcome::Cancelled);
    }

    #[tokio::test]
    async fn every_task_yields_exactly_one_report() {
        let mut registry = TaskRegistry::new();
        for i in 0..8u64 {
            registry.spawn(format!("task {i}"), async move { i });
        }
        // Let them all finish, then abort whatever is still pending.
        registry.abort_all();

        let mut seen = std::collections::HashSet::new();
        let mut count = 0usize;
        while let Some(report) = registry.join_next().await {
            assert!(
                seen.insert(report.id),
                "duplicate report for {:?}",
                report.id
            );
            count += 1;
        }
        assert_eq!(count, 8, "each task must report exactly once");
        assert_eq!(registry.pending_count(), 0);
    }
}
