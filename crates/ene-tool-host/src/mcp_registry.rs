use crate::tools::ToolDefinition;
use crate::tools::registry::ToolRegistry;
use async_trait::async_trait;
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
    pub tools: Vec<ToolDefinition>,
}

/// Registry for MCP server connections and their tools.
pub struct McpToolRegistry {
    servers: Arc<RwLock<Vec<McpServerConnection>>>,
}

impl McpToolRegistry {
    /// Creates a new empty MCP tool registry.
    pub fn new() -> Self {
        Self {
            servers: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Connects to an MCP server via stdio transport.
    pub async fn connect_stdio(
        &self,
        name: &str,
        command: &str,
        args: &[&str],
    ) -> Result<(), String> {
        let cmd = Command::new(command).configure(|c| {
            for arg in args {
                c.arg(arg);
            }
        });

        let client = serve_client((), TokioChildProcess::new(cmd).map_err(|e| e.to_string())?)
            .await
            .map_err(|e| e.to_string())?;

        let mcp_tools_resp = client.list_tools(None).await.map_err(|e| e.to_string())?;

        let mut tools = Vec::new();
        for t in mcp_tools_resp.tools {
            tools.push(ToolDefinition {
                name: t.name.to_string(),
                description: t.description.map(|d| d.to_string()).unwrap_or_default(),
                parameters: serde_json::Value::Object(t.input_schema.as_ref().clone()),
                category: None,
                keywords: vec![],
            });
        }

        let mut servers = self.servers.write().unwrap_or_else(|e| e.into_inner());
        servers.push(McpServerConnection {
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
    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut res = Vec::new();
        let servers = self.servers.read().unwrap_or_else(|e| e.into_inner());
        for s in servers.iter() {
            res.extend(s.tools.clone());
        }
        res
    }

    async fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<String, crate::error::ToolError> {
        let client_opt = {
            let servers = self.servers.read().unwrap_or_else(|e| e.into_inner());
            let mut found = None;
            for s in servers.iter() {
                if s.tools.iter().any(|t| t.name == name) {
                    found = Some(s.client.clone());
                    break;
                }
            }
            found
        };

        let client = client_opt.ok_or_else(|| {
            crate::error::ToolError::ToolExecutionError(format!("Tool {} not found in MCP", name))
        })?;

        // Parse arguments to serde_json::Value
        let args_val: serde_json::Value = serde_json::from_str(arguments)
            .map_err(|e| crate::error::ToolError::ToolExecutionError(e.to_string()))?;

        let mut params = rmcp::model::CallToolRequestParams::new(name.to_string());
        if let Some(obj) = args_val.as_object() {
            params = params.with_arguments(obj.clone());
        }

        let result = client
            .call_tool(params)
            .await
            .map_err(|e| crate::error::ToolError::ToolExecutionError(e.to_string()))?;

        serde_json::to_string(&result.content)
            .map_err(|e| crate::error::ToolError::ToolExecutionError(e.to_string()))
    }
}
