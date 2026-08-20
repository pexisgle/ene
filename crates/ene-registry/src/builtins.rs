use crate::builtin;
use crate::def::{ToolDefinition, ToolSource};
use ene_plane::Sensitivity;
use ene_plugin_ipc::{BuiltinKind, IpcError, ToolHandler, ToolSpecWire, serve_from_env};
use serde_json::Value;
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

/// Digest of the current plugin executable.
pub fn builtin_digest(_kind: BuiltinKind) -> Result<String, IpcError> {
    let exe = std::env::current_exe().map_err(IpcError::Io)?;
    file_digest(&exe)
}

/// Process entry for a bundled harness plugin binary.
pub async fn run_plugin(kind: BuiltinKind) -> Result<(), IpcError> {
    let digest = builtin_digest(kind)?;
    serve_from_env(BuiltinHandler::new(kind, digest)).await
}

/// Executes bundled tools in-process (tests) or from a plugin binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinExecutor;

impl BuiltinExecutor {
    pub fn execute(&self, name: &str, args: &Value) -> Result<Value, String> {
        builtin::execute(name, args)
    }
}

#[must_use]
pub fn host_spec_for(name: &str) -> Option<ToolSpecWire> {
    for kind in [
        BuiltinKind::Fs,
        BuiltinKind::Exec,
        BuiltinKind::Web,
        BuiltinKind::Utility,
        BuiltinKind::App,
    ] {
        for spec in builtin_specs(kind) {
            if spec.name == name {
                return Some(spec);
            }
        }
    }
    None
}

#[must_use]
pub fn host_sensitivity(name: &str) -> Sensitivity {
    match name {
        "app.screenshot" | "app.window_list" | "app.active_window" | "app.clipboard_get" => {
            Sensitivity::High
        }
        _ => Sensitivity::None,
    }
}

/// Specs advertised by each harness plugin.
#[must_use]
pub fn builtin_specs(kind: BuiltinKind) -> Vec<ToolSpecWire> {
    builtin::specs(kind)
}

#[must_use]
pub fn definitions_for(kind: BuiltinKind) -> Vec<ToolDefinition> {
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
