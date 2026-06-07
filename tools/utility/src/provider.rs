use crate::action;
use crate::todo_store::TodoStore;
use async_trait::async_trait;
use ene_tool_common::ToolAction;
use ene_tool_proto::{SandboxConfigData, ToolError, ToolProvider, ToolSpec};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

/// Shared state for the todo actions.
#[derive(Clone)]
pub struct UtilityState {
    todo_store: Arc<Mutex<Option<Arc<TodoStore>>>>,
    session_id: Arc<RwLock<String>>,
    db_socket: Arc<RwLock<Option<String>>>,
}

impl UtilityState {
    /// Creates a new `UtilityState`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            todo_store: Arc::new(Mutex::new(None)),
            session_id: Arc::new(RwLock::new(String::new())),
            db_socket: Arc::new(RwLock::new(None)),
        }
    }

    /// Returns the current session ID.
    #[must_use]
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
    pub fn set_db_socket(&self, socket: String) {
        *self
            .db_socket
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(socket);
        let store = self.todo_store.clone();
        tokio::spawn(async move {
            *store.lock().await = None;
        });
    }

    /// Lazily connects to the DB IPC server and returns the `TodoStore`.
    pub async fn ensure_todo_store(&self) -> Result<Arc<TodoStore>, ToolError> {
        {
            let guard = self.todo_store.lock().await;
            if let Some(store) = guard.clone() {
                return Ok(store);
            }
        }

        let mut guard = self.todo_store.lock().await;
        if let Some(store) = guard.clone() {
            return Ok(store);
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

        let store = TodoStore::new(&socket_path)
            .await
            .map_err(|e| ToolError::Internal {
                message: format!("Failed to connect to DB: {e}"),
            })?;
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
pub struct UtilityToolProvider {
    actions: Vec<Box<dyn ToolAction>>,
    state: UtilityState,
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
        Self {
            actions,
            state: (*state).clone(),
        }
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
        self.actions.iter().map(|a| a.definition()).collect()
    }
    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolError> {
        for action in &self.actions {
            if action.name() == name {
                return action.execute(arguments).await;
            }
        }
        Err(ToolError::NotFound {
            tool_name: name.to_string(),
        })
    }

    fn set_session_id(&self, session_id: &str) {
        self.state.set_session_id(session_id);
    }

    fn set_sandbox(&self, sandbox: &SandboxConfigData) {
        if let Some(socket) = &sandbox.db_socket {
            self.state.set_db_socket(socket.clone());
        }
    }
}
