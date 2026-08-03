use crate::task_registry::TaskRegistry;
use ene_plugin::prelude::*;
use std::sync::Arc;

#[derive(Debug, Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "utility",
    name = "notify_send",
    summary = "Send a desktop notification to the user.",
    description = "Shows a desktop notification with the given `title` and `message` on the user's machine. The call returns immediately; delivery is confirmed to the host asynchronously. Use this to alert the user about long-running work, background tasks, or anything that needs their attention outside the chat.",
    category = "Utility",
    keywords_primary = "notify, notification, alert, remind, popup",
    background_capable
)]
/// Action to send a desktop notification.
pub struct NotifySendAction {
    /// Notification title.
    #[arg(min_length = 1)]
    title: String,
    /// Notification body text.
    message: String,
}

impl NotifySendAction {
    /// Dispatches the notification on `registry`, returning its `task_id`.
    ///
    /// Called by the provider's deferred dispatch; the synchronous
    /// [`run`](Self::run) path is unreachable because the host always
    /// invokes background-capable tools in deferred mode.
    pub fn dispatch(&self, registry: &Arc<TaskRegistry>) -> Result<String, ToolError> {
        registry.start_notify(&self.title, &self.message)
    }

    async fn run(&self) -> Result<String, ToolError> {
        Err(ToolError::internal(
            "utility.notify_send must be executed in deferred (background) mode".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ene_plugin_proto::{DeferredStatus, ErrorKind};

    fn registry_with(
        notifier: impl Fn(&str, &str) -> Result<(), String> + Send + Sync + 'static,
    ) -> Arc<TaskRegistry> {
        Arc::new(TaskRegistry::with_notifier(Arc::new(notifier)))
    }

    #[test]
    fn args_deserialize() {
        let json = r#"{"title":"Build done","message":"All tests passed"}"#;
        let a: NotifySendAction = serde_json::from_str(json).unwrap();
        assert_eq!(a.title, "Build done");
        assert_eq!(a.message, "All tests passed");
    }

    #[test]
    fn args_reject_missing_title() {
        let err = serde_json::from_str::<NotifySendAction>(r#"{"message":"hi"}"#).unwrap_err();
        assert!(
            err.to_string().contains("title"),
            "expected missing-field error mentioning `title`, got: {err}"
        );
    }

    #[test]
    fn spec_declares_background_capable() {
        assert!(NotifySendAction::spec().background_capable);
    }

    #[tokio::test]
    async fn dispatch_spawns_deferred_task() {
        let registry = registry_with(|_, _| Ok(()));
        let action = NotifySendAction {
            title: "Heads up".to_string(),
            message: "Done".to_string(),
        };
        let task_id = action.dispatch(&registry).unwrap();
        assert!(!task_id.is_empty());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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
    async fn synchronous_run_is_rejected() {
        let action = NotifySendAction {
            title: "Heads up".to_string(),
            message: "Done".to_string(),
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
