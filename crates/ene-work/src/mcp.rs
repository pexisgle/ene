use async_trait::async_trait;
use ene_plane::Sensitivity;
use ene_registry::{ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Handwritten MCP server row (`mcp.json` / `mcp.<id>` fiber).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl McpServer {
    #[must_use]
    pub fn plugin_id(&self) -> String {
        format!("mcp.{}", self.id)
    }

    #[must_use]
    pub fn is_http(&self) -> bool {
        matches!(
            self.transport.as_str(),
            "http" | "sse" | "streamable_http" | "streamable-http"
        )
    }
}

/// Handwritten MCP profile row (D-23). No marketplace picker (P-616).
#[derive(Debug, Clone)]
pub struct McpProfile {
    pub server: String,
    pub transport: String,
    pub command: Option<String>,
    pub url: Option<String>,
}

/// Register MCP tools onto the same pipeline as native tools.
pub fn register_mcp_tools(
    registry: &ToolRegistry,
    profile: &McpProfile,
    tools: Vec<McpTool>,
    invoke: &Arc<dyn ToolInvoke>,
) {
    for tool in tools {
        let name = format!("mcp:{}.{}", profile.server, tool.name);
        registry.register_with(
            ToolDefinition {
                name,
                description: tool.description,
                parameters: tool.parameters,
                output: json!({ "type": "object" }),
                side_effects: tool.side_effects,
                source: ToolSource::Mcp {
                    server: profile.server.clone(),
                },
                timeout_ms: Some(30_000),
                sensitivity: Sensitivity::None,
            },
            Arc::clone(invoke),
        );
    }
}

#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub side_effects: Vec<String>,
}

/// In-process MCP stand-in for registry unit tests. Process acceptance uses
/// `ene-harness-mcp` plus a real stdio server (`plugins/harness/mcp/tests`).
#[derive(Debug, Default)]
pub struct ScriptedMcp {
    replies: parking_lot::Mutex<HashMap<String, Value>>,
}

impl ScriptedMcp {
    #[must_use]
    pub fn new(replies: impl IntoIterator<Item = (String, Value)>) -> Self {
        Self {
            replies: parking_lot::Mutex::new(replies.into_iter().collect()),
        }
    }
}

#[async_trait]
impl ToolInvoke for ScriptedMcp {
    async fn invoke(&self, name: &str, _args: Value) -> Result<Value, String> {
        self.replies
            .lock()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("mcp unavailable: {name}"))
    }
}
