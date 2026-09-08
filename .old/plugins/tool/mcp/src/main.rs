//! MCP stdio / HTTP bridge. One process per handwritten `mcp.<server>` profile row.

use std::path::PathBuf;

use async_trait::async_trait;
use ene_plugin_ipc::{IpcError, PluginConfigSchema, ToolHandler, ToolSpecWire, serve_from_env};
use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientInfo, GetPromptRequestParams, Implementation,
    ReadResourceRequestParams, ToolAnnotations,
};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

type McpSession = RunningService<RoleClient, ClientInfo>;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let config = load_config();
    let bridge = match McpBridge::connect(config).await {
        Ok(bridge) => bridge,
        Err(err) => {
            tracing::error!(error = %err, "mcp spawn failed");
            std::process::exit(1);
        }
    };
    if let Err(err) = serve_from_env(bridge).await {
        tracing::error!(error = %err, plugin = "mcp", "fatal");
        std::process::exit(1);
    }
}

struct McpConfig {
    server: String,
    transport: String,
    command: String,
    args: Vec<String>,
    url: Option<String>,
    auth_token: Option<String>,
    probe_only: bool,
    skills_home: PathBuf,
}

fn load_config() -> McpConfig {
    let raw = std::env::var("ENE_PROVIDER_CONFIG").unwrap_or_else(|_| "{}".to_owned());
    let value: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({}));
    let server = value
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("default")
        .to_owned();
    let transport = value
        .get("transport")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let command = value
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let args = value
        .get("args")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let auth_token = value
        .get("auth_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let probe_only = value
        .get("probe_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let skills_home = value
        .get("skills_home")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_default();
    McpConfig {
        server,
        transport,
        command,
        args,
        url,
        auth_token,
        probe_only,
        skills_home,
    }
}

struct McpBridge {
    plugin_id: String,
    digest: String,
    specs: Vec<ToolSpecWire>,
    session: Mutex<McpSession>,
}

fn tool_side_effects(annotations: Option<&ToolAnnotations>) -> Vec<String> {
    let Some(annotations) = annotations else {
        return vec!["may modify data".to_owned()];
    };
    if annotations.read_only_hint == Some(true) {
        return Vec::new();
    }
    let mut effects = Vec::new();
    if annotations.destructive_hint.unwrap_or(true) {
        effects.push("may modify data".to_owned());
    }
    if annotations.open_world_hint == Some(true) {
        effects.push("sends data to external services".to_owned());
    }
    effects
}

fn client_info() -> ClientInfo {
    ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("ene", "0.1.0"),
    )
}

impl McpBridge {
    async fn connect(config: McpConfig) -> Result<Self, String> {
        let session = if is_http_transport(&config) {
            connect_http(&config).await?
        } else {
            connect_stdio(&config).await?
        };
        let listed = session
            .list_all_tools()
            .await
            .map_err(|err| err.to_string())?;
        let plugin_id = format!("mcp.{}", config.server);
        let specs = listed
            .into_iter()
            .map(|tool| {
                let name = tool.name.as_ref().to_owned();
                let description = tool.description.as_deref().unwrap_or("").to_owned();
                let side_effects = tool_side_effects(tool.annotations.as_ref());
                let parameters = Value::Object((*tool.input_schema).clone());
                ToolSpecWire {
                    name: format!("mcp:{}.{name}", config.server),
                    description,
                    parameters,
                    output: json!({"type":"object"}),
                    side_effects,
                    broker_socket: None,
                    category: String::new(),
                    keywords: Vec::new(),
                    examples: Vec::new(),
                    background: false,
                }
            })
            .collect();
        if !config.probe_only {
            ingest_context(&session, &config).await;
        }
        Ok(Self {
            plugin_id,
            digest: exe_digest(),
            specs,
            session: Mutex::new(session),
        })
    }
}

fn is_http_transport(config: &McpConfig) -> bool {
    matches!(
        config.transport.as_str(),
        "http" | "sse" | "streamable_http" | "streamable-http"
    ) || (config.url.is_some() && config.command.is_empty())
}

async fn connect_stdio(config: &McpConfig) -> Result<McpSession, String> {
    if config.command.is_empty() {
        return Err("mcp stdio needs command".to_owned());
    }
    let args = config.args.clone();
    let transport = TokioChildProcess::new(Command::new(&config.command).configure(|cmd| {
        cmd.args(&args);
        cmd.kill_on_drop(true);
    }))
    .map_err(|err| err.to_string())?;
    client_info()
        .serve(transport)
        .await
        .map_err(|err| err.to_string())
}

async fn connect_http(config: &McpConfig) -> Result<McpSession, String> {
    let url = config
        .url
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "mcp http needs url".to_owned())?;
    let mut transport_config = StreamableHttpClientTransportConfig::with_uri(url.to_owned());
    if let Some(token) = config.auth_token.as_deref() {
        transport_config = transport_config.auth_header(token);
    }
    let transport = StreamableHttpClientTransport::from_config(transport_config);
    client_info()
        .serve(transport)
        .await
        .map_err(|err| err.to_string())
}

async fn ingest_context(session: &McpSession, config: &McpConfig) {
    if let Ok(resources) = session.list_all_resources().await {
        let text = resource_markdown(session, &resources).await;
        if !text.is_empty() {
            write_resource_snapshot(&config.server, &text);
        }
    }
    if config.skills_home.as_os_str().is_empty() {
        return;
    }
    let Ok(prompts) = session.list_all_prompts().await else {
        return;
    };
    for prompt in prompts {
        let name = prompt.name;
        let description = prompt.description.as_deref().unwrap_or(&name);
        let body = match session.get_prompt(GetPromptRequestParams::new(&name)).await {
            Ok(got) => match serde_json::to_value(&got) {
                Ok(value) => prompt_body(&value),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };
        write_prompt_skill(&config.skills_home, &name, description, &body);
    }
}

async fn resource_markdown(session: &McpSession, resources: &[rmcp::model::Resource]) -> String {
    let mut chunks = Vec::new();
    for resource in resources.iter().take(32) {
        let uri = resource.uri.as_str();
        if uri.is_empty() {
            continue;
        }
        let title = if resource.name.is_empty() {
            uri
        } else {
            resource.name.as_str()
        };
        let body = match session
            .read_resource(ReadResourceRequestParams::new(uri))
            .await
        {
            Ok(got) => match serde_json::to_value(&got) {
                Ok(value) => resource_text(&value),
                Err(_) => String::new(),
            },
            Err(_) => String::new(),
        };
        if body.is_empty() {
            chunks.push(format!("### {title}\n{uri}"));
        } else {
            chunks.push(format!("### {title}\n{body}"));
        }
    }
    chunks.join("\n\n")
}

fn resource_text(got: &Value) -> String {
    got.get("contents")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| row.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn prompt_body(got: &Value) -> String {
    got.get("messages")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let content = row.get("content")?;
                    if let Some(text) = content.as_str() {
                        return Some(text.to_owned());
                    }
                    content
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn write_resource_snapshot(server: &str, text: &str) {
    let Ok(workspace) = std::env::var("ENE_WORKSPACE") else {
        return;
    };
    let dir = PathBuf::from(workspace).join("mcp-context");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let mut end = 16_384.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let body = if text.len() > end {
        format!("{}\n", text.get(..end).unwrap_or(text))
    } else {
        text.to_owned()
    };
    drop(std::fs::write(dir.join(format!("{server}.md")), body));
}

fn write_prompt_skill(home: &std::path::Path, name: &str, description: &str, body: &str) {
    let slug: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if slug.is_empty() {
        return;
    }
    let skill_dir = home.join(&slug);
    if std::fs::create_dir_all(&skill_dir).is_err() {
        return;
    }
    let summary = if description.is_empty() {
        slug.clone()
    } else {
        description
            .chars()
            .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
            .collect()
    };
    let md = format!("---\nname: {slug}\ndescription: {summary}\n---\n\n{body}\n");
    drop(std::fs::write(skill_dir.join("SKILL.md"), md));
}

fn exe_digest() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map_or_else(
            || "blake3:unknown".to_owned(),
            |bytes| format!("blake3:{}", blake3::hash(&bytes).to_hex()),
        )
}

#[async_trait]
impl ToolHandler for McpBridge {
    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
    fn plugin_name(&self) -> &str {
        &self.plugin_id
    }
    fn digest(&self) -> &str {
        &self.digest
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        self.specs.clone()
    }
    async fn call(&self, name: &str, args: Value) -> Result<Value, IpcError> {
        let server = self
            .plugin_id
            .strip_prefix("mcp.")
            .unwrap_or(&self.plugin_id);
        let prefix = format!("mcp:{server}.");
        let tool = name.strip_prefix(&prefix).unwrap_or(name).to_owned();
        let mut params = CallToolRequestParams::new(tool);
        if let Value::Object(map) = args {
            params = params.with_arguments(map);
        }
        let session = self.session.lock().await;
        let result = session
            .call_tool(params)
            .await
            .map_err(|err| IpcError::Call(err.to_string()))?;
        serde_json::to_value(&result).map_err(|err| IpcError::Call(err.to_string()))
    }

    fn has_config(&self) -> bool {
        true
    }

    async fn config_schema(&self) -> Result<PluginConfigSchema, IpcError> {
        Ok(PluginConfigSchema {
            has_config: true,
            schema: json!({
                "type": "object",
                "properties": {
                    "auth_token": { "type": "string", "x-ene-secret": true }
                },
                "additionalProperties": false
            }),
            secret_keys: vec!["auth_token".to_owned()],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_map_to_side_effect_strings() {
        assert_eq!(
            tool_side_effects(Some(&ToolAnnotations::from_raw(
                None,
                Some(true),
                None,
                None,
                None,
            ))),
            Vec::<String>::new()
        );
        assert_eq!(
            tool_side_effects(Some(&ToolAnnotations::from_raw(
                None,
                None,
                Some(true),
                None,
                None,
            ))),
            vec!["may modify data".to_owned()]
        );
        assert_eq!(
            tool_side_effects(Some(&ToolAnnotations::default())),
            vec!["may modify data".to_owned()]
        );
        assert_eq!(tool_side_effects(None), vec!["may modify data".to_owned()]);
    }
}
