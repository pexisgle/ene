use crate::action;
use crate::todo_store::TodoStore;
use async_trait::async_trait;
use ene_tool_common::{ActionSetProvider, ToolAction};
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::Mutex;
use std::sync::{Arc, RwLock};

/// Shared state for the todo actions.
#[derive(Clone)]
pub struct UtilityState {
    todo_store: Arc<Mutex<Option<Arc<TodoStore>>>>,
    session_id: Arc<RwLock<String>>,
    db_socket: Arc<RwLock<Option<String>>>,
    db_auth_token: Arc<RwLock<Option<String>>>,
}

impl UtilityState {
    /// Creates a new `UtilityState`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(Mutex::new(None)),
            session_id: Arc::new(RwLock::new(String::new())),
            db_socket: Arc::new(RwLock::new(None)),
            db_auth_token: Arc::new(RwLock::new(None)),
        }
    }

    /// Returns the current session ID.
    pub fn session_id(&self) -> String {
        self.session_id
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Sets the session ID.
    pub fn set_session_id(&self, session_id: &str) {
        *self
            .session_id
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = session_id.to_string();
    }

    /// Sets the DB IPC socket path and resets the todo store.
    ///
    /// The previous implementation wrote the new socket path first and
    /// then spawned a `tokio::spawn` task to clear the todo store, which
    /// created a window where a concurrent `ensure_todo_store` could
    /// either see the new socket with the *old* store, or clear the
    /// store after a fresh `ensure_todo_store` had already populated it.
    /// Use `blocking_lock` on the todo-store mutex to make the reset
    /// happen atomically with the socket update from the caller's
    /// perspective: the socket write is visible to subsequent readers
    /// only after the old store has been dropped.
    pub fn set_db_socket(&self, socket: String) {
        *self
            .todo_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .db_socket
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(socket);
    }

    /// Sets the DB IPC auth token used to authenticate the connection.
    pub fn set_db_auth_token(&self, token: Option<String>) {
        *self
            .db_auth_token
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = token;
    }

    /// Lazily connects to the DB IPC server and returns the `TodoStore`.
    pub async fn ensure_todo_store(&self) -> Result<Arc<TodoStore>, ToolError> {
        {
            let guard = self
                .todo_store
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(store) = guard.as_ref() {
                return Ok(store.clone());
            }
        }

        let socket = self
            .db_socket
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let socket_path = match socket.as_deref() {
            Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => {
                return Err(ToolError::Internal {
                    message: "DB socket path not configured for utility tool".to_string(),
                });
            }
        };

        let token = self
            .db_auth_token
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let store = TodoStore::new(&socket_path, token.as_deref())
            .await
            .map_err(|e| ToolError::Internal {
                message: format!("Failed to connect to DB: {e}"),
            })?;
        let store = Arc::new(store);

        let mut guard = self
            .todo_store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = guard.clone() {
            drop(guard);
            Ok(existing)
        } else {
            *guard = Some(store.clone());
            drop(guard);
            Ok(store)
        }
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
/// `docs/tools/sdk.md`): `list_specs`/`call_tool` dispatch is handled
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
            .with_set_call_context_hook(move |conv_id| session_state.set_session_id(conv_id))
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

    fn set_call_context(&self, ctx: &ene_tool_proto::CallContext) {
        self.inner.set_call_context(ctx);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        self.inner.set_sandbox(sandbox);
    }
}
