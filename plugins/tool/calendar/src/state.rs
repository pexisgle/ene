use crate::approval::ApprovalGate;
use crate::registry::{LocalCalendarProvider, ProviderRegistry};
use crate::store::CalendarStore;
use ene_plugin_proto::ToolError;
use std::sync::Arc;

/// Shared state for the calendar actions.
///
/// Mirrors `ene-plugin-utility`'s state threading: the DB socket and auth
/// token arrive via the sandbox handshake, and the store is lazily
/// initialized once so concurrent callers serialize on the lock instead of
/// racing to create duplicate connections.
#[derive(Clone)]
pub struct CalendarState {
    store: Arc<tokio::sync::Mutex<Option<Arc<CalendarStore>>>>,
    session_id: Arc<parking_lot::RwLock<String>>,
    db_socket: Arc<parking_lot::RwLock<Option<String>>>,
    db_auth_token: Arc<parking_lot::RwLock<Option<String>>>,
    gate: ApprovalGate,
    registry: ProviderRegistry,
}

impl CalendarState {
    /// Creates a new `CalendarState` with the built-in local provider
    /// registered.
    #[must_use]
    pub fn new() -> Self {
        let mut registry = ProviderRegistry::default();
        registry.register(Arc::new(LocalCalendarProvider::new()));
        Self {
            store: Arc::new(tokio::sync::Mutex::new(None)),
            session_id: Arc::new(parking_lot::RwLock::new(String::new())),
            db_socket: Arc::new(parking_lot::RwLock::new(None)),
            db_auth_token: Arc::new(parking_lot::RwLock::new(None)),
            gate: ApprovalGate::new(),
            registry,
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

    /// Sets the DB IPC socket path and resets the cached store.
    ///
    /// `try_lock` is used because this method is called from a synchronous
    /// provider hook; if initialization is concurrently in progress the
    /// store is rebuilt on the next access because the socket has already
    /// been updated. In practice this runs during the sandbox handshake
    /// before any tool calls, so the lock is uncontended.
    pub fn set_db_socket(&self, socket: String) {
        *self.db_socket.write() = Some(socket);
        if let Ok(mut guard) = self.store.try_lock() {
            *guard = None;
        }
    }

    /// Sets the DB IPC auth token used to authenticate the connection.
    pub fn set_db_auth_token(&self, token: Option<String>) {
        *self.db_auth_token.write() = token;
    }

    /// Lazily connects to the DB IPC server and returns the `CalendarStore`.
    ///
    /// The `tokio::sync::Mutex` guard is held across the entire
    /// initialization (socket read → connect → store) so concurrent callers
    /// serialize on the lock rather than racing to create duplicate
    /// connections.
    pub async fn ensure_store(&self) -> Result<Arc<CalendarStore>, ToolError> {
        let mut guard = self.store.lock().await;
        if let Some(store) = guard.as_ref() {
            return Ok(store.clone());
        }

        let socket = self.db_socket.read().clone();
        let socket_path = match socket.as_deref() {
            Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
            _ => {
                return Err(ToolError::internal(
                    "DB socket path not configured for calendar tool".to_string(),
                ));
            }
        };

        let token = self.db_auth_token.read().clone();

        let store = CalendarStore::new(&socket_path, token.as_deref())
            .await
            .map_err(|e| ToolError::internal(format!("Failed to connect to DB: {e}")))?;
        let store = Arc::new(store);
        *guard = Some(store.clone());
        Ok(store)
    }

    /// Returns the approval gate shared by all calendar actions.
    pub fn gate(&self) -> &ApprovalGate {
        &self.gate
    }

    /// Returns the provider registry for the configured account kinds.
    pub fn registry(&self) -> &ProviderRegistry {
        &self.registry
    }
}

impl Default for CalendarState {
    fn default() -> Self {
        Self::new()
    }
}
