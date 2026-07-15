use crate::ToolHostError;
use crate::tools::registry::ToolRegistry;
use async_trait::async_trait;
use ene_tool_proto::{ToolName, ToolSpec};
use rmcp::serve_client;
use rmcp::transport::child_process::{ConfigureCommandExt, TokioChildProcess};
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

/// Registry for MCP server connections and their tools.
#[derive(Default)]
pub struct McpToolRegistry {
    servers: Arc<RwLock<Vec<McpServerConnection>>>,
}

impl McpToolRegistry {
    /// Creates a new empty MCP tool registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Connects to an MCP server via stdio transport.
    pub async fn connect_stdio(
        &self,
        name: &str,
        command: &str,
        args: &[&str],
    ) -> Result<(), ToolHostError> {
        let cmd = Command::new(command).configure(|c| {
            for arg in args {
                c.arg(arg);
            }
        });

        let client = serve_client(
            (),
            TokioChildProcess::new(cmd).map_err(|e| ToolHostError::McpConnect(e.to_string()))?,
        )
        .await
        .map_err(|e| ToolHostError::McpHandshake(e.to_string()))?;

        let mcp_tools_resp = client
            .list_tools(None)
            .await
            .map_err(|e| ToolHostError::McpRpc(e.to_string()))?;

        let mut tools = Vec::new();
        for t in mcp_tools_resp.tools {
            let desc = t.description.map(|d| d.to_string()).unwrap_or_default();
            let name = ToolName::try_new(t.name.to_string()).map_err(|e| {
                ToolHostError::McpInvalidName(format!(
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

    // HTTP transport connection can be added similarly
}

#[async_trait]
impl ToolRegistry for McpToolRegistry {
    fn list_tools(&self) -> Vec<ToolSpec> {
        let mut res = Vec::new();
        let servers = self
            .servers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for s in servers.iter() {
            res.extend(s.tools.clone());
        }
        drop(servers);
        res
    }

    async fn call_tool(&self, name: &str, arguments: &str) -> Result<String, ToolHostError> {
        let client_opt = {
            let servers = self
                .servers
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut found = None;
            for s in servers.iter() {
                if s.tools.iter().any(|t| t.name.as_str() == name) {
                    found = Some(s.client.clone());
                    break;
                }
            }
            drop(servers);
            found
        };

        let client = client_opt.ok_or_else(|| ToolHostError::ExecutionFailed {
            message: format!("Tool {name} not found in MCP"),
        })?;

        // Parse arguments to serde_json::Value
        let args_val: serde_json::Value =
            serde_json::from_str(arguments).map_err(|e| ToolHostError::ExecutionFailed {
                message: e.to_string(),
            })?;

        let mut params = rmcp::model::CallToolRequestParams::new(name.to_string());
        if let Some(obj) = args_val.as_object() {
            params = params.with_arguments(obj.clone());
        }

        let result = client
            .call_tool(params)
            .await
            .map_err(|e| ToolHostError::McpRpc(e.to_string()))?;

        serde_json::to_string(&result.content).map_err(|e| ToolHostError::ExecutionFailed {
            message: e.to_string(),
        })
    }
}
