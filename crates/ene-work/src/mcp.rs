use async_trait::async_trait;
use ene_plane::Sensitivity;
use ene_registry::{ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;

/// Handwritten MCP profile row (D-23). No connection UI (P-616).
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

/// In-process MCP stand-in for tests / offline boots.
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
