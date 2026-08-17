use crate::def::{ToolDefinition, ToolSource};
use ene_plugin_ipc::{BuiltinKind, IpcError, ToolHandler, ToolSpecWire, serve_from_env};
use serde_json::{Value, json};
use std::path::Path;

/// BLAKE3 digest of a plugin binary or script file (`blake3:<hex>`).
///
/// # Errors
///
/// Returns [`IpcError::Io`] when the file cannot be read.
pub fn file_digest(path: &Path) -> Result<String, IpcError> {
    let bytes = std::fs::read(path).map_err(IpcError::Io)?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

/// Legacy alias kept for callers that still name this `builtin_digest`.
pub fn builtin_digest(_kind: BuiltinKind) -> Result<String, IpcError> {
    let exe = std::env::current_exe().map_err(IpcError::Io)?;
    file_digest(&exe)
}

/// Process entry for a bundled harness plugin binary.
pub async fn run_plugin(kind: BuiltinKind) -> Result<(), IpcError> {
    let digest = builtin_digest(kind)?;
    serve_from_env(BuiltinHandler::new(kind, digest)).await
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
    if let Ok(workspace) = std::env::var("ENE_WORKSPACE") {
        let root = Path::new(&workspace);
        if path.starts_with('/') {
            let canonical = Path::new(path)
                .canonicalize()
                .map_err(|err| err.to_string())?;
            let base = root.canonicalize().map_err(|err| err.to_string())?;
            if !canonical.starts_with(&base) {
                return Err("path outside workspace".to_owned());
            }
        }
    }
    let body = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    Ok(json!({ "text": body }))
}

fn fs_write(args: &Value) -> Result<Value, String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing path".to_owned())?;
    if let Ok(workspace) = std::env::var("ENE_WORKSPACE") {
        let root = Path::new(&workspace);
        if path.starts_with('/') {
            let canonical = Path::new(path)
                .canonicalize()
                .map_err(|err| err.to_string())?;
            let base = root.canonicalize().map_err(|err| err.to_string())?;
            if !canonical.starts_with(&base) {
                return Err("path outside workspace".to_owned());
            }
        }
    }
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing text".to_owned())?;
    std::fs::write(path, text).map_err(|err| err.to_string())?;
    Ok(json!({ "ok": true }))
}

#[must_use]
pub fn host_spec_for(name: &str) -> Option<ToolSpecWire> {
    for kind in [
        BuiltinKind::Fs,
        BuiltinKind::Exec,
        BuiltinKind::Web,
        BuiltinKind::Utility,
    ] {
        for spec in builtin_specs(kind) {
            if spec.name == name {
                return Some(spec);
            }
        }
    }
    None
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
            vec!["send".to_owned()],
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

#[async_trait::async_trait]
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
