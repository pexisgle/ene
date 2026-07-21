//! In-process store for deferred (background) tool tasks (#196).
//!
//! Background-capable tools (timers, notifications, long-running jobs)
//! can use [`DeferredTaskStore`] to track tasks spawned with
//! `tokio::spawn` and report their status through the IPC
//! `PollDeferred` / `CancelDeferred` messages. The store is intentionally
//! process-local: a tool binary owns its own tasks, and the host/runtime
//! polls for completion via [`ene_tool_proto::ToolProvider::poll_deferred`].

use ene_tool_proto::DeferredStatus;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Shared, thread-safe registry of deferred background tasks (#196).
///
/// A tool spawns work with [`DeferredTaskStore::spawn`], which assigns a
/// unique `task_id`, runs the future on the Tokio runtime, and records the
/// outcome. The tool's [`ToolProvider::poll_deferred`] implementation then
/// forwards to [`DeferredTaskStore::poll`], and [`ToolProvider::cancel_deferred`]
/// forwards to [`DeferredTaskStore::cancel`].
///
/// [`ToolProvider::poll_deferred`]: ene_tool_proto::ToolProvider::poll_deferred
/// [`ToolProvider::cancel_deferred`]: ene_tool_proto::ToolProvider::cancel_deferred
#[derive(Clone, Default)]
pub struct DeferredTaskStore {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    tasks: Arc<Mutex<HashMap<String, TaskState>>>,
    next_id: AtomicU64,
}

enum TaskState {
    /// Still running; holds the abort handle so the task can be cancelled.
    Running(tokio::task::AbortHandle),
    /// Finished with a result string.
    Completed(String),
    /// Finished with an error string.
    Failed(String),
    /// Cancelled before completion.
    Cancelled,
}

impl DeferredTaskStore {
    /// Creates an empty task store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns `fut` as a background task and returns its unique `task_id`.
    ///
    /// The future must produce the tool's output string. On panic the task
    /// is recorded as failed so a polling host observes a terminal state
    /// rather than waiting forever.
    pub fn spawn<F>(&self, fut: F) -> String
    where
        F: std::future::Future<Output = String> + Send + 'static,
    {
        let id = self
            .inner
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .to_string();
        let tasks = Arc::clone(&self.inner.tasks);
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let outcome = tokio::spawn(fut).await;
            let mut guard = tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A task that was cancelled while running is left as `Cancelled`.
            if matches!(guard.get(&task_id), Some(TaskState::Cancelled)) {
                return;
            }
            let state = match outcome {
                Ok(result) => TaskState::Completed(result),
                Err(e) => TaskState::Failed(format!("background task panicked: {e}")),
            };
            guard.insert(task_id, state);
        });

        self.inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id.clone(), TaskState::Running(handle.abort_handle()));
        id
    }

    /// Returns the current [`DeferredStatus`] for `task_id`.
    ///
    /// Terminal states (`Completed`, `Failed`, `Cancelled`) are returned
    /// without removing the entry, so a host may poll the same id more than
    /// once. Use [`DeferredTaskStore::take`] to consume a finished task.
    pub fn poll(&self, task_id: &str) -> DeferredStatus {
        let guard = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(task_id) {
            None => DeferredStatus::Unknown,
            Some(TaskState::Running(_)) => DeferredStatus::Pending,
            Some(TaskState::Completed(result)) => DeferredStatus::Completed {
                result: result.clone(),
            },
            Some(TaskState::Failed(error)) => DeferredStatus::Failed {
                error: error.clone(),
            },
            Some(TaskState::Cancelled) => DeferredStatus::Cancelled,
        }
    }

    /// Removes and returns the terminal status of `task_id`, if finished.
    ///
    /// Returns `None` when the task is still running or unknown, leaving the
    /// entry in place. Use this from a completion callback to reclaim memory
    /// once the host has observed the result.
    pub fn take(&self, task_id: &str) -> Option<DeferredStatus> {
        let mut guard = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let finished = matches!(
            guard.get(task_id),
            Some(TaskState::Completed(_) | TaskState::Failed(_) | TaskState::Cancelled)
        );
        if !finished {
            return None;
        }
        let state = guard.remove(task_id)?;
        Some(match state {
            TaskState::Completed(result) => DeferredStatus::Completed { result },
            TaskState::Failed(error) => DeferredStatus::Failed { error },
            TaskState::Cancelled => DeferredStatus::Cancelled,
            // The `finished` check above guarantees a terminal state.
            TaskState::Running(_) => return None,
        })
    }

    /// Cancels the task with `task_id`, if it is still running.
    ///
    /// Returns `true` when a running task was aborted. Unknown or already
    /// finished tasks return `false`.
    pub fn cancel(&self, task_id: &str) -> bool {
        let mut guard = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(TaskState::Running(handle)) = guard.get(task_id) {
            handle.abort();
            guard.insert(task_id.to_string(), TaskState::Cancelled);
            return true;
        }
        false
    }

    /// Number of tasks currently tracked (running or finished).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Returns `true` when no tasks are tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_completes_and_polls() {
        let store = DeferredTaskStore::new();
        let id = store.spawn(async { "hello".to_string() });
        // Poll until the background task records its result.
        let mut status = store.poll(&id);
        for _ in 0..100 {
            if !matches!(status, DeferredStatus::Pending) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            status = store.poll(&id);
        }
        assert_eq!(
            status,
            DeferredStatus::Completed {
                result: "hello".to_string()
            }
        );
        assert_eq!(
            store.take(&id),
            Some(DeferredStatus::Completed {
                result: "hello".to_string()
            })
        );
        assert_eq!(store.poll(&id), DeferredStatus::Unknown);
    }

    #[tokio::test]
    async fn cancel_running_task() {
        let store = DeferredTaskStore::new();
        let id = store.spawn(async {
            tokio::time::sleep(std::time::Duration::from_mins(1)).await;
            "never".to_string()
        });
        assert!(store.cancel(&id));
        assert_eq!(store.poll(&id), DeferredStatus::Cancelled);
        // Cancelling again reports false (already terminal).
        assert!(!store.cancel(&id));
    }

    #[test]
    fn poll_unknown_task() {
        let store = DeferredTaskStore::new();
        assert_eq!(store.poll("missing"), DeferredStatus::Unknown);
        assert!(store.is_empty());
    }
}
