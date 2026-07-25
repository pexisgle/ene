//! MCP connector implementation for the connector framework.
//!
//! Wraps an [`McpToolRegistry`] instance as an [`ene_connector::Connector`],
//! enabling credential management, retry policies, and health checks through
//! the unified connector framework.

use async_trait::async_trait;

use ene_connector::{
    Connector, ConnectorConfig, ConnectorError, ConnectorId, ConnectorIdentity, ConnectorStatus,
    CredentialStore, PermissionScope,
};

use crate::mcp_config::McpServerConfig;
use crate::mcp_registry::McpToolRegistry;

/// A connector that wraps an MCP server connection.
///
/// Manages lifecycle (connect/disconnect), health checks, and credential
/// management for an external MCP server through the ene-connector framework.
///
/// # Thread safety
///
/// The [`Connector`] trait methods take `&mut self`, so this type is not
/// shared concurrently. The internal [`McpToolRegistry`] is therefore used
/// directly without additional synchronization.
pub struct McpConnector {
    /// Connector identity and unique ID.
    id: ConnectorId,
    /// Human-readable name.
    name: String,
    /// The underlying MCP registry/tool connection.
    registry: McpToolRegistry,
    /// Server configuration (transport, URL, etc.).
    config: McpServerConfig,
    /// The current connection status.
    status: ConnectorStatus,
    /// Credential store for this connector.
    credentials: CredentialStore,
    /// Permission scopes required by this connector.
    scopes: Vec<PermissionScope>,
    /// Connector identity metadata.
    identity: ConnectorIdentity,
    /// Connector configuration (timeouts, retries).
    connector_config: ConnectorConfig,
}

impl McpConnector {
    /// Creates a new MCP connector from a server configuration.
    pub fn new(server: McpServerConfig) -> Self {
        let id = ConnectorId::try_new(format!("mcp.{}", server.name))
            .unwrap_or_else(|_| ConnectorId::try_new("mcp.unknown").unwrap());
        let identity = ConnectorIdentity::new(id.clone(), format!("MCP: {}", server.name))
            .with_description(format!("MCP server '{}'", server.name));

        Self {
            id,
            name: server.name.clone(),
            registry: McpToolRegistry::new(),
            config: server,
            status: ConnectorStatus::Disconnected,
            credentials: CredentialStore::None,
            scopes: Vec::new(),
            identity,
            connector_config: ConnectorConfig::default(),
        }
    }

    /// Returns a reference to the underlying MCP tool registry.
    pub fn registry(&self) -> &McpToolRegistry {
        &self.registry
    }

    /// Returns a mutable reference to the underlying MCP tool registry.
    pub fn registry_mut(&mut self) -> &mut McpToolRegistry {
        &mut self.registry
    }

    /// Returns the server configuration.
    pub fn server_config(&self) -> &McpServerConfig {
        &self.config
    }
}

#[async_trait]
impl Connector for McpConnector {
    fn id(&self) -> &ConnectorId {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn identity(&self) -> &ConnectorIdentity {
        &self.identity
    }

    fn config(&self) -> &ConnectorConfig {
        &self.connector_config
    }

    fn status(&self) -> &ConnectorStatus {
        &self.status
    }

    fn credentials(&self) -> &CredentialStore {
        &self.credentials
    }

    fn scopes(&self) -> &[PermissionScope] {
        &self.scopes
    }

    async fn connect(&mut self, credentials: &CredentialStore) -> Result<(), ConnectorError> {
        let name = self.name.clone();
        let config = self.config.clone();

        // Store credentials
        self.credentials = credentials.clone();

        // Extract auth header from credentials if present
        let auth_header = credentials.api_key().map(|k| format!("Bearer {}", k));

        let result = match &config.transport {
            crate::mcp_config::McpTransport::Stdio { command, args } => {
                let command = command.clone();
                let args: Vec<String> = args.clone();
                let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
                self.registry
                    .connect_stdio(&name, &command, &args_ref)
                    .await
                    .map_err(|e| ConnectorError::transport(e.to_string()))
            }
            crate::mcp_config::McpTransport::Http {
                url,
                auth_header: config_auth,
            } => {
                let url = url.clone();
                let auth = auth_header.or_else(|| config_auth.as_deref().map(String::from));
                self.registry
                    .connect_http(&name, &url, auth.as_deref())
                    .await
                    .map_err(|e| ConnectorError::transport(e.to_string()))
            }
        };

        match &result {
            Ok(()) => {
                self.status = ConnectorStatus::Connected;
            }
            Err(e) => {
                self.status = ConnectorStatus::Error(e.to_string());
            }
        }

        result
    }

    async fn disconnect(&mut self) -> Result<(), ConnectorError> {
        self.registry
            .disconnect()
            .await
            .map_err(|e| ConnectorError::transport(e.to_string()))?;
        self.status = ConnectorStatus::Disconnected;
        self.credentials = CredentialStore::None;
        Ok(())
    }

    async fn health_check(&self) -> Result<ConnectorStatus, ConnectorError> {
        if !self.status.is_connected() {
            return Ok(ConnectorStatus::Disconnected);
        }
        match self.registry.ping().await {
            Ok(()) => Ok(ConnectorStatus::Connected),
            Err(e) => Ok(ConnectorStatus::Error(e.to_string())),
        }
    }

    fn rate_limiter(&self) -> Option<&ene_connector::RateLimiter> {
        None
    }
}
