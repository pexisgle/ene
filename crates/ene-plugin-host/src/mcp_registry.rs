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
use std::sync::Arc;
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
    servers: Arc<parking_lot::RwLock<Vec<McpServerConnection>>>,
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
        sandbox: Option<&crate::config::SandboxEntryConfig>,
    ) -> Result<(), PluginHostError> {
        let passthrough = env_passthrough.to_vec();
        let sandbox_spec = sandbox.filter(|config| config.enabled).and_then(|config| {
            let temp_dir = ene_config::app_data_dir()
                .join("tmp")
                .join("mcp")
                .join(name);
            std::fs::create_dir_all(&temp_dir).ok()?;
            crate::manager::build_plugin_sandbox(
                name,
                std::path::Path::new(command),
                config,
                &[],
                &temp_dir,
            )
        });
        #[cfg(not(target_os = "linux"))]
        if sandbox_spec.is_some() {
            return Err(PluginHostError::McpConnect(
                "sandboxed MCP stdio is only supported on Linux".to_string(),
            ));
        }
        let sandbox_spec = sandbox_spec.map(std::sync::Arc::new);
        let cmd = Command::new(command).configure(move |c| {
            // Harden: clear inherited environment and forward only essentials
            // via the shared helper (same whitelist as plugin spawn).
            crate::manager::apply_hardened_env(c, &passthrough);
            if let Some(spec) = sandbox_spec.as_ref() {
                if let Ok(temp_dir) = ene_config::app_data_dir()
                    .join("tmp")
                    .join("mcp")
                    .join(name)
                    .canonicalize()
                {
                    c.env("TMPDIR", temp_dir);
                }
                #[cfg(target_os = "linux")]
                {
                    // SAFETY: the closure runs in the forked child before exec
                    // and only touches process-local state (see ene-sandbox).
                    unsafe {
                        c.pre_exec(ene_sandbox::linux::pre_exec_closure((**spec).clone()));
                    }
                }
            }
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

        self.servers.write().push(McpServerConnection {
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
    /// only HTTPS is accepted and loopback addresses and the `localhost`
    /// hostname are refused; when `true` plain-HTTP and loopback/`localhost`
    /// endpoints are permitted for local development. Link-local addresses
    /// (cloud metadata) and unspecified addresses (`0.0.0.0`, `[::]`) are
    /// refused regardless.
    ///
    /// # Errors
    /// Returns [`PluginHostError::McpHandshake`] when the URL fails SSRF
    /// validation, when a configured auth header contains invalid characters
    /// (fail-closed: the connection is refused rather than downgraded to
    /// unauthenticated), or when the transport handshake / `list_tools` call
    /// fails.
    pub async fn connect_http(
        &self,
        name: &str,
        url: &str,
        auth_header: Option<&str>,
        allow_insecure_urls: bool,
    ) -> Result<(), PluginHostError> {
        validate_http_url(url, allow_insecure_urls).map_err(|reason| {
            // Log scheme/host/port only — the URL may embed userinfo
            // credentials (`https://user:token@host/sse`) that must not leak.
            let (scheme, host, port) = redacted_endpoint(url);
            tracing::warn!(
                component = "McpToolRegistry",
                server = %name,
                scheme = %scheme,
                host = %host,
                port = ?port,
                reason = %reason,
                "MCP HTTP URL rejected by SSRF validation"
            );
            PluginHostError::McpHandshake(format!("MCP HTTP URL rejected for '{name}': {reason}"))
        })?;

        let mut config = StreamableHttpClientTransportConfig::with_uri(url);

        // Convert the auth_header from config into a custom Authorization
        // header. Fail closed: a configured-but-malformed header means the
        // user intended authenticated access, so connecting unauthenticated
        // would silently downgrade their security posture. The header value is
        // a secret and is never logged.
        if let Some(auth) = auth_header {
            let value = http::HeaderValue::from_str(auth).map_err(|_| {
                PluginHostError::McpHandshake(format!(
                    "MCP HTTP auth header for '{name}' contains invalid characters; \
                     refusing to connect unauthenticated"
                ))
            })?;
            let mut custom_headers = HashMap::new();
            custom_headers.insert(http::HeaderName::from_static("authorization"), value);
            config = config.custom_headers(custom_headers);
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

        self.servers.write().push(McpServerConnection {
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
        let mut servers = self.servers.write();
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
            let guard = self.servers.read();
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
        let mut servers = self.servers.write();
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
///   and are the primary SSRF hazard. IPv4-mapped IPv6 literals
///   (`[::ffff:169.254.169.254]`) are normalized to their IPv4 form first, so
///   they cannot smuggle a link-local (or loopback) address past these checks;
///   on Linux a dual-stack connect to such an address reaches the IPv4 target.
/// - **Unspecified addresses** (`0.0.0.0`, `[::]`) are **always** rejected:
///   they are not loopback, yet on Linux connecting to them lands on localhost.
/// - **Loopback addresses** (`127.0.0.0/8`, `::1`) and the `localhost`
///   hostname (plus any `*.localhost` subdomain) are rejected by default and
///   permitted only when `allow_insecure` is `true`, so `http://127.0.0.1`
///   and `http://localhost` local servers work under the explicit opt-in.
///   (The loopback rule does not apply to arbitrary hostnames, since a hostname
///   is not an IP literal — the scheme rule still rejects `http://` for them.)
///
/// Returns `Ok(())` when the URL is acceptable, or a human-readable reason on
/// rejection (surfaced in both the error and a tracing log by the caller).
///
/// # Scope: DNS rebinding
///
/// Only IP-literal hosts (and the well-known `localhost` name) are inspected.
/// A hostname that *resolves* to an internal address (e.g. a DNS-rebinding
/// attack) is not caught here; a full defense requires validating the resolved
/// address at connect time. That is out of scope for this validation.
fn validate_http_url(url: &str, allow_insecure: bool) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;

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

    // Inspect the host for loopback / link-local / unspecified addresses.
    // Match on the parsed `url::Host` rather than `host_str()`: IP literals
    // arrive already normalized (the url crate folds decimal/octal IPv4 forms
    // such as `https://2852039166/` into `169.254.169.254`), and bracket
    // stripping is handled for us instead of via fragile `trim` munging.
    match parsed.host() {
        Some(url::Host::Domain(domain)) => {
            // `localhost` (and any `*.localhost` subdomain) resolves to the
            // loopback interface, so apply the same default-deny rule as for
            // loopback IP literals.
            let is_localhost = domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost");
            if is_localhost && !allow_insecure {
                return Err("localhost is not allowed; set \
                     plugins.mcp_allow_insecure_urls = true for local development"
                    .to_string());
            }
        }
        Some(url::Host::Ipv4(ipv4)) => {
            reject_ip(std::net::IpAddr::V4(ipv4), allow_insecure)?;
        }
        Some(url::Host::Ipv6(ipv6)) => {
            // Normalize IPv4-mapped IPv6 (`::ffff:a.b.c.d`) to its IPv4 form so
            // the IPv4 link-local / loopback / unspecified rules apply; a
            // dual-stack connect on Linux reaches the mapped IPv4 target.
            let ip = ipv6
                .to_ipv4_mapped()
                .map_or(std::net::IpAddr::V6(ipv6), std::net::IpAddr::V4);
            reject_ip(ip, allow_insecure)?;
        }
        None => {}
    }

    Ok(())
}

/// Applies the IP-literal SSRF rules shared by the IPv4 and (normalized) IPv6
/// branches of [`validate_http_url`].
///
/// Link-local addresses are always refused; unspecified and loopback addresses
/// are refused unless `allow_insecure` opts into local development.
fn reject_ip(ip: std::net::IpAddr, allow_insecure: bool) -> Result<(), String> {
    match ip {
        std::net::IpAddr::V4(ipv4) => {
            // Refuse link-local (169.254.0.0/16) — always, incl. metadata.
            if ipv4.octets()[0] == 169 && ipv4.octets()[1] == 254 {
                return Err(
                    "link-local addresses (169.254.0.0/16, incl. cloud metadata) are not allowed"
                        .to_string(),
                );
            }
            // Refuse unspecified (0.0.0.0) — always: on Linux a connect to it
            // lands on localhost, bypassing the loopback default-deny.
            if ipv4.is_unspecified() {
                return Err(
                    "unspecified address (0.0.0.0) is not allowed; it connects to localhost"
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
            // Refuse unspecified (::) — always.
            if ipv6.is_unspecified() {
                return Err(
                    "unspecified address (::) is not allowed; it connects to localhost".to_string(),
                );
            }
            // Refuse loopback (::1) unless explicitly opted in.
            if ipv6.is_loopback() && !allow_insecure {
                return Err("loopback address (::1) is not allowed; set \
                     plugins.mcp_allow_insecure_urls = true for local development"
                    .to_string());
            }
        }
    }
    Ok(())
}

/// Extracts the diagnostic-safe `(scheme, host, port)` triple from a URL for
/// logging.
///
/// MCP URLs may embed userinfo credentials (`https://user:token@host/sse`), so
/// the full URL must never be logged verbatim. This returns only the scheme,
/// host, and explicit port — enough to identify the endpoint in a diagnostic
/// without leaking the secret. An unparseable URL yields placeholder values.
pub(crate) fn redacted_endpoint(url: &str) -> (String, String, Option<u16>) {
    match url::Url::parse(url) {
        Ok(parsed) => (
            parsed.scheme().to_string(),
            parsed.host_str().unwrap_or("<no host>").to_string(),
            parsed.port(),
        ),
        Err(_) => (
            "<unparseable>".to_string(),
            "<unparseable>".to_string(),
            None,
        ),
    }
}

#[async_trait]
impl ToolRegistry for McpToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        // First pass: read-locked snapshot + liveness detection. We cannot
        // prune while holding the read lock, so collect the dead names and
        // prune in a second (write-locked) pass only when needed.
        let (tools, dead) = {
            let servers = self.servers.read();
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
            let servers = self.servers.read();
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

    /// HTTP liveness must use an authoritative `ping` RPC, not just
    /// `is_transport_closed`. This test verifies the transport-kind dispatch
    /// contract: `McpTransportKind::Http` and `McpTransportKind::Stdio` are
    /// distinct variants that drive different liveness strategies in `ping()`,
    /// so HTTP servers that keep their channel open but become unresponsive
    /// are still detected.
    #[tokio::test]
    async fn http_liveness_requires_ping_rpc_not_just_transport_check() {
        assert_ne!(McpTransportKind::Http, McpTransportKind::Stdio);

        let registry = McpToolRegistry::new();
        let result = registry.ping().await;
        assert!(result.is_err());
    }

    // ── SSRF URL validation ────────────────────────────────────────

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
    fn ipv4_mapped_ipv6_link_local_rejected_even_with_opt_in() {
        // `::ffff:169.254.169.254` is the IPv4-mapped form of the cloud
        // metadata endpoint. A dual-stack connect on Linux reaches the IPv4
        // target, so it must be normalized and rejected under both settings.
        for allow in [false, true] {
            let err = validate_http_url("https://[::ffff:169.254.169.254]/latest/meta-data", allow)
                .unwrap_err();
            assert!(err.contains("link-local"), "unexpected: {err}");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_loopback_rejected_by_default() {
        // `::ffff:127.0.0.1` is the IPv4-mapped loopback; default-deny.
        let err = validate_http_url("https://[::ffff:127.0.0.1]:8080/sse", false).unwrap_err();
        assert!(err.contains("loopback"), "unexpected: {err}");
        assert!(validate_http_url("https://[::ffff:127.0.0.1]:8080/sse", true).is_ok());
    }

    #[test]
    fn unspecified_addresses_rejected_even_with_opt_in() {
        // `0.0.0.0` and `[::]` are not loopback, yet on Linux a connect to
        // them lands on localhost — so they are refused under both settings.
        for url in ["https://0.0.0.0:8080/sse", "https://[::]:8080/sse"] {
            for allow in [false, true] {
                let err = validate_http_url(url, allow).unwrap_err();
                assert!(err.contains("unspecified"), "unexpected: {err}");
            }
        }
    }

    #[test]
    fn localhost_hostname_rejected_by_default_allowed_with_opt_in() {
        // `localhost` resolves to loopback, so it follows the loopback
        // default-deny rule — including `*.localhost` subdomains.
        for url in [
            "https://localhost:8080/sse",
            "https://foo.localhost:8080/sse",
        ] {
            let err = validate_http_url(url, false).unwrap_err();
            assert!(err.contains("localhost"), "unexpected: {err}");
            assert!(validate_http_url(url, true).is_ok());
        }
    }

    #[test]
    fn public_hostname_is_not_treated_as_localhost() {
        // The localhost rule must not over-match unrelated hostnames.
        assert!(validate_http_url("https://notlocalhost.example.com/sse", false).is_ok());
        assert!(validate_http_url("https://example.localhost.evil.com/sse", false).is_ok());
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

    /// A configured auth header with invalid characters must fail closed
    /// (refuse to connect) rather than silently downgrading to an
    /// unauthenticated connection. This is checked before any network I/O.
    #[tokio::test]
    async fn connect_http_fails_closed_on_invalid_auth_header() {
        let registry = McpToolRegistry::new();
        // A valid HTTPS URL (passes SSRF validation) but a header value with a
        // control character, which http::HeaderValue::from_str rejects.
        let result = registry
            .connect_http(
                "secure",
                "https://mcp.example.com/sse",
                Some("Bearer bad\nvalue"),
                false,
            )
            .await;
        let err = result.unwrap_err();
        assert!(
            matches!(err, PluginHostError::McpHandshake(_)),
            "unexpected variant: {err:?}"
        );
        let msg = err.to_string();
        assert!(msg.contains("invalid characters"), "unexpected: {msg}");
        assert!(msg.contains("secure"), "server name missing: {msg}");
    }
}
