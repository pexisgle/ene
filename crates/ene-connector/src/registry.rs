use crate::connector::{Connector, ConnectorId, ConnectorStatus};
use crate::credential::CredentialStore;
use crate::error::ConnectorError;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// A shared, mutex-guarded connector handle stored in the registry.
type ConnectorHandle = Arc<tokio::sync::Mutex<Box<dyn Connector>>>;

pub struct ConnectorRegistry {
    connectors: RwLock<HashMap<ConnectorId, ConnectorHandle>>,
}

impl fmt::Debug for ConnectorRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.connectors.read();
        let ids: Vec<&ConnectorId> = guard.keys().collect();
        f.debug_struct("ConnectorRegistry")
            .field("connector_count", &ids.len())
            .field("connector_ids", &ids)
            .finish()
    }
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self {
            connectors: RwLock::new(HashMap::new()),
        }
    }
    pub fn register(&self, connector: Box<dyn Connector>) -> Result<(), ConnectorError> {
        let id = connector.id().clone();
        let mut guard = self.connectors.write();
        if guard.contains_key(&id) {
            return Err(ConnectorError::internal(format!(
                "connector already registered: {id}"
            )));
        }
        debug!(connector_id = %id, "registered connector");
        guard.insert(id, Arc::new(tokio::sync::Mutex::new(connector)));
        Ok(())
    }
    pub fn unregister(&self, id: &ConnectorId) -> Option<Box<dyn Connector>> {
        let mut guard = self.connectors.write();
        let removed = guard.remove(id);
        if removed.is_some() {
            debug!(connector_id = %id, "unregistered connector");
        }
        // Best-effort: try to extract the inner connector without blocking.
        // If the connector is currently in use (locked), we drop the Arc and
        // the connector is lost — acceptable for an explicit unregister.
        removed.and_then(|arc| {
            Arc::try_unwrap(arc)
                .ok()
                .map(tokio::sync::Mutex::into_inner)
        })
    }
    pub fn is_registered(&self, id: &ConnectorId) -> bool {
        self.connectors.read().contains_key(id)
    }
    pub fn len(&self) -> usize {
        self.connectors.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.connectors.read().is_empty()
    }
    pub fn connector_ids(&self) -> Vec<ConnectorId> {
        self.connectors.read().keys().cloned().collect()
    }
    pub async fn connect(
        &self,
        id: &ConnectorId,
        credentials: &CredentialStore,
    ) -> Result<(), ConnectorError> {
        let connector = {
            let guard = self.connectors.read();
            guard.get(id).cloned().ok_or_else(|| {
                ConnectorError::not_found(format!("connector not registered: {id}"))
            })?
        };
        info!(connector_id = %id, "connecting connector");
        connector
            .lock()
            .await
            .connect(credentials)
            .await
            .map_err(|e| {
                error!(connector_id = %id, error = %e, "failed to connect connector");
                e
            })?;
        debug!(connector_id = %id, "connector connected successfully");
        Ok(())
    }
    pub async fn disconnect(&self, id: &ConnectorId) -> Result<(), ConnectorError> {
        let connector = {
            let guard = self.connectors.read();
            guard.get(id).cloned().ok_or_else(|| {
                ConnectorError::not_found(format!("connector not registered: {id}"))
            })?
        };
        info!(connector_id = %id, "disconnecting connector");
        connector.lock().await.disconnect().await.map_err(|e| {
            warn!(connector_id = %id, error = %e, "error during connector disconnect");
            e
        })?;
        debug!(connector_id = %id, "connector disconnected");
        Ok(())
    }
    pub async fn health_check(&self, id: &ConnectorId) -> Result<ConnectorStatus, ConnectorError> {
        let connector = {
            let guard = self.connectors.read();
            guard.get(id).cloned().ok_or_else(|| {
                ConnectorError::not_found(format!("connector not registered: {id}"))
            })?
        };
        connector.lock().await.health_check().await
    }
    pub fn get(&self, id: &ConnectorId) -> Option<ConnectorRef<'_>> {
        let guard = self.connectors.read();
        if guard.contains_key(id) {
            Some(ConnectorRef {
                guard,
                id: id.clone(),
            })
        } else {
            None
        }
    }
    pub async fn health_check_all(&self) -> HashMap<ConnectorId, ConnectorStatus> {
        let ids: Vec<ConnectorId> = self.connector_ids();
        let mut results = HashMap::new();
        for id in &ids {
            let status = match self.health_check(id).await {
                Ok(status) => status,
                Err(e) => {
                    warn!(connector_id = %id, error = %e, "health check failed");
                    ConnectorStatus::Error(e.to_string())
                }
            };
            results.insert(id.clone(), status);
        }
        results
    }
    pub fn status_summary(&self) -> Vec<(ConnectorId, String, ConnectorStatus)> {
        let guard = self.connectors.read();
        guard
            .iter()
            .map(|(id, c)| {
                let connector = c.try_lock().ok();
                let name = connector
                    .as_ref()
                    .map_or_else(|| "<locked>".to_owned(), |c| c.name().to_owned());
                let status = connector
                    .as_ref()
                    .map_or(ConnectorStatus::Disconnected, |c| c.status().clone());
                (id.clone(), name, status)
            })
            .collect()
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// A read-locked reference into the [`ConnectorRegistry`].
///
/// Provides access to the underlying connector handle while the registry read
/// lock is held. The connector itself is behind a `tokio::sync::Mutex`, so
/// callers must `.lock().await` to invoke connector methods.
pub struct ConnectorRef<'a> {
    guard: parking_lot::RwLockReadGuard<'a, HashMap<ConnectorId, ConnectorHandle>>,
    id: ConnectorId,
}

impl ConnectorRef<'_> {
    /// Returns a handle to the connector, which can be locked to invoke
    /// connector methods.
    pub fn connector(&self) -> Option<ConnectorHandle> {
        self.guard.get(&self.id).cloned()
    }
}

impl fmt::Debug for ConnectorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorRef")
            .field("id", &self.id)
            .finish()
    }
}
