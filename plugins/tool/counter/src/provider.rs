use crate::action;
use crate::approval::ApprovalGate;
use crate::store::{CounterStore, DbCounterStore};
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

#[derive(Clone)]
pub struct CounterState {
    store: Arc<tokio::sync::Mutex<Option<Arc<dyn CounterStore>>>>,
    db_socket: Arc<parking_lot::RwLock<Option<String>>>,
    db_auth_token: Arc<parking_lot::RwLock<Option<String>>>,
    session_id: Arc<parking_lot::RwLock<String>>,
    gate: Arc<ApprovalGate>,
}

impl CounterState {
    /// Creates a new `CounterState` without a store; the store is built
    /// lazily from the sandbox handshake by [`Self::ensure_store`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Arc::new(tokio::sync::Mutex::new(None)),
            db_socket: Arc::new(parking_lot::RwLock::new(None)),
            db_auth_token: Arc::new(parking_lot::RwLock::new(None)),
            session_id: Arc::new(parking_lot::RwLock::new(String::new())),
            gate: Arc::new(ApprovalGate::new()),
        }
    }

    /// Sets the DB IPC socket path and resets the cached store so the
    /// next [`Self::ensure_store`] call reconnects with the new path.
    ///
    /// `try_lock` is used because this method is called from a synchronous
    /// provider hook; if initialization is concurrently in progress the
    /// store will be rebuilt on the next access because the socket has
    /// already been updated. In practice the sandbox handshake runs before
    /// any tool call, so the lock is uncontended.
    pub fn set_db_socket(&self, socket: String) {
        *self.db_socket.write() = Some(socket);
        if let Ok(mut guard) = self.store.try_lock() {
            *guard = None;
        }
    }

    pub fn set_db_auth_token(&self, token: Option<String>) {
        *self.db_auth_token.write() = token;
    }

    pub fn session_id(&self) -> String {
        self.session_id.read().clone()
    }

    pub fn set_session_id(&self, session_id: &str) {
        *self.session_id.write() = session_id.to_string();
    }

    #[must_use]
    pub fn gate(&self) -> &ApprovalGate {
        &self.gate
    }

    /// Lazily connects to the DB IPC server and returns the store.
    ///
    /// The `tokio::sync::Mutex` guard is held across the entire
    /// initialization (socket read → connect → store) so concurrent
    /// callers serialize on the lock rather than racing to create
    /// duplicate connections.
    pub async fn ensure_store(&self) -> Result<Arc<dyn CounterStore>, ToolError> {
        let mut guard = self.store.lock().await;
        if let Some(store) = guard.as_ref() {
            return Ok(store.clone());
        }

        let socket = self.db_socket.read().clone();
        let socket_path = match socket.as_deref() {
            Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => {
                return Err(ToolError::internal(
                    "DB socket path not configured for counter tool".to_string(),
                ));
            }
        };

        let token = self.db_auth_token.read().clone();
        let store = DbCounterStore::new(&socket_path, token.as_deref())
            .await
            .map_err(|e| ToolError::internal(format!("Failed to connect to DB: {e}")))?;
        let store: Arc<dyn CounterStore> = Arc::new(store);
        *guard = Some(store.clone());
        Ok(store)
    }

    /// Installs a store directly, bypassing the DB connection. Test seam
    /// for exercising actions without a live DB server.
    #[cfg(test)]
    pub async fn set_test_store(&self, store: Arc<dyn CounterStore>) {
        *self.store.lock().await = Some(store);
    }
}

impl Default for CounterState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CounterState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The store is a trait object without Debug; the remaining fields
        // are enough context for action-level test failures.
        f.debug_struct("CounterState")
            .field("session_id", &self.session_id)
            .field("db_socket", &self.db_socket)
            .finish_non_exhaustive()
    }
}

/// Sample tool provider built on [`ActionSetProvider`].
///
/// The two provider-specific pieces of state — the session ID and the
/// DB sandbox socket/token — are threaded into [`CounterState`] via
/// hooks, and the approval gate is fed by the permission hooks so
/// per-turn approvals and session-wide allow patterns survive across
/// the host's post-approval re-invocation.
pub struct CounterToolProvider {
    inner: ActionSetProvider,
}

impl CounterToolProvider {
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(CounterState::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::GetAction::new(state.clone())),
            Box::new(action::IncrementAction::new(state.clone())),
            Box::new(action::ResetAction::new(state.clone())),
        ];

        let sandbox_state = state.clone();
        let session_state = state.clone();
        let context_state = state.clone();
        let approve_state = state.clone();
        let allow_state = state.clone();
        let revoke_state = state;
        let inner = ActionSetProvider::new(actions)
            .with_sandbox_hook(move |sandbox: &SandboxConfigData| {
                if let Some(socket) = &sandbox.db_socket {
                    sandbox_state.set_db_socket(socket.clone());
                }
                sandbox_state.set_db_auth_token(sandbox.db_auth_token.clone());
            })
            .with_set_call_context_hook(move |conversation_id, turn_id| {
                session_state.set_session_id(conversation_id);
                context_state
                    .gate()
                    .on_call_context(conversation_id, turn_id);
            })
            .with_approve_permission_hook(move |request_id| {
                approve_state.gate().approve_request(request_id);
            })
            .with_allow_pattern_hook(move |action, target_pattern| {
                allow_state.gate().allow_pattern(action, target_pattern);
            })
            .with_revoke_pattern_hook(move |action, target_pattern| {
                revoke_state.gate().revoke_pattern(action, target_pattern);
            });

        Self { inner }
    }
}

impl Default for CounterToolProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolProvider for CounterToolProvider {
    fn list_specs(&self) -> Vec<ToolSpec> {
        self.inner.list_specs()
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        self.inner.call_tool(name, arguments).await
    }

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }

    fn approve_permission(&self, request_id: &str) {
        self.inner.approve_permission(request_id);
    }

    fn allow_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.allow_pattern(action, target_pattern);
    }

    fn revoke_pattern(&self, action: &str, target_pattern: &str) {
        self.inner.revoke_pattern(action, target_pattern);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryCounterStore;
    use ene_plugin_proto::ToolError;

    async fn provider_with_store() -> CounterToolProvider {
        let state = Arc::new(CounterState::new());
        state
            .set_test_store(Arc::new(InMemoryCounterStore::default()))
            .await;
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::GetAction::new(state.clone())),
            Box::new(action::IncrementAction::new(state.clone())),
            Box::new(action::ResetAction::new(state)),
        ];
        CounterToolProvider {
            inner: ActionSetProvider::new(actions),
        }
    }

    #[tokio::test]
    async fn list_specs_exposes_three_actions() {
        let provider = provider_with_store().await;
        let specs = provider.list_specs();
        let names: Vec<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
        assert_eq!(names, ["counter.get", "counter.increment", "counter.reset"]);
    }

    #[tokio::test]
    async fn unknown_tool_is_not_found() {
        let provider = provider_with_store().await;
        let err = provider.call_tool("counter.nope", "{}").await.unwrap_err();
        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn malformed_arguments_are_invalid() {
        let provider = provider_with_store().await;
        let err = provider
            .call_tool("counter.get", "not json")
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }
}
