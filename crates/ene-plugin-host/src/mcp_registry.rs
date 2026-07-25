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
use rmcp::serve_client;
use rmcp::transport::child_process::{ConfigureCommandExt, TokioChildProcess};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::process::Command;

/// Represents a connection to an MCP server.
pub struct McpServerConnection {
    /// The server name.
    pub name: String,
    /// The MCP client peer.
    pub client: Arc<rmcp::Peer<rmcp::RoleClient>>,
    /// Tools provided by this server.
    pub tools: Vec<ToolSpec>,
}

impl McpServerConnection {
    /// Returns `true` when the underlying transport (and thus the stdio child
    /// process) is still alive.
    ///
    /// When an MCP stdio process exits, its pipes close and rmcp marks the
    /// peer's transport as closed; this is the cheapest reliable liveness
    /// signal available without polling the OS for the child's PID.
    fn is_alive(&self) -> bool {
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
    ) -> Result<(), PluginHostError> {
        let cmd = Command::new(command).configure(|c| {
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
            });

        Ok(())
    }

    /// Connects to an MCP server via HTTP (SSE) transport.
    pub async fn connect_http(
        &self,
        name: &str,
        url: &str,
        auth_header: Option<&str>,
    ) -> Result<(), PluginHostError> {
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
                "MCP HTTP transport handshake failed for '{}': {}",
                name, e
            ))
        })?;

        let mcp_tools_resp = client.peer().list_tools(None).await.map_err(|e| {
            PluginHostError::McpRpc(format!("MCP HTTP list_tools failed for '{}': {}", name, e))
        })?;

        let mut tools = Vec::new();
        for t in mcp_tools_resp.tools {
            let desc = t.description.map(|d| d.to_string()).unwrap_or_default();
            let tool_name = ToolName::try_new(t.name.to_string()).map_err(|e| {
                PluginHostError::McpInvalidName(format!(
                    "MCP server '{}' advertised an invalid tool name: {e}",
                    name
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
            });

        Ok(())
    }

    /// Discover available tools from the MCP server.
    pub async fn discover_tools(&self) -> Result<Vec<ToolSpec>, PluginHostError> {
        let tools = self
            .servers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .flat_map(|s| s.tools.clone())
            .collect();
        Ok(tools)
    }

    /// Map an MCP tool to an Ene ToolSpec.
    pub fn mcp_tool_to_spec(server_name: &str, mcp_tool: &rmcp::model::Tool) -> ToolSpec {
        ToolSpec::new(
            ToolName::try_new(format!("mcp.{}.{}", server_name, mcp_tool.name)).unwrap_or_else(
                |_| ToolName::new(&format!("mcp.{}.{}", server_name, mcp_tool.name)),
            ),
            mcp_tool.description.clone().unwrap_or_default().to_string(),
            serde_json::to_value(&mcp_tool.input_schema).unwrap_or_default(),
        )
    }

    /// Disconnect from all MCP servers.
    pub async fn disconnect(&self) -> Result<(), PluginHostError> {
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
    /// was terminated.
    pub async fn ping(&self) -> Result<(), PluginHostError> {
        let servers = self
            .servers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if servers.is_empty() {
            return Err(PluginHostError::ExecutionFailed {
                message: "No MCP servers connected".to_string(),
            });
        }
        for s in servers.iter() {
            if !s.is_alive() {
                return Err(PluginHostError::ExecutionFailed {
                    message: format!("MCP server '{}' is not alive", s.name),
                });
            }
        }
        Ok(())
    }

    /// Removes servers whose process has died, logging a warning for each.
    ///
    /// This is the on-access circuit breaker: dead servers stop advertising
    /// their tools as soon as the liveness check observes a closed transport.
    /// Pruning requires a write lock, so callers that only hold a read lock
    /// must drop it first.
    fn prune_dead_servers(&self) {
        let mut servers = self
            .servers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = servers.len();
        servers.retain(|s| {
            let alive = s.is_alive();
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
                if s.is_alive() {
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
                    if s.is_alive() {
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

        Ok(ToolResult::from_string(content_text))
    }
}

#[cfg(test)]
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
}
