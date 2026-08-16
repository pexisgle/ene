use crate::action;
use crate::task_registry::TaskRegistry;
use crate::todo_store::TodoStore;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{
    DeferredOutcome, DeferredStatus, SandboxConfigData, ToolError, ToolProvider, ToolResult,
    ToolSpec,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct UtilityState {
    /// Lazily-initialized todo store. Uses `tokio::sync::Mutex` so the
    /// lock can be held across the entire async initialization in
    /// [`ensure_todo_store`](Self::ensure_todo_store), eliminating the
    /// TOCTOU race inherent in double-checked locking.
    todo_store: Arc<tokio::sync::Mutex<Option<Arc<TodoStore>>>>,
    session_id: Arc<parking_lot::RwLock<String>>,
    db_socket: Arc<parking_lot::RwLock<Option<String>>>,
    db_auth_token: Arc<parking_lot::RwLock<Option<String>>>,
}

impl UtilityState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(tokio::sync::Mutex::new(None)),
            session_id: Arc::new(parking_lot::RwLock::new(String::new())),
            db_socket: Arc::new(parking_lot::RwLock::new(None)),
            db_auth_token: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    pub fn session_id(&self) -> String {
        self.session_id.read().clone()
    }

    pub fn set_session_id(&self, session_id: &str) {
        *self.session_id.write() = session_id.to_string();
    }

    /// Sets the DB IPC socket path and resets the todo store.
    ///
    /// The socket is updated first, then the cached store is cleared so
    /// the next [`ensure_todo_store`](Self::ensure_todo_store) call
    /// reconnects with the new path. `try_lock` is used because this
    /// method is called from a synchronous provider hook; if
    /// initialization is concurrently in progress the store will be
    /// rebuilt on the next access because the socket has already been
    /// updated. In practice `set_db_socket` is invoked during the
    /// sandbox handshake before any tool calls, so the lock is
    /// uncontended.
    pub fn set_db_socket(&self, socket: String) {
        *self.db_socket.write() = Some(socket);
        if let Ok(mut guard) = self.todo_store.try_lock() {
            *guard = None;
        }
    }

    pub fn set_db_auth_token(&self, token: Option<String>) {
        *self.db_auth_token.write() = token;
    }

    /// The `tokio::sync::Mutex` guard is held across the entire
    /// initialization (socket read → connect → store) so concurrent
    /// callers serialize on the lock rather than racing to create
    /// duplicate connections.
    pub async fn ensure_todo_store(&self) -> Result<Arc<TodoStore>, ToolError> {
        let mut guard = self.todo_store.lock().await;
        if let Some(store) = guard.as_ref() {
            return Ok(store.clone());
        }

        let socket = self.db_socket.read().clone();
        let socket_path = match socket.as_deref() {
            Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => {
                return Err(ToolError::internal(
                    "DB socket path not configured for utility tool".to_string(),
                ));
            }
        };

        let token = self.db_auth_token.read().clone();

        let store = TodoStore::new(&socket_path, token.as_deref())
            .await
            .map_err(|e| ToolError::internal(format!("Failed to connect to DB: {e}")))?;
        let store = Arc::new(store);
        *guard = Some(store.clone());
        Ok(store)
    }
}

impl Default for UtilityState {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in utility tool provider.
///
/// Built on [`ActionSetProvider`] (see the tool-ABI adapter documented in
/// `docs/reference/tools/sdk.md`): `list_specs`/`call_tool` dispatch is handled
/// generically, and the two utility-specific pieces of `ToolProvider`
/// state — session ID and the DB sandbox socket/token — are threaded into
/// `UtilityState` via hooks instead of a hand-written `ToolProvider` impl.
///
/// Deferred (background) execution is the exception: [`ActionSetProvider`]
/// deliberately leaves `call_tool_deferred`/`poll_deferred`/`cancel_deferred`
/// at their synchronous defaults (see its module docs), so this provider
/// overrides the three deferred methods to dispatch `utility.timer_start`
/// and `utility.notify_send` onto the shared [`TaskRegistry`].
pub struct UtilityToolProvider {
    inner: ActionSetProvider,
    tasks: Arc<TaskRegistry>,
}

impl UtilityToolProvider {
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(UtilityState::new());
        let tasks = Arc::new(TaskRegistry::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::AskQuestionAction::default()),
            Box::new(action::TodoListAction::new(state.clone())),
            Box::new(action::TodoAddAction::new(state.clone())),
            Box::new(action::TodoUpdateAction::new(state.clone())),
            Box::new(action::TodoCompleteAction::new(state.clone())),
            Box::new(action::TodoDeleteAction::new(state.clone())),
            Box::new(action::GetCurrentTimeAction::default()),
            Box::new(action::GetSystemInfoAction::default()),
            Box::new(action::TimerStartAction::default()),
            Box::new(action::TimerStopAction::new(tasks.clone())),
            Box::new(action::NotifySendAction::default()),
        ];

        let session_state = state.clone();
        let sandbox_state = state;
        let inner = ActionSetProvider::new(actions)
            .with_set_call_context_hook(move |conv_id, _turn_id| {
                session_state.set_session_id(conv_id);
            })
            .with_sandbox_hook(move |sandbox: &SandboxConfigData| {
                if let Some(socket) = &sandbox.db_socket {
                    sandbox_state.set_db_socket(socket.clone());
                }
                sandbox_state.set_db_auth_token(sandbox.db_auth_token.clone());
            });

        Self { inner, tasks }
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(name: &str, arguments: &str) -> Result<T, ToolError> {
    serde_json::from_str(arguments).map_err(|e| ToolError::InvalidArguments {
        message: format!("Invalid arguments for {name}: {e}"),
    })
}

impl Default for UtilityToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for UtilityToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    async fn call_tool_deferred(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<DeferredOutcome, ToolError> {
        let task_id = match name {
            action::TimerStartAction::TOOL_NAME => {
                let args: action::TimerStartAction = parse_args(name, arguments)?;
                args.schedule(&self.tasks)?
            }
            action::NotifySendAction::TOOL_NAME => {
                let args: action::NotifySendAction = parse_args(name, arguments)?;
                args.dispatch(&self.tasks)?
            }
            _ => {
                return Ok(DeferredOutcome::Sync(ToolResult::text(
                    self.inner.call_tool(name, arguments).await?,
                )));
            }
        };
        Ok(DeferredOutcome::Deferred { task_id })
    }

    fn poll_deferred(&self, task_id: &str) -> DeferredStatus {
        self.tasks.poll(task_id)
    }

    fn cancel_deferred(&self, task_id: &str) {
        self.tasks.cancel(task_id);
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> UtilityToolProvider {
        UtilityToolProvider::new()
    }

    #[tokio::test]
    async fn deferred_dispatch_starts_polls_and_cancels_timer() {
        let provider = provider();
        let outcome = provider
            .call_tool_deferred(
                action::TimerStartAction::TOOL_NAME,
                r#"{"name":"pasta","seconds":300}"#,
            )
            .await
            .unwrap();
        let DeferredOutcome::Deferred { task_id } = outcome else {
            unreachable!("timer_start is background-capable");
        };

        assert_eq!(provider.poll_deferred(&task_id), DeferredStatus::Pending);
        provider.cancel_deferred(&task_id);
        assert_eq!(provider.poll_deferred(&task_id), DeferredStatus::Cancelled);
        assert_eq!(provider.poll_deferred(&task_id), DeferredStatus::Unknown);
    }

    #[tokio::test]
    async fn deferred_dispatch_accepts_notify() {
        let provider = provider();
        let outcome = provider
            .call_tool_deferred(
                action::NotifySendAction::TOOL_NAME,
                r#"{"title":"Heads up","message":"Done"}"#,
            )
            .await
            .unwrap();
        let DeferredOutcome::Deferred { task_id } = outcome else {
            unreachable!("notify_send is background-capable");
        };
        // Best-effort abort: keeps a real notification from popping up on
        // developer machines while exercising `cancel_deferred`.
        provider.cancel_deferred(&task_id);
    }

    #[tokio::test]
    async fn deferred_dispatch_falls_back_to_sync() {
        let provider = provider();
        let outcome = provider
            .call_tool_deferred(action::TimerStopAction::TOOL_NAME, r"{}")
            .await
            .unwrap();
        let DeferredOutcome::Sync(result) = outcome else {
            unreachable!("timer_stop is not background-capable");
        };
        assert!(
            result.text_for_llm().contains("\"running\""),
            "sync fallback must run the action, got: {}",
            result.text_for_llm()
        );
    }

    #[tokio::test]
    async fn deferred_dispatch_unknown_tool_is_not_found() {
        let provider = provider();
        let err = provider
            .call_tool_deferred("utility.nope", "{}")
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn timer_stop_via_execute_uses_shared_registry() {
        // `TimerStopAction` carries the registry in a `#[tool(skip)]` field
        // that `execute()` re-copies from `self` after deserialization; the
        // serde default would otherwise fall back to a fresh empty registry
        // and silently report every timer as not found.
        let provider = provider();
        let outcome = provider
            .call_tool_deferred(
                action::TimerStartAction::TOOL_NAME,
                r#"{"name":"pasta","seconds":300}"#,
            )
            .await
            .unwrap();
        let DeferredOutcome::Deferred { .. } = outcome else {
            unreachable!("timer_start is background-capable");
        };

        let result = provider
            .call_tool(action::TimerStopAction::TOOL_NAME, r#"{"name":"pasta"}"#)
            .await
            .unwrap();
        assert!(
            result.contains("\"status\": \"stopped\""),
            "timer_stop must see the timer started through the provider, got: {result}"
        );
    }
}
