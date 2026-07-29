//! MCP connector implementation for the connector framework.
//!
//! Wraps an [`McpToolRegistry`] instance as an [`ene_connector::Connector`],
//! enabling credential management, retry policies, and health checks through
//! the unified connector framework.

use std::sync::Arc;

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
/// shared concurrently. The internal [`McpToolRegistry`] is wrapped in an
/// `Arc` to allow sharing with the tool registry list.
pub struct McpConnector {
    /// Connector identity and unique ID.
    id: ConnectorId,
    /// Human-readable name.
    name: String,
    /// The underlying MCP registry/tool connection (shared with tool registry).
    registry: Arc<McpToolRegistry>,
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
    #[expect(
        clippy::unreachable,
        reason = "unreachable fallback: hardcoded valid ConnectorId cannot fail validation"
    )]
    pub fn new(server: McpServerConfig) -> Self {
        let id = match ConnectorId::try_new(format!("mcp.{}", server.name)) {
            Ok(id) => id,
            Err(_) => {
                // Fallback to a guaranteed-valid ID if the server name is invalid.
                // "mcp.unknown" is always valid per ConnectorId validation rules.
                ConnectorId::try_new("mcp.unknown").unwrap_or_else(|_| {
                    unreachable!("ConnectorId::try_new(\"mcp.unknown\") failed")
                })
            }
        };
        let identity = ConnectorIdentity::new(id.clone(), format!("MCP: {}", server.name))
            .with_description(format!("MCP server '{}'", server.name));

        Self {
            id,
            name: server.name.clone(),
            registry: Arc::new(McpToolRegistry::new()),
            config: server,
            status: ConnectorStatus::Disconnected,
            credentials: CredentialStore::None,
            scopes: Vec::new(),
            identity,
            connector_config: ConnectorConfig::default(),
        }
    }

    /// Returns a reference to the underlying MCP tool registry.
    pub fn registry(&self) -> Arc<McpToolRegistry> {
        Arc::clone(&self.registry)
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

        let result = match &config.transport {
            crate::mcp_config::McpTransport::Stdio { command, args } => {
                let command = command.clone();
                let args: Vec<String> = args.clone();
                let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
                let passthrough = config.env_passthrough.clone();
                self.registry
                    .connect_stdio(&name, &command, &args_ref, &passthrough)
                    .await
                    .map_err(|e| ConnectorError::transport(e.to_string()))
            }
            crate::mcp_config::McpTransport::Http {
                url,
                auth_header: config_auth,
            } => {
                // WS7 SSRF protection: validate URL scheme and refuse link-local/loopback
                let url_str = url.clone();
                if let Err(e) = validate_mcp_http_url(&url_str) {
                    return Err(ConnectorError::transport(format!(
                        "MCP HTTP URL validation failed: {e}"
                    )));
                }

                // WS7 auth unification: single explicit source, fail-closed on malformed headers
                let auth = resolve_auth_header(credentials, config_auth.as_deref())?;

                self.registry
                    .connect_http(&name, &url_str, auth.as_deref())
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

/// Validates an MCP HTTP URL for SSRF protection.
///
/// Enforces:
/// - HTTPS-only (or explicit allowlist in the future)
/// - Refuses link-local addresses (`169.254.0.0/16`, `fe80::/10`)
/// - Refuses loopback addresses (`127.0.0.0/8`, `::1`)
///
/// Returns `Ok(())` if the URL is safe, or an error message if validation fails.
fn validate_mcp_http_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

    // Enforce HTTPS-only (or explicit allowlist in the future)
    match parsed.scheme() {
        "https" => {}
        "http" => {
            return Err(
                "HTTP is not allowed for MCP servers; use HTTPS or configure an explicit allowlist"
                    .to_string(),
            );
        }
        scheme => {
            return Err(format!(
                "unsupported URL scheme '{scheme}'; only HTTPS is allowed"
            ));
        }
    }

    // Check for link-local and loopback addresses
    if let Some(host) = parsed.host_str() {
        // Try to parse as IP address
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            match ip {
                std::net::IpAddr::V4(ipv4) => {
                    // Refuse loopback (127.0.0.0/8)
                    if ipv4.is_loopback() {
                        return Err("loopback addresses (127.0.0.0/8) are not allowed".to_string());
                    }
                    // Refuse link-local (169.254.0.0/16)
                    if ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254 {
                        return Err(
                            "link-local addresses (169.254.0.0/16) are not allowed".to_string()
                        );
                    }
                }
                std::net::IpAddr::V6(ipv6) => {
                    // Refuse loopback (::1)
                    if ipv6.is_loopback() {
                        return Err("loopback address (::1) is not allowed".to_string());
                    }
                    // Refuse link-local (fe80::/10)
                    if ipv6.segments()[0] & 0xffc0 == 0xfe80 {
                        return Err("link-local addresses (fe80::/10) are not allowed".to_string());
                    }
                }
            }
        }
        // For hostnames, we could do DNS resolution and check the resolved IPs,
        // but that's expensive and can be bypassed with DNS rebinding.
        // A production implementation should use a DNS resolver that validates
        // resolved IPs, or require IP addresses only.
    }

    Ok(())
}

/// Resolves the authentication header from credentials and config.
///
/// Unifies API key and config `auth_header` into a single explicit source:
/// 1. If credentials contain an API key, use `Bearer {key}`
/// 2. Otherwise, if config provides an `auth_header`, use it
/// 3. Otherwise, return None (no auth)
///
/// Fails closed on malformed auth headers (returns an error instead of
/// warn+continue unauthenticated).
fn resolve_auth_header(
    credentials: &CredentialStore,
    config_auth: Option<&str>,
) -> Result<Option<String>, ConnectorError> {
    // Priority: credentials API key > config auth_header
    if let Some(api_key) = credentials.api_key() {
        // Validate the API key is not empty
        if api_key.trim().is_empty() {
            return Err(ConnectorError::auth(
                "API key is present but empty or whitespace-only",
            ));
        }
        return Ok(Some(format!("Bearer {api_key}")));
    }

    // Fall back to config auth_header
    if let Some(auth) = config_auth {
        // Validate the auth header is not empty
        if auth.trim().is_empty() {
            return Err(ConnectorError::auth(
                "config auth_header is present but empty or whitespace-only",
            ));
        }
        return Ok(Some(auth.to_string()));
    }

    // No auth configured
    Ok(None)
}
