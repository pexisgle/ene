//! MCP stdio / HTTP bridge. One process per handwritten `mcp.<server>` profile row.

use std::process::Stdio;

use async_trait::async_trait;
use ene_plugin_ipc::{IpcError, ToolHandler, ToolSpecWire, serve_from_env};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

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
    McpConfig {
        server,
        transport,
        command,
        args,
        url,
    }
}

struct McpBridge {
    plugin_id: String,
    digest: String,
    specs: Vec<ToolSpecWire>,
    session: Mutex<McpTransport>,
}

enum McpTransport {
    Stdio(Box<McpStdio>),
    Http(McpHttp),
}

struct McpStdio {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

struct McpHttp {
    url: String,
    client: reqwest::Client,
    next_id: u64,
}

impl McpBridge {
    async fn connect(config: McpConfig) -> Result<Self, String> {
        let http = config.transport == "http"
            || config.transport == "sse"
            || config.transport == "streamable_http"
            || config.transport == "streamable-http"
            || (config.url.is_some() && config.command.is_empty());
        let mut session = if http {
            let url = config.url.ok_or_else(|| "mcp http needs url".to_owned())?;
            McpTransport::Http(McpHttp {
                url,
                client: reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .map_err(|err| err.to_string())?,
                next_id: 1,
            })
        } else {
            if config.command.is_empty() {
                return Err("mcp stdio needs command".to_owned());
            }
            let mut child = Command::new(&config.command)
                .args(&config.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn()
                .map_err(|err| err.to_string())?;
            let stdin = child.stdin.take().ok_or_else(|| "mcp stdin".to_owned())?;
            let stdout = child.stdout.take().ok_or_else(|| "mcp stdout".to_owned())?;
            McpTransport::Stdio(Box::new(McpStdio {
                child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            }))
        };
        session
            .rpc(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "ene", "version": "0.1.0" }
                }),
            )
            .await?;
        session
            .notify("notifications/initialized", json!({}))
            .await?;
        let listed = session.rpc("tools/list", json!({})).await?;
        let tools = listed
            .get("tools")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let plugin_id = format!("mcp.{}", config.server);
        let specs = tools
            .into_iter()
            .filter_map(|tool| {
                let name = tool.get("name")?.as_str()?.to_owned();
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let parameters = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type":"object"}));
                Some(ToolSpecWire {
                    name: format!("mcp:{}.{name}", config.server),
                    description,
                    parameters,
                    output: json!({"type":"object"}),
                    side_effects: Vec::new(),
                })
            })
            .collect();
        Ok(Self {
            plugin_id,
            digest: exe_digest(),
            specs,
            session: Mutex::new(session),
        })
    }
}

impl McpTransport {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Self::Stdio(session) => session.rpc(method, params).await,
            Self::Http(session) => session.rpc(method, params).await,
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Self::Stdio(session) => session.notify(method, params).await,
            Self::Http(session) => session.notify(method, params).await,
        }
    }
}

impl McpStdio {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        write_msg(&mut self.stdin, &msg).await?;
        loop {
            let incoming = read_msg(&mut self.stdout).await?;
            if json_id(&incoming) == Some(id) {
                if let Some(err) = incoming.get("error") {
                    return Err(err.to_string());
                }
                return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        write_msg(&mut self.stdin, &msg).await
    }
}

impl McpHttp {
    async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let incoming = post_rpc(&self.client, &self.url, &msg).await?;
        if let Some(err) = incoming.get("error") {
            return Err(err.to_string());
        }
        Ok(incoming.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        drop(post_rpc(&self.client, &self.url, &msg).await);
        Ok(())
    }
}

async fn post_rpc(client: &reqwest::Client, url: &str, msg: &Value) -> Result<Value, String> {
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .json(msg)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    let text = response.text().await.map_err(|err| err.to_string())?;
    parse_rpc_body(&text)
}

fn parse_rpc_body(text: &str) -> Result<Value, String> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(|err| err.to_string());
    }
    for line in trimmed.lines() {
        let Some(rest) = line.strip_prefix("data:") else {
            continue;
        };
        let rest = rest.trim();
        if rest.starts_with('{') {
            return serde_json::from_str(rest).map_err(|err| err.to_string());
        }
    }
    Err("mcp http: no json body".to_owned())
}

fn json_id(value: &Value) -> Option<u64> {
    value.get("id").and_then(Value::as_u64).or_else(|| {
        value
            .get("id")
            .and_then(Value::as_i64)
            .and_then(|n| u64::try_from(n).ok())
    })
}

async fn write_msg(stdin: &mut ChildStdin, msg: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(msg).map_err(|err| err.to_string())?;
    let header = format!("Content-Length: {}\r\n\r\n", body.len());
    stdin
        .write_all(header.as_bytes())
        .await
        .map_err(|err| err.to_string())?;
    stdin
        .write_all(&body)
        .await
        .map_err(|err| err.to_string())?;
    stdin.flush().await.map_err(|err| err.to_string())
}

async fn read_msg(stdout: &mut BufReader<ChildStdout>) -> Result<Value, String> {
    let mut first = String::new();
    stdout
        .read_line(&mut first)
        .await
        .map_err(|err| err.to_string())?;
    if first.is_empty() {
        return Err("mcp eof".to_owned());
    }
    if first.trim_start().starts_with('{') {
        return serde_json::from_str(first.trim()).map_err(|err| err.to_string());
    }
    let mut headers = first;
    loop {
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .await
            .map_err(|err| err.to_string())?;
        if line.is_empty() {
            return Err("mcp eof".to_owned());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        headers.push_str(&line);
    }
    let mut length = 0_usize;
    for line in headers.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.eq_ignore_ascii_case("content-length") {
            length = value
                .trim()
                .parse()
                .map_err(|err: std::num::ParseIntError| err.to_string())?;
        }
    }
    if length == 0 {
        return Err("mcp missing content-length".to_owned());
    }
    let mut buf = vec![0_u8; length];
    stdout
        .read_exact(&mut buf)
        .await
        .map_err(|err| err.to_string())?;
    serde_json::from_slice(&buf).map_err(|err| err.to_string())
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
        let mut session = self.session.lock().await;
        session
            .rpc("tools/call", json!({"name": tool, "arguments": args}))
            .await
            .map_err(IpcError::Call)
    }
}

impl Drop for McpBridge {
    fn drop(&mut self) {
        if let Ok(mut session) = self.session.try_lock()
            && let McpTransport::Stdio(stdio) = &mut *session
        {
            drop(stdio.child.start_kill());
        }
    }
}
