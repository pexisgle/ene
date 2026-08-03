//! In-memory registry for background (deferred) tasks: countdown timers
//! and one-shot desktop notifications.
//!
//! The host polls deferred tasks over IPC (`poll_deferred`); this registry
//! owns the plugin-side state behind those polls, plus the
//! `utility.timer_stop` state machine. Timers are deliberately ephemeral
//! process state — they are not persisted.

use ene_plugin_proto::{DeferredStatus, ToolError, ToolResult};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Sends a desktop notification, returning a human-readable error on failure.
///
/// Injectable so tests can exercise the registry without a D-Bus session.
type Notifier = Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

/// Maximum number of finished-timer names retained for status lookups
/// (oldest dropped first).
const MAX_FINISHED_TIMERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Timer,
    Notify,
}

struct RunningTask {
    name: String,
    kind: TaskKind,
    ends_at: Option<Instant>,
    handle: JoinHandle<()>,
}

#[derive(Default)]
struct State {
    /// `task_id` → running task.
    running: HashMap<String, RunningTask>,
    /// Timer name → `task_id`, for `utility.timer_stop` by name.
    timer_names: HashMap<String, String>,
    /// `task_id` → (name, terminal status) awaiting one `poll_deferred`.
    /// Terminal statuses are reported once, then forgotten.
    terminal: HashMap<String, (String, DeferredStatus)>,
    /// Names of successfully finished timers, oldest first.
    finished: VecDeque<(String, Instant)>,
}

/// Outcome of stopping a timer by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerStopOutcome {
    /// The timer was running and has been cancelled.
    Stopped {
        /// Name of the cancelled timer.
        name: String,
    },
    /// The timer already fired; only its completion remains.
    AlreadyFinished {
        /// Name of the timer that fired.
        name: String,
    },
    /// No timer with this name exists.
    NotFound {
        /// Name that matched nothing.
        name: String,
    },
}

/// A running timer as reported by [`TaskRegistry::list`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RunningTimerInfo {
    /// Timer name.
    pub name: String,
    /// Whole seconds until the timer fires.
    pub remaining_seconds: u64,
}

/// Background task registry shared by the deferred utility actions.
pub struct TaskRegistry {
    notifier: Notifier,
    state: Mutex<State>,
}

impl std::fmt::Debug for TaskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskRegistry").finish_non_exhaustive()
    }
}

impl TaskRegistry {
    /// Creates a registry that shows desktop notifications via `notify-rust`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            notifier: Arc::new(|summary, body| {
                match notify_rust::Notification::new()
                    .summary(summary)
                    .body(body)
                    .show()
                {
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.to_string()),
                }
            }),
            state: Mutex::new(State::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_notifier(notifier: Notifier) -> Self {
        Self {
            notifier,
            state: Mutex::new(State::default()),
        }
    }

    /// Starts a countdown timer that fires a desktop notification after
    /// `duration`, and returns its `task_id`.
    ///
    /// Starting a timer with a name that is already running cancels the
    /// previous one first (the host is told it was cancelled), so the
    /// same name always denotes exactly one timer.
    pub fn start_timer(
        self: &Arc<Self>,
        name: &str,
        duration: Duration,
    ) -> Result<String, ToolError> {
        if name.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "timer name must not be empty".to_string(),
            });
        }
        if duration.is_zero() {
            return Err(ToolError::InvalidArguments {
                message: "timer duration must be at least 1 second".to_string(),
            });
        }
        let Some(ends_at) = Instant::now().checked_add(duration) else {
            return Err(ToolError::InvalidArguments {
                message: "timer duration is too large".to_string(),
            });
        };

        let task_id = Uuid::new_v4().to_string();
        let name_owned = name.to_string();
        let seconds = duration.as_secs();
        let registry = Arc::clone(self);
        let task_name = name_owned.clone();
        let task_id_task = task_id.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            match (registry.notifier)(
                &format!("Timer finished: {task_name}"),
                &format!("{seconds} seconds elapsed."),
            ) {
                Ok(()) => registry.complete_timer(
                    task_id_task,
                    task_name.clone(),
                    format!("Timer '{task_name}' finished after {seconds} seconds."),
                ),
                Err(e) => registry.fail(task_id_task, task_name, e),
            }
        });

        let mut state = self.state.lock();
        if let Some(old_id) = state.timer_names.remove(name)
            && let Some(old) = state.running.remove(&old_id)
        {
            old.handle.abort();
            state
                .terminal
                .insert(old_id, (name_owned.clone(), DeferredStatus::Cancelled));
        }
        state.running.insert(
            task_id.clone(),
            RunningTask {
                name: name_owned,
                kind: TaskKind::Timer,
                ends_at: Some(ends_at),
                handle,
            },
        );
        state.timer_names.insert(name.to_string(), task_id.clone());
        Ok(task_id)
    }

    /// Sends a desktop notification in the background and returns its
    /// `task_id`. The task completes as soon as the notification is shown.
    pub fn start_notify(self: &Arc<Self>, title: &str, body: &str) -> Result<String, ToolError> {
        if title.trim().is_empty() {
            return Err(ToolError::InvalidArguments {
                message: "notification title must not be empty".to_string(),
            });
        }
        let task_id = Uuid::new_v4().to_string();
        let title_owned = title.to_string();
        let body_owned = body.to_string();
        let registry = Arc::clone(self);
        let task_title = title_owned.clone();
        let task_id_task = task_id.clone();
        let handle = tokio::spawn(async move {
            match (registry.notifier)(&task_title, &body_owned) {
                Ok(()) => registry.complete_notify(
                    task_id_task,
                    task_title.clone(),
                    format!("Notification sent: {task_title}"),
                ),
                Err(e) => registry.fail(task_id_task, task_title, e),
            }
        });

        self.state.lock().running.insert(
            task_id.clone(),
            RunningTask {
                name: title_owned,
                kind: TaskKind::Notify,
                ends_at: None,
                handle,
            },
        );
        Ok(task_id)
    }

    /// Cancels the timer with the given name.
    pub fn stop_timer(&self, name: &str) -> TimerStopOutcome {
        let mut state = self.state.lock();
        if let Some(task_id) = state.timer_names.remove(name)
            && let Some(task) = state.running.remove(&task_id)
        {
            task.handle.abort();
            state
                .terminal
                .insert(task_id, (name.to_string(), DeferredStatus::Cancelled));
            return TimerStopOutcome::Stopped {
                name: name.to_string(),
            };
        }
        if state.finished.iter().any(|(n, _)| n == name) {
            return TimerStopOutcome::AlreadyFinished {
                name: name.to_string(),
            };
        }
        TimerStopOutcome::NotFound {
            name: name.to_string(),
        }
    }

    /// Cancels a task by its `task_id` (host-driven cancellation).
    ///
    /// Returns `false` when no such task is known.
    pub fn cancel(&self, task_id: &str) -> bool {
        let mut state = self.state.lock();
        let Some(task) = state.running.remove(task_id) else {
            return false;
        };
        task.handle.abort();
        if state
            .timer_names
            .get(&task.name)
            .is_some_and(|id| id == task_id)
        {
            state.timer_names.remove(&task.name);
        }
        state
            .terminal
            .insert(task_id.to_string(), (task.name, DeferredStatus::Cancelled));
        true
    }

    /// Returns running timers (with seconds remaining) and the names of
    /// finished ones, for `utility.timer_stop` without a name.
    pub fn list(&self) -> (Vec<RunningTimerInfo>, Vec<String>) {
        let state = self.state.lock();
        let now = Instant::now();
        let running = state
            .running
            .iter()
            .filter(|(_, t)| t.kind == TaskKind::Timer)
            .map(|(_, t)| {
                let remaining = t.ends_at.map_or(0, |ends_at| {
                    ends_at.saturating_duration_since(now).as_secs()
                });
                RunningTimerInfo {
                    name: t.name.clone(),
                    remaining_seconds: remaining,
                }
            })
            .collect();
        let finished = state.finished.iter().map(|(n, _)| n.clone()).collect();
        (running, finished)
    }

    /// Reports the status of a task by `task_id` for one host poll.
    ///
    /// Terminal statuses (completed/cancelled/failed) are returned once and
    /// then forgotten, matching the host's single-poll consumption.
    pub fn poll(&self, task_id: &str) -> DeferredStatus {
        let mut state = self.state.lock();
        if let Some((_, status)) = state.terminal.remove(task_id) {
            return status;
        }
        if state.running.contains_key(task_id) {
            return DeferredStatus::Pending;
        }
        DeferredStatus::Unknown
    }

    fn complete_timer(&self, task_id: String, name: String, result: String) {
        let mut state = self.state.lock();
        // The task may already have been cancelled/stopped, in which case
        // `running` no longer holds it and the terminal status is final.
        if state.running.remove(&task_id).is_none() {
            return;
        }
        if state.timer_names.get(&name) == Some(&task_id) {
            state.timer_names.remove(&name);
        }
        state.finished.push_back((name.clone(), Instant::now()));
        if state.finished.len() > MAX_FINISHED_TIMERS {
            state.finished.pop_front();
        }
        state.terminal.insert(
            task_id,
            (
                name,
                DeferredStatus::Completed {
                    result: ToolResult::text(result),
                },
            ),
        );
    }

    fn complete_notify(&self, task_id: String, name: String, result: String) {
        let mut state = self.state.lock();
        if state.running.remove(&task_id).is_none() {
            return;
        }
        state.terminal.insert(
            task_id,
            (
                name,
                DeferredStatus::Completed {
                    result: ToolResult::text(result),
                },
            ),
        );
    }

    fn fail(&self, task_id: String, name: String, error: String) {
        let mut state = self.state.lock();
        if state.running.remove(&task_id).is_none() {
            return;
        }
        if state.timer_names.get(&name) == Some(&task_id) {
            state.timer_names.remove(&name);
        }
        state
            .terminal
            .insert(task_id, (name, DeferredStatus::Failed { error }));
    }
}

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn registry_with(
        notifier: impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Arc<TaskRegistry> {
        Arc::new(TaskRegistry::with_notifier(Arc::new(notifier)))
    }

    fn ok_notifier() -> impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static {
        |_, _| Ok(())
    }

    #[tokio::test]
    async fn timer_transitions_pending_to_completed() {
        let registry = registry_with(ok_notifier());
        let task_id = registry
            .start_timer("pasta", Duration::from_millis(50))
            .unwrap();
        assert_eq!(registry.poll(&task_id), DeferredStatus::Pending);

        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            registry.poll(&task_id),
            DeferredStatus::Completed {
                result: ToolResult::text("Timer 'pasta' finished after 0 seconds.")
            }
        );
        // Terminal statuses are one-shot.
        assert_eq!(registry.poll(&task_id), DeferredStatus::Unknown);
    }

    #[tokio::test]
    async fn timer_stop_cancels_and_reports_cancelled() {
        let registry = registry_with(ok_notifier());
        let task_id = registry
            .start_timer("break", Duration::from_mins(5))
            .unwrap();
        assert_eq!(
            registry.stop_timer("break"),
            TimerStopOutcome::Stopped {
                name: "break".to_string()
            }
        );
        assert_eq!(registry.poll(&task_id), DeferredStatus::Cancelled);
        assert_eq!(
            registry.stop_timer("break"),
            TimerStopOutcome::NotFound {
                name: "break".to_string()
            }
        );
    }

    #[tokio::test]
    async fn timer_reports_already_finished() {
        let registry = registry_with(ok_notifier());
        let _task_id = registry
            .start_timer("done", Duration::from_millis(50))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!(
            registry.stop_timer("done"),
            TimerStopOutcome::AlreadyFinished {
                name: "done".to_string()
            }
        );
    }

    #[tokio::test]
    async fn restart_same_name_cancels_previous() {
        let registry = registry_with(ok_notifier());
        let first = registry
            .start_timer("pasta", Duration::from_mins(5))
            .unwrap();
        let second = registry
            .start_timer("pasta", Duration::from_millis(50))
            .unwrap();
        assert_ne!(first, second);
        assert_eq!(registry.poll(&first), DeferredStatus::Cancelled);
        assert_eq!(registry.poll(&second), DeferredStatus::Pending);
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!(
            matches!(registry.poll(&second), DeferredStatus::Completed { .. }),
            "second timer should complete"
        );
    }

    #[tokio::test]
    async fn notify_completes_with_result() {
        let registry = registry_with(ok_notifier());
        let task_id = registry.start_notify("Heads up", "Build done").unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            matches!(
                registry.poll(&task_id),
                DeferredStatus::Completed { result }
                    if result.text_for_llm().contains("Notification sent")
            ),
            "expected completed notification"
        );
    }

    #[tokio::test]
    async fn notify_failure_reports_failed() {
        let registry = registry_with(|_, _| Err("no D-Bus session".to_string()));
        let task_id = registry.start_notify("Oops", "x").unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            registry.poll(&task_id),
            DeferredStatus::Failed {
                error: "no D-Bus session".to_string()
            }
        );
    }

    #[tokio::test]
    async fn cancel_by_task_id() {
        let registry = registry_with(ok_notifier());
        let task_id = registry.start_timer("x", Duration::from_mins(5)).unwrap();
        assert!(registry.cancel(&task_id));
        assert_eq!(registry.poll(&task_id), DeferredStatus::Cancelled);
        assert!(!registry.cancel(&task_id));
    }

    #[tokio::test]
    async fn list_reports_running_and_finished() {
        let registry = registry_with(ok_notifier());
        let _long = registry
            .start_timer("long", Duration::from_mins(5))
            .unwrap();
        let _short = registry
            .start_timer("short", Duration::from_millis(50))
            .unwrap();
        let _notify = registry.start_notify("n", "n").unwrap();

        let (running, _finished) = registry.list();
        assert_eq!(running.len(), 2);
        assert!(
            running
                .iter()
                .any(|t| t.name == "long" && (299..=300).contains(&t.remaining_seconds))
        );
        assert!(running.iter().any(|t| t.name == "short"));

        tokio::time::sleep(Duration::from_millis(120)).await;
        let (running, finished) = registry.list();
        assert_eq!(running.len(), 1);
        assert_eq!(finished, vec!["short".to_string()]);
    }

    #[tokio::test]
    async fn rejects_invalid_args() {
        let registry = Arc::new(TaskRegistry::new());
        assert!(matches!(
            registry.start_timer("", Duration::from_secs(1)),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            registry.start_timer("x", Duration::ZERO),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            registry.start_notify("", "body"),
            Err(ToolError::InvalidArguments { .. })
        ));
    }

    #[tokio::test]
    async fn finished_history_is_bounded() {
        let registry = registry_with(ok_notifier());
        for i in 0..(MAX_FINISHED_TIMERS + 10) {
            let task_id = Uuid::new_v4().to_string();
            let handle = tokio::spawn(async {});
            registry.state.lock().running.insert(
                task_id.clone(),
                RunningTask {
                    name: format!("t{i}"),
                    kind: TaskKind::Timer,
                    ends_at: None,
                    handle,
                },
            );
            registry.complete_timer(task_id, format!("t{i}"), "done".to_string());
        }
        let (_running, finished) = registry.list();
        assert_eq!(finished.len(), MAX_FINISHED_TIMERS);
    }

    #[tokio::test]
    async fn notifier_invoked_once_on_timer_fire() {
        let seen = Arc::new(AtomicUsize::new(0));
        let seen_clone = Arc::clone(&seen);
        let registry = registry_with(move |summary, body| {
            seen_clone.fetch_add(1, Ordering::SeqCst);
            assert!(summary.contains("Timer finished"));
            assert!(body.contains("elapsed"));
            Ok(())
        });
        let _task_id = registry
            .start_timer("t", Duration::from_millis(20))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }
}
