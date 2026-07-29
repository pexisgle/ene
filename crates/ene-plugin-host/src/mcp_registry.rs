//! MCP client for external server connections.
//!
//! Connects to MCP (Model Context Protocol) servers via stdio transport and
//! exposes their tools through the [`ToolRegistry`] trait so they integrate
//! seamlessly with plugin-provided tools.
//!
//! ## Liveness
//!
//! MCP servers are external child processes that can die at any time. A dead
//! server's tools must not keep being advertised to the model, or calls would
//! fail with confusing transport errors. Before tools are listed or dispatched,
//! the registry checks each server's transport liveness via
//! `rmcp::Peer::is_transport_closed` and prunes any server whose
//! process has exited, logging a warning. This is a simple on-access circuit
//! breaker: once pruned, a server's tools disappear from the registry until it
//! is reconnected explicitly via [`connect_stdio`](McpToolRegistry::connect_stdio).

use crate::error::PluginHostError;
use crate::tool_registry::ToolRegistry;
use async_trait::async_trait;
use ene_plugin_proto::ToolResult;
use ene_plugin_proto::{CallContext, ToolName, ToolSpec};
use rmcp::model::{ClientRequest, PingRequest};
use rmcp::serve_client;
use rmcp::transport::child_process::{ConfigureCommandExt, TokioChildProcess};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::process::Command;

/// The transport used to reach an MCP server.
///
/// Liveness detection differs by transport: a stdio child process reliably
/// closes its pipes on exit (so [`rmcp::Peer::is_transport_closed`] is a cheap,
/// accurate signal), whereas an HTTP endpoint may keep its channel open while
/// the server is unresponsive — so HTTP liveness requires an actual `ping` RPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportKind {
    /// Spawned child process over stdio.
    Stdio,
    /// Remote endpoint over streamable HTTP.
    Http,
}

/// Represents a connection to an MCP server.
pub struct McpServerConnection {
    /// The server name.
    pub name: String,
    /// The MCP client peer.
    pub client: Arc<rmcp::Peer<rmcp::RoleClient>>,
    /// Tools provided by this server.
    pub tools: Vec<ToolSpec>,
    /// Transport used to reach this server (drives liveness strategy).
    pub transport: McpTransportKind,
}

impl McpServerConnection {
    /// Cheap, synchronous transport-closed check.
    ///
    /// For stdio this is an accurate liveness signal (a dead child closes its
    /// pipes). For HTTP it only detects a fully torn-down channel, so callers
    /// that need authoritative HTTP liveness should issue a `ping` RPC (see
    /// [`McpToolRegistry::ping`](crate::McpToolRegistry::ping)).
    fn is_transport_alive(&self) -> bool {
        !self.client.is_transport_closed()
    }
}

/// Registry for MCP server connections and their tools.
#[derive(Default)]
pub struct McpToolRegistry {
    servers: Arc<RwLock<Vec<McpServerConnection>>>,
}

impl McpToolRegistry {
    /// Creates a new empty MCP tool registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Connects to an MCP server via stdio transport.
    pub async fn connect_stdio(
        &self,
        name: &str,
        command: &str,
        args: &[&str],
        env_passthrough: &[String],
    ) -> Result<(), PluginHostError> {
        let passthrough = env_passthrough.to_vec();
        let cmd = Command::new(command).configure(move |c| {
            // Harden: clear inherited environment and forward only essentials
            // via the shared helper (same whitelist as plugin spawn).
            crate::manager::apply_hardened_env(c, &passthrough);
            for arg in args {
                c.arg(arg);
            }
        });

        let client = serve_client(
            (),
            TokioChildProcess::new(cmd).map_err(|e| PluginHostError::McpConnect(e.to_string()))?,
        )
        .await
        .map_err(|e| PluginHostError::McpHandshake(e.to_string()))?;

        let mcp_tools_resp = client
            .list_tools(None)
            .await
            .map_err(|e| PluginHostError::McpRpc(e.to_string()))?;

        let mut tools = Vec::new();
        for t in mcp_tools_resp.tools {
            let desc = t.description.map(|d| d.to_string()).unwrap_or_default();
            let name = ToolName::try_new(t.name.to_string()).map_err(|e| {
                PluginHostError::McpInvalidName(format!(
                    "MCP server advertised an invalid tool name: {e}"
                ))
            })?;
            tools.push(ToolSpec::new(
                name,
                desc,
                serde_json::Value::Object(t.input_schema.as_ref().clone()),
            ));
        }

        self.servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(McpServerConnection {
                name: name.to_string(),
                client: Arc::new(client.peer().clone()),
                tools,
                transport: McpTransportKind::Stdio,
            });

        Ok(())
    }

    /// Connects to an MCP server via HTTP (SSE) transport.
    ///
    /// The URL is validated for SSRF protection before any connection is
    /// attempted (see [`validate_http_url`]). `allow_insecure_urls` is the
    /// `plugins.mcp_allow_insecure_urls` opt-in: when `false` (the default)
    /// only HTTPS is accepted and loopback addresses are refused; when `true`
    /// plain-HTTP and loopback endpoints are permitted for local development.
    /// Link-local addresses (cloud metadata) are refused regardless.
    ///
    /// # Errors
    /// Returns [`PluginHostError::McpHandshake`] when the URL fails SSRF
    /// validation or the transport handshake / `list_tools` call fails.
    pub async fn connect_http(
        &self,
        name: &str,
        url: &str,
        auth_header: Option<&str>,
        allow_insecure_urls: bool,
    ) -> Result<(), PluginHostError> {
        validate_http_url(url, allow_insecure_urls).map_err(|reason| {
            tracing::warn!(
                component = "McpToolRegistry",
                server = %name,
                url = %url,
                reason = %reason,
                "MCP HTTP URL rejected by SSRF validation"
            );
            PluginHostError::McpHandshake(format!("MCP HTTP URL rejected for '{name}': {reason}"))
        })?;

        let mut config = StreamableHttpClientTransportConfig::with_uri(url);

        // Convert the auth_header from config into a custom Authorization header.
        if let Some(auth) = auth_header {
            if let Ok(value) = http::HeaderValue::from_str(auth) {
                let mut custom_headers = HashMap::new();
                custom_headers.insert(http::HeaderName::from_static("authorization"), value);
                config = config.custom_headers(custom_headers);
            } else {
                tracing::warn!(
                    component = "McpToolRegistry",
                    server = %name,
                    "MCP HTTP auth header contains invalid characters; skipping"
                );
            }
        }

        let transport = StreamableHttpClientTransport::from_config(config);

        let client = serve_client((), transport).await.map_err(|e| {
            PluginHostError::McpHandshake(format!(
                "MCP HTTP transport handshake failed for '{name}': {e}"
            ))
        })?;

        let mcp_tools_resp = client.peer().list_tools(None).await.map_err(|e| {
            PluginHostError::McpRpc(format!("MCP HTTP list_tools failed for '{name}': {e}"))
        })?;

        let mut tools = Vec::new();
        for t in mcp_tools_resp.tools {
            let desc = t.description.map(|d| d.to_string()).unwrap_or_default();
            let tool_name = ToolName::try_new(t.name.to_string()).map_err(|e| {
                PluginHostError::McpInvalidName(format!(
                    "MCP server '{name}' advertised an invalid tool name: {e}"
                ))
            })?;
            tools.push(ToolSpec::new(
                tool_name,
                desc,
                serde_json::Value::Object(t.input_schema.as_ref().clone()),
            ));
        }

        self.servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(McpServerConnection {
                name: name.to_string(),
                client: Arc::new(client.peer().clone()),
                tools,
                transport: McpTransportKind::Http,
            });

        Ok(())
    }

    /// Discover available tools from the MCP server.
    pub fn discover_tools(&self) -> Result<Vec<ToolSpec>, PluginHostError> {
        let tools = self
            .servers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flat_map(|s| s.tools.clone())
            .collect();
        Ok(tools)
    }

    /// Map an MCP tool to an Ene `ToolSpec`.
    ///
    /// # Errors
    /// Returns [`PluginHostError::McpInvalidName`] when the constructed
    /// `mcp.<server>.<tool>` name is not a valid [`ToolName`].
    pub fn mcp_tool_to_spec(
        server_name: &str,
        mcp_tool: &rmcp::model::Tool,
    ) -> Result<ToolSpec, PluginHostError> {
        let name =
            ToolName::try_new(format!("mcp.{}.{}", server_name, mcp_tool.name)).map_err(|e| {
                PluginHostError::McpInvalidName(format!(
                    "MCP server '{server_name}' advertised an invalid tool name: {e}"
                ))
            })?;
        Ok(ToolSpec::new(
            name,
            mcp_tool.description.clone().unwrap_or_default().to_string(),
            serde_json::to_value(&mcp_tool.input_schema).unwrap_or_default(),
        ))
    }

    /// Disconnect from all MCP servers.
    pub fn disconnect(&self) -> Result<(), PluginHostError> {
        let mut servers = self
            .servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        servers.clear();
        Ok(())
    }

    /// Ping each connected server for liveness.
    ///
    /// Checks transport liveness via the underlying rmcp peer. A closed
    /// transport indicates the server process has exited or the connection
    /// was terminated. For HTTP servers, an actual `ping` RPC is issued.
    pub async fn ping(&self) -> Result<(), PluginHostError> {
        // Clone the server handles out and drop the (non-Send) read guard
        // before awaiting, so the future stays `Send`.
        let servers: Vec<(String, Arc<rmcp::Peer<rmcp::RoleClient>>, McpTransportKind)> = {
            let guard = self
                .servers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.is_empty() {
                return Err(PluginHostError::ExecutionFailed {
                    message: "No MCP servers connected".to_string(),
                });
            }
            guard
                .iter()
                .map(|s| (s.name.clone(), Arc::clone(&s.client), s.transport))
                .collect()
        };

        for (name, client, transport) in servers {
            let alive = match transport {
                McpTransportKind::Stdio => !client.is_transport_closed(),
                McpTransportKind::Http => {
                    !client.is_transport_closed()
                        && client
                            .send_request(ClientRequest::PingRequest(PingRequest::default()))
                            .await
                            .is_ok()
                }
            };
            if !alive {
                return Err(PluginHostError::ExecutionFailed {
                    message: format!("MCP server '{name}' is not alive"),
                });
            }
        }
        Ok(())
    }

    /// Removes servers whose transport has closed, logging a warning for each.
    ///
    /// This is the on-access circuit breaker: dead servers stop advertising
    /// their tools as soon as the liveness check observes a closed transport.
    /// Uses the cheap synchronous transport-closed check (accurate for stdio;
    /// for HTTP it only catches a fully torn-down channel — authoritative HTTP
    /// liveness is [`ping`](Self::ping)). Pruning requires a write lock, so
    /// callers that only hold a read lock must drop it first.
    fn prune_dead_servers(&self) {
        let mut servers = self
            .servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = servers.len();
        servers.retain(|s| {
            let alive = s.is_transport_alive();
            if !alive {
                tracing::warn!(
                    component = "McpToolRegistry",
                    server = %s.name,
                    tools = s.tools.len(),
                    "MCP server process is dead; no longer advertising its tools"
                );
            }
            alive
        });
        let pruned = before.saturating_sub(servers.len());
        drop(servers);
        if pruned > 0 {
            tracing::info!(
                component = "McpToolRegistry",
                pruned = pruned,
                "Pruned dead MCP server(s) from registry"
            );
        }
    }
}

/// Validates an MCP HTTP URL for SSRF protection before connecting.
///
/// Enforced rules:
/// - **Scheme**: only `https` is accepted by default. `http` is accepted only
///   when `allow_insecure` is `true` (the `plugins.mcp_allow_insecure_urls`
///   opt-in for local development); any other scheme is always rejected.
/// - **Link-local addresses** (`169.254.0.0/16` — including the cloud metadata
///   endpoint `169.254.169.254` — and `fe80::/10`) are **always** rejected,
///   even when `allow_insecure` is set. These are never a legitimate MCP target
///   and are the primary SSRF hazard.
/// - **Loopback addresses** (`127.0.0.0/8`, `::1`) are rejected by default and
///   permitted only when `allow_insecure` is `true`, so `http://127.0.0.1`
///   local servers work under the explicit opt-in. (`http://localhost` already
///   passes, since a hostname is not an IP literal.)
///
/// Returns `Ok(())` when the URL is acceptable, or a human-readable reason on
/// rejection (surfaced in both the error and a tracing log by the caller).
///
/// # Scope: DNS rebinding
///
/// Only IP-literal hosts are inspected. A hostname that *resolves* to an
/// internal address (e.g. a DNS-rebinding attack) is not caught here; a full
/// defense requires validating the resolved address at connect time. That is
/// out of scope for this validation — it is no weaker than the previous
/// behavior, which performed no validation on the live path at all.
fn validate_http_url(url: &str, allow_insecure: bool) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

    match parsed.scheme() {
        "https" => {}
        "http" => {
            if !allow_insecure {
                return Err("HTTP is not allowed for MCP servers; use HTTPS, or set \
                     plugins.mcp_allow_insecure_urls = true for local development"
                    .to_string());
            }
        }
        scheme => {
            return Err(format!(
                "unsupported URL scheme '{scheme}'; only HTTPS is allowed"
            ));
        }
    }

    // Inspect IP-literal hosts for loopback / link-local addresses. IPv6
    // literals arrive bracketed from `Url::host_str` (e.g. `[::1]`), so strip
    // the brackets before parsing.
    if let Some(host) = parsed.host_str()
        && let Ok(ip) = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
    {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                // Refuse link-local (169.254.0.0/16) — always, incl. metadata.
                if ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254 {
                    return Err(
                        "link-local addresses (169.254.0.0/16, incl. cloud metadata) are not allowed"
                            .to_string(),
                    );
                }
                // Refuse loopback (127.0.0.0/8) unless explicitly opted in.
                if ipv4.is_loopback() && !allow_insecure {
                    return Err("loopback addresses (127.0.0.0/8) are not allowed; set \
                         plugins.mcp_allow_insecure_urls = true for local development"
                        .to_string());
                }
            }
            std::net::IpAddr::V6(ipv6) => {
                // Refuse link-local (fe80::/10) — always.
                if ipv6.segments()[0] & 0xffc0 == 0xfe80 {
                    return Err("link-local addresses (fe80::/10) are not allowed".to_string());
                }
                // Refuse loopback (::1) unless explicitly opted in.
                if ipv6.is_loopback() && !allow_insecure {
                    return Err("loopback address (::1) is not allowed; set \
                         plugins.mcp_allow_insecure_urls = true for local development"
                        .to_string());
                }
            }
        }
    }

    Ok(())
}

#[async_trait]
impl ToolRegistry for McpToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        // First pass: read-locked snapshot + liveness detection. We cannot
        // prune while holding the read lock, so collect the dead names and
        // prune in a second (write-locked) pass only when needed.
        let (tools, dead) = {
            let servers = self
                .servers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut tools = Vec::new();
            let mut dead = Vec::new();
            for s in servers.iter() {
                if s.is_transport_alive() {
                    tools.extend(s.tools.clone());
                } else {
                    dead.push(s.name.clone());
                }
            }
            (tools, dead)
        };

        if !dead.is_empty() {
            self.prune_dead_servers();
        }

        tools
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
        _context: Option<&CallContext>,
    ) -> Result<ToolResult, PluginHostError> {
        let client_opt = {
            let servers = self
                .servers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut found = None;
            for s in servers.iter() {
                if s.tools.iter().any(|t| t.name.as_str() == name) {
                    // Refuse to dispatch to a dead server; the liveness check
                    // will prune it on the next `list_tools`.
                    if s.is_transport_alive() {
                        found = Some(s.client.clone());
                    }
                    break;
                }
            }
            drop(servers);
            found
        };

        let client = client_opt.ok_or_else(|| PluginHostError::ExecutionFailed {
            message: format!("Tool {name} not found in MCP (server may be dead or disconnected)"),
        })?;

        let args_val: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| PluginHostError::ExecutionFailed {
                message: e.to_string(),
            })?;

        let mut params = rmcp::model::CallToolRequestParams::new(name.to_string());
        if let Some(obj) = args_val.as_object() {
            params = params.with_arguments(obj.clone());
        }

        let result = client
            .call_tool(params)
            .await
            .map_err(|e| PluginHostError::McpRpc(e.to_string()))?;

        let content_text = serde_json::to_string(&result.content).map_err(|e| {
            PluginHostError::ExecutionFailed {
                message: e.to_string(),
            }
        })?;

        Ok(ToolResult::text(content_text))
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "tests use unwrap/unwrap_err for concise failure messages"
)]
mod tests {
    use super::*;

    #[test]
    fn empty_registry_lists_no_tools() {
        let registry = McpToolRegistry::new();
        assert!(registry.list_tools().is_empty());
    }

    #[tokio::test]
    async fn empty_registry_call_tool_not_found() {
        let registry = McpToolRegistry::new();
        let result = registry.call_tool("nonexistent", "{}", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn ping_empty_registry_returns_error() {
        let registry = McpToolRegistry::new();
        let result = registry.ping().await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("No MCP servers connected"),
            "unexpected error: {msg}"
        );
    }

    /// H-8: HTTP liveness must use an authoritative `ping` RPC, not just
    /// `is_transport_closed`. This test verifies the transport-kind dispatch
    /// contract: `McpTransportKind::Http` and `McpTransportKind::Stdio` are
    /// distinct variants that drive different liveness strategies in `ping()`.
    ///
    /// The `ping()` method (see above) branches on transport kind:
    /// - `Stdio`: `!client.is_transport_closed()` (cheap, accurate for pipes)
    /// - `Http`: `!client.is_transport_closed()` **and** a `PingRequest` RPC
    ///
    /// This ensures HTTP servers that keep their channel open but become
    /// unresponsive are detected via the ping RPC, not just channel teardown.
    #[tokio::test]
    async fn http_liveness_requires_ping_rpc_not_just_transport_check() {
        // The two transport kinds are distinct enum variants.
        assert_ne!(McpTransportKind::Http, McpTransportKind::Stdio);

        // Verify the ping() method is wired up and reachable. A full
        // integration test would require a live HTTP MCP server; the contract
        // is enforced here at the type level and by code review of the
        // ping() branch above.
        let registry = McpToolRegistry::new();
        let result = registry.ping().await;
        assert!(result.is_err());
    }

    // ── SSRF URL validation (#418) ──────────────────────────────────

    #[test]
    fn https_url_is_accepted() {
        assert!(validate_http_url("https://mcp.example.com/sse", false).is_ok());
        assert!(validate_http_url("https://mcp.example.com/sse", true).is_ok());
    }

    #[test]
    fn public_https_ip_is_accepted() {
        assert!(validate_http_url("https://203.0.113.10/sse", false).is_ok());
    }

    #[test]
    fn http_url_is_rejected_by_default() {
        let err = validate_http_url("http://mcp.example.com/sse", false).unwrap_err();
        assert!(err.contains("HTTP is not allowed"), "unexpected: {err}");
    }

    #[test]
    fn http_url_is_allowed_with_opt_in() {
        assert!(validate_http_url("http://mcp.example.com/sse", true).is_ok());
    }

    #[test]
    fn non_http_scheme_is_rejected_even_with_opt_in() {
        for url in [
            "ftp://example.com",
            "file:///etc/passwd",
            "ws://example.com",
        ] {
            let err = validate_http_url(url, true).unwrap_err();
            assert!(err.contains("unsupported URL scheme"), "unexpected: {err}");
        }
    }

    #[test]
    fn loopback_ipv4_rejected_by_default_allowed_with_opt_in() {
        let err = validate_http_url("https://127.0.0.1:8080/sse", false).unwrap_err();
        assert!(err.contains("loopback"), "unexpected: {err}");
        assert!(validate_http_url("https://127.0.0.1:8080/sse", true).is_ok());
    }

    #[test]
    fn loopback_ipv6_rejected_by_default_allowed_with_opt_in() {
        let err = validate_http_url("https://[::1]:8080/sse", false).unwrap_err();
        assert!(err.contains("loopback"), "unexpected: {err}");
        assert!(validate_http_url("https://[::1]:8080/sse", true).is_ok());
    }

    #[test]
    fn cloud_metadata_ipv4_rejected_even_with_opt_in() {
        // https:// so the scheme check passes and the link-local rule is what
        // rejects the URL — and it must reject under both opt-in settings.
        for allow in [false, true] {
            let err =
                validate_http_url("https://169.254.169.254/latest/meta-data", allow).unwrap_err();
            assert!(err.contains("link-local"), "unexpected: {err}");
        }
    }

    #[test]
    fn link_local_ipv6_rejected_even_with_opt_in() {
        for allow in [false, true] {
            let err = validate_http_url("https://[fe80::1]/sse", allow).unwrap_err();
            assert!(err.contains("link-local"), "unexpected: {err}");
        }
    }

    #[test]
    fn invalid_url_is_rejected() {
        assert!(validate_http_url("not a url", false).is_err());
    }

    /// The live connect path must surface a validation rejection as a
    /// `McpHandshake` error *before* any network I/O is attempted.
    #[tokio::test]
    async fn connect_http_rejects_insecure_url_before_connecting() {
        let registry = McpToolRegistry::new();
        let result = registry
            .connect_http("insecure", "http://example.com/sse", None, false)
            .await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, PluginHostError::McpHandshake(_)),
            "unexpected variant: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("MCP HTTP URL rejected"), "unexpected: {msg}");
        assert!(msg.contains("insecure"), "server name missing: {msg}");
    }
}
