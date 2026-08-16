use crate::result::ok_json;
use crate::task_registry::{TaskRegistry, TimerStopOutcome};
use ene_plugin::prelude::*;
use std::sync::Arc;
use std::time::Duration;

fn default_registry() -> Arc<TaskRegistry> {
    Arc::new(TaskRegistry::new())
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "timer_start",
    summary = "Start a countdown timer that ends with a desktop notification.",
    description = "Starts a background timer named `name` that counts down for `seconds` and then shows a desktop notification on the user's machine. The call returns immediately; completion is reported to the host asynchronously. If a timer with the same name is already running, it is restarted with the new duration. Use utility.timer_stop to cancel a timer early or check its status.",
    category = "Utility",
    keywords_primary = "timer, countdown, alarm, remind, notification",
    background_capable
)]
pub struct TimerStartAction {
    /// Name identifying this timer; used by `utility.timer_stop`.
    #[arg(min_length = 1)]
    name: String,
    #[arg(minimum = 1)]
    seconds: u64,
}

impl TimerStartAction {
    /// Schedules the timer on `registry`, returning its `task_id`.
    ///
    /// Called by the provider's deferred dispatch; the synchronous
    /// [`run`](Self::run) path is unreachable because the host always
    /// invokes background-capable tools in deferred mode.
    pub fn schedule(&self, registry: &Arc<TaskRegistry>) -> Result<String, ToolError> {
        registry.start_timer(&self.name, Duration::from_secs(self.seconds))
    }

    async fn run(&self) -> Result<String, ToolError> {
        Err(ToolError::internal(
            "utility.timer_start must be executed in deferred (background) mode".to_string(),
        ))
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "timer_stop",
    summary = "Stop a running timer or report timer status.",
    description = "With `name`, cancels the matching running timer and reports whether it was stopped, already finished, or unknown. Without `name`, lists all running timers with their remaining seconds plus the names of timers that already finished — use this to confirm the status of timers started with utility.timer_start.",
    category = "Utility",
    keywords_primary = "timer, countdown, alarm, stop, cancel, status, confirm"
)]
pub struct TimerStopAction {
    #[tool(skip)]
    #[serde(skip, default = "default_registry")]
    registry: Arc<TaskRegistry>,
    /// Name of the timer to stop. Omit to list running and finished timers.
    #[serde(default)]
    name: Option<String>,
}

impl TimerStopAction {
    #[must_use]
    pub const fn new(registry: Arc<TaskRegistry>) -> Self {
        Self {
            registry,
            name: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let Some(name) = &self.name else {
            let (running, finished) = self.registry.list();
            return ok_json(&serde_json::json!({ "running": running, "finished": finished }));
        };
        match self.registry.stop_timer(name) {
            TimerStopOutcome::Stopped { name } => {
                ok_json(&serde_json::json!({ "status": "stopped", "name": name }))
            }
            TimerStopOutcome::AlreadyFinished { name } => {
                ok_json(&serde_json::json!({ "status": "already_finished", "name": name }))
            }
            TimerStopOutcome::NotFound { name } => {
                ok_json(&serde_json::json!({ "status": "not_found", "name": name }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::NotifySendAction;
    use ene_plugin_proto::{DeferredStatus, ErrorKind};

    fn registry_with(
        notifier: impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Arc<TaskRegistry> {
        Arc::new(TaskRegistry::with_notifier(Arc::new(notifier)))
    }

    fn ok_notifier() -> impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static {
        |_, _| Ok(())
    }

    #[test]
    fn timer_start_args_deserialize() {
        let json = r#"{"name":"pasta","seconds":300}"#;
        let a: TimerStartAction = serde_json::from_str(json).unwrap();
        assert_eq!(a.name, "pasta");
        assert_eq!(a.seconds, 300);
    }

    #[test]
    fn timer_start_rejects_missing_seconds() {
        let err = serde_json::from_str::<TimerStartAction>(r#"{"name":"pasta"}"#).unwrap_err();
        assert!(
            err.to_string().contains("seconds"),
            "expected missing-field error mentioning `seconds`, got: {err}"
        );
    }

    #[test]
    fn timer_stop_args_optional_name() {
        let a: TimerStopAction = serde_json::from_str(r#"{"name":"pasta"}"#).unwrap();
        assert_eq!(a.name.as_deref(), Some("pasta"));
        let b: TimerStopAction = serde_json::from_str(r"{}").unwrap();
        assert!(b.name.is_none());
    }

    #[test]
    fn specs_declare_background_capability() {
        assert!(TimerStartAction::spec().background_capable);
        assert!(NotifySendAction::spec().background_capable);
        assert!(!TimerStopAction::spec().background_capable);
    }

    #[test]
    fn timer_start_schema_constrains_seconds() {
        let schema = TimerStartAction::spec().parameters;
        let seconds = schema
            .get("properties")
            .and_then(|p| p.get("seconds"))
            .unwrap();
        assert_eq!(
            seconds.get("type").and_then(serde_json::Value::as_str),
            Some("integer")
        );
        assert_eq!(
            seconds.get("minimum").and_then(serde_json::Value::as_i64),
            Some(1)
        );
        assert_eq!(
            schema
                .get("properties")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.get("minLength"))
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
    }

    #[tokio::test]
    async fn timer_stop_run_stops_running_timer() {
        let registry = registry_with(ok_notifier());
        let action = TimerStopAction {
            registry: registry.clone(),
            name: Some("pasta".to_string()),
        };
        let _task_id = registry
            .start_timer("pasta", Duration::from_mins(5))
            .unwrap();

        let result = action.run().await.unwrap();
        assert!(result.contains("\"status\": \"stopped\""), "got: {result}");
        assert!(result.contains("\"name\": \"pasta\""), "got: {result}");
    }

    #[tokio::test]
    async fn timer_stop_run_lists_timers_without_name() {
        let registry = registry_with(ok_notifier());
        let action = TimerStopAction::new(registry.clone());
        let _task_id = registry
            .start_timer("pasta", Duration::from_mins(5))
            .unwrap();

        let result = action.run().await.unwrap();
        assert!(result.contains("\"running\""), "got: {result}");
        assert!(result.contains("\"name\": \"pasta\""), "got: {result}");
        assert!(
            result.contains("\"remaining_seconds\": 300")
                || result.contains("\"remaining_seconds\": 299"),
            "got: {result}"
        );
    }

    #[tokio::test]
    async fn schedule_spawns_deferred_task() {
        let registry = registry_with(ok_notifier());
        let action = TimerStartAction {
            name: "pasta".to_string(),
            seconds: 300,
        };
        let task_id = action.schedule(&registry).unwrap();
        assert!(!task_id.is_empty());
        assert_eq!(registry.poll(&task_id), DeferredStatus::Pending);
        assert_eq!(
            registry.stop_timer("pasta"),
            TimerStopOutcome::Stopped {
                name: "pasta".to_string()
            }
        );
    }

    #[tokio::test]
    async fn synchronous_run_is_rejected() {
        let action = TimerStartAction {
            name: "pasta".to_string(),
            seconds: 300,
        };
        assert!(
            matches!(
                action.run().await,
                Err(ToolError::Generic {
                    kind: ErrorKind::Internal,
                    ..
                })
            ),
            "synchronous execution must be rejected"
        );
    }
}
