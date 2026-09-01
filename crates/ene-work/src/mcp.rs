use async_trait::async_trait;
use ene_access_control::Sensitivity;
use ene_tool_registry::{ToolDefinition, ToolInvoke, ToolRegistry, ToolSource};
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

/// Auth method a catalog entry needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCatalogAuth {
    None,
    ApiKeyHeader,
    Oauth2Remote,
}

/// One curated MCP server in the allowlisted metadata table (P-616).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub transport: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub auth: McpCatalogAuth,
    /// Human-readable side-effect summary shown before the user connects.
    pub side_effects: Vec<String>,
    pub source_url: String,
}

/// Curated server metadata (v1 static allowlist; distribution and signing
/// are a later design). Entries must point at official upstream sources.
pub fn mcp_catalog() -> &'static [McpCatalogEntry] {
    static CATALOG: std::sync::OnceLock<Vec<McpCatalogEntry>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        vec![
            McpCatalogEntry {
                id: "git".to_owned(),
                label: "Git".to_owned(),
                description: "Read-only git repository queries through mcp-server-git.".to_owned(),
                transport: "stdio".to_owned(),
                command: Some("uvx".to_owned()),
                args: vec!["mcp-server-git".to_owned()],
                url: None,
                auth: McpCatalogAuth::None,
                side_effects: vec![
                    "reads local repositories".to_owned(),
                    "spawns uvx subprocess".to_owned(),
                ],
                source_url: "https://github.com/modelcontextprotocol/servers/tree/main/src/git"
                    .to_owned(),
            },
            McpCatalogEntry {
                id: "fetch".to_owned(),
                label: "Fetch".to_owned(),
                description: "Fetches web pages and converts them to markdown.".to_owned(),
                transport: "stdio".to_owned(),
                command: Some("uvx".to_owned()),
                args: vec!["mcp-server-fetch".to_owned()],
                url: None,
                auth: McpCatalogAuth::None,
                side_effects: vec![
                    "makes outbound HTTP requests".to_owned(),
                    "spawns uvx subprocess".to_owned(),
                ],
                source_url: "https://github.com/modelcontextprotocol/servers/tree/main/src/fetch"
                    .to_owned(),
            },
            McpCatalogEntry {
                id: "memory".to_owned(),
                label: "Memory".to_owned(),
                description: "Persistent knowledge-graph memory backed by a local file.".to_owned(),
                transport: "stdio".to_owned(),
                command: Some("npx".to_owned()),
                args: vec![
                    "-y".to_owned(),
                    "@modelcontextprotocol/server-memory".to_owned(),
                ],
                url: None,
                auth: McpCatalogAuth::None,
                side_effects: vec![
                    "writes memory file under data dir".to_owned(),
                    "spawns npx subprocess".to_owned(),
                ],
                source_url: "https://github.com/modelcontextprotocol/servers/tree/main/src/memory"
                    .to_owned(),
            },
            McpCatalogEntry {
                id: "github-remote".to_owned(),
                label: "GitHub (remote)".to_owned(),
                description: "Official remote GitHub MCP server; you provide your own ".to_owned()
                    + "personal access token or bearer token via plugin config.",
                transport: "streamable_http".to_owned(),
                command: None,
                args: Vec::new(),
                url: Some("https://api.githubcopilot.com/mcp/".to_owned()),
                auth: McpCatalogAuth::Oauth2Remote,
                side_effects: vec![
                    "reads and writes GitHub repositories the signed-in account can access"
                        .to_owned(),
                    "sends requests to api.githubcopilot.com".to_owned(),
                ],
                source_url: "https://github.com/github/github-mcp-server".to_owned(),
            },
        ]
    })
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
                category: String::new(),
                keywords: Vec::new(),
                examples: Vec::new(),
                background: false,
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
/// `ene-tool-mcp` plus a real stdio server.
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

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")]

    use super::*;

    #[test]
    fn catalog_has_four_official_servers() {
        let catalog = mcp_catalog();
        assert_eq!(catalog.len(), 4);
        let ids: Vec<&str> = catalog.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, vec!["git", "fetch", "memory", "github-remote"]);
        for entry in catalog {
            assert!(!entry.label.is_empty());
            assert!(!entry.description.is_empty());
            assert!(
                entry.transport == "stdio" || entry.transport == "streamable_http",
                "unexpected transport {}",
                entry.transport
            );
            if entry.auth == McpCatalogAuth::Oauth2Remote {
                assert!(entry.url.is_some());
            } else {
                assert!(entry.command.is_some());
            }
            assert!(!entry.source_url.is_empty());
        }
    }

    #[test]
    fn catalog_entry_serde_round_trip() {
        let entry = &mcp_catalog()[0];
        let json = serde_json::to_string(entry).unwrap();
        let parsed: McpCatalogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, entry);
    }

    #[test]
    fn catalog_auth_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&McpCatalogAuth::ApiKeyHeader).unwrap(),
            "\"api_key_header\""
        );
        assert_eq!(
            serde_json::to_string(&McpCatalogAuth::Oauth2Remote).unwrap(),
            "\"oauth2_remote\""
        );
    }
}
