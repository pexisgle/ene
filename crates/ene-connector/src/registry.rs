use crate::connector::{Connector, ConnectorId, ConnectorStatus};
use crate::credential::CredentialStore;
use crate::error::ConnectorError;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt;
use tracing::{debug, error, info, warn};

pub struct ConnectorRegistry {
    connectors: RwLock<HashMap<ConnectorId, Box<dyn Connector>>>,
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
        guard.insert(id, connector);
        Ok(())
    }
    pub fn unregister(&self, id: &ConnectorId) -> Option<Box<dyn Connector>> {
        let mut guard = self.connectors.write();
        let removed = guard.remove(id);
        if removed.is_some() {
            debug!(connector_id = %id, "unregistered connector");
        }
        removed
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
    #[expect(clippy::await_holding_lock)]
    pub async fn connect(
        &self,
        id: &ConnectorId,
        credentials: &CredentialStore,
    ) -> Result<(), ConnectorError> {
        let mut guard = self.connectors.write();
        let connector = guard
            .get_mut(id)
            .ok_or_else(|| ConnectorError::not_found(format!("connector not registered: {id}")))?;
        info!(connector_id = %id, "connecting connector");
        connector.connect(credentials).await.map_err(|e| {
            error!(connector_id = %id, error = %e, "failed to connect connector");
            e
        })?;
        debug!(connector_id = %id, "connector connected successfully");
        Ok(())
    }
    #[expect(clippy::await_holding_lock)]
    pub async fn disconnect(&self, id: &ConnectorId) -> Result<(), ConnectorError> {
        let mut guard = self.connectors.write();
        let connector = guard
            .get_mut(id)
            .ok_or_else(|| ConnectorError::not_found(format!("connector not registered: {id}")))?;
        info!(connector_id = %id, "disconnecting connector");
        connector.disconnect().await.map_err(|e| {
            warn!(connector_id = %id, error = %e, "error during connector disconnect");
            e
        })?;
        debug!(connector_id = %id, "connector disconnected");
        Ok(())
    }
    #[expect(clippy::await_holding_lock)]
    pub async fn health_check(&self, id: &ConnectorId) -> Result<ConnectorStatus, ConnectorError> {
        let guard = self.connectors.read();
        let connector = guard
            .get(id)
            .ok_or_else(|| ConnectorError::not_found(format!("connector not registered: {id}")))?;
        connector.health_check().await
    }
    pub fn get(&self, id: &ConnectorId) -> Option<ConnectorRef<'_>> {
        let guard = self.connectors.read();
        if guard.contains_key(id) {
            Some(ConnectorRef { _guard: guard })
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
            .map(|(id, c)| (id.clone(), c.name().to_owned(), c.status().clone()))
            .collect()
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ConnectorRef<'a> {
    _guard: parking_lot::RwLockReadGuard<'a, HashMap<ConnectorId, Box<dyn Connector>>>,
}
impl fmt::Debug for ConnectorRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectorRef").finish()
    }
}
