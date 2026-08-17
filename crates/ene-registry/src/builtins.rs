use crate::def::{ToolDefinition, ToolSource};
use async_trait::async_trait;
use ene_plugin_ipc::{BuiltinKind, IpcError, ToolHandler, ToolSpecWire, serve_from_env};
use serde_json::{Value, json};

/// Manifest digest stand-in for bundled harness plugins (hash of plugin id).
#[must_use]
pub fn builtin_digest(kind: BuiltinKind) -> String {
    format!(
        "sha256:{}",
        blake3::hash(kind.plugin_id().as_bytes()).to_hex()
    )
}

/// Process entry for a bundled harness plugin binary.
pub async fn run_plugin(kind: BuiltinKind) -> Result<(), IpcError> {
    serve_from_env(BuiltinHandler::new(kind, builtin_digest(kind))).await
}

/// Executes bundled fs/exec/web/utility tools in-process (tests) or from a plugin binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinExecutor;

impl BuiltinExecutor {
    pub fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        match name {
            "utility.hash" => hash(args),
            "utility.time" => Ok(json!({ "unix_ms": chrono::Utc::now().timestamp_millis() })),
            "fs.read" => fs_read(args),
            "fs.write" => fs_write(args),
            "web.fetch" => Err("web.fetch requires broker net in the plugin process".to_owned()),
            "exec.run" => Err(format!(
                "{name} has side effects and must not reach execute"
            )),
            other => Err(format!("unknown builtin {other}")),
        }
    }
}

fn hash(args: &Value) -> Result<Value, String> {
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing text".to_owned())?;
    Ok(json!({ "blake3": blake3::hash(text.as_bytes()).to_hex().to_string() }))
}

fn fs_read(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing path".to_owned())?;
    let body = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    Ok(json!({ "text": body }))
}

fn fs_write(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing path".to_owned())?;
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing text".to_owned())?;
    std::fs::write(path, text).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

/// Specs advertised by each harness plugin.
#[must_use]
pub fn builtin_specs(kind: ene_plugin_ipc::BuiltinKind) -> Vec<ToolSpecWire> {
    match kind {
        ene_plugin_ipc::BuiltinKind::Utility => vec![
            spec(
                "utility.hash",
                "BLAKE3 hash of text",
                json!({"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}),
                Vec::new(),
            ),
            spec(
                "utility.time",
                "Current UTC time",
                json!({"type":"object","additionalProperties":false}),
                Vec::new(),
            ),
        ],
        ene_plugin_ipc::BuiltinKind::Fs => vec![
            spec(
                "fs.read",
                "Read a UTF-8 file",
                json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
                Vec::new(),
            ),
            spec(
                "fs.write",
                "Write a UTF-8 file",
                json!({"type":"object","properties":{"path":{"type":"string"},"text":{"type":"string"}},"required":["path","text"],"additionalProperties":false}),
                vec!["fs.write".to_owned()],
            ),
        ],
        ene_plugin_ipc::BuiltinKind::Exec => vec![spec(
            "exec.run",
            "Run a process",
            json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}),
            vec!["exec".to_owned()],
        )],
        ene_plugin_ipc::BuiltinKind::Web => vec![spec(
            "web.fetch",
            "Fetch a URL via the host broker",
            json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}),
            Vec::new(),
        )],
    }
}

fn spec(
    name: &str,
    description: &str,
    parameters: Value,
    side_effects: Vec<String>,
) -> ToolSpecWire {
    ToolSpecWire {
        name: name.to_owned(),
        description: description.to_owned(),
        parameters,
        output: json!({"type":"object"}),
        side_effects,
    }
}

#[must_use]
pub fn definitions_for(kind: ene_plugin_ipc::BuiltinKind) -> Vec<ToolDefinition> {
    let source = ToolSource::Plugin {
        plugin_id: kind.plugin_id().to_owned(),
    };
    builtin_specs(kind)
        .into_iter()
        .map(|wire| ToolDefinition::from_wire(wire, source.clone()))
        .collect()
}

/// Plugin-side handler for a bundled tool set.
pub struct BuiltinHandler {
    kind: BuiltinKind,
    digest: String,
}

impl BuiltinHandler {
    #[must_use]
    pub fn new(kind: BuiltinKind, digest: impl Into<String>) -> Self {
        Self {
            kind,
            digest: digest.into(),
        }
    }
}

#[async_trait]
impl ToolHandler for BuiltinHandler {
    fn plugin_id(&self) -> &str {
        self.kind.plugin_id()
    }
    fn plugin_name(&self) -> &str {
        self.kind.plugin_id()
    }
    fn digest(&self) -> &str {
        self.digest.as_str()
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        builtin_specs(self.kind)
    }
    async fn call(&self, name: &str, args: Value) -> Result<Value, IpcError> {
        BuiltinExecutor.execute(name, &args).map_err(IpcError::Call)
    }
}
