use crate::action;
use crate::todo_store::TodoStore;
use async_trait::async_trait;
use ene_plugin::{ActionSetProvider, ToolAction};
use ene_plugin_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Arc;

/// Shared state for the todo actions.
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
    /// Creates a new `UtilityState`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(tokio::sync::Mutex::new(None)),
            session_id: Arc::new(parking_lot::RwLock::new(String::new())),
            db_socket: Arc::new(parking_lot::RwLock::new(None)),
            db_auth_token: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Returns the current session ID.
    pub fn session_id(&self) -> String {
        self.session_id.read().clone()
    }

    /// Sets the session ID.
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

    /// Sets the DB IPC auth token used to authenticate the connection.
    pub fn set_db_auth_token(&self, token: Option<String>) {
        *self.db_auth_token.write() = token;
    }

    /// Lazily connects to the DB IPC server and returns the `TodoStore`.
    ///
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
pub struct UtilityToolProvider {
    inner: ActionSetProvider,
}

impl UtilityToolProvider {
    /// Creates a new `UtilityToolProvider`.
    #[must_use]
    pub fn new() -> Self {
        let state = Arc::new(UtilityState::new());
        let actions: Vec<Box<dyn ToolAction>> = vec![
            Box::new(action::AskQuestionAction::default()),
            Box::new(action::TodoListAction::new(state.clone())),
            Box::new(action::TodoAddAction::new(state.clone())),
            Box::new(action::TodoUpdateAction::new(state.clone())),
            Box::new(action::TodoCompleteAction::new(state.clone())),
            Box::new(action::TodoDeleteAction::new(state.clone())),
            Box::new(action::GetCurrentTimeAction::default()),
            Box::new(action::GetSystemInfoAction::default()),
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

        Self { inner }
    }
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

    fn set_call_context(&self, ctx: &ene_plugin_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }
}
