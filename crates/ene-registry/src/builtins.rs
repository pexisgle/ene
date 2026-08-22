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
///
/// # Errors
///
/// Returns [`IpcError::Io`] when the executable path cannot be read.
pub fn builtin_digest(_kind: BuiltinKind) -> Result<String, IpcError> {
    let exe = std::env::current_exe().map_err(IpcError::Io)?;
    file_digest(&exe)
}

/// Serve a tool plugin over `ENE_PLUGIN_SOCKET`.
///
/// Bundled binaries and a third-party Rust plugin use this entry: local specs
/// and `execute`, no host `BuiltinKind` table. The host still maps bundled
/// plugin ids for in-process tests.
///
/// # Errors
///
/// Returns [`IpcError`] when the socket env is missing or the session fails.
pub async fn run_tool_plugin(
    plugin_id: &'static str,
    specs: fn() -> Vec<ToolSpecWire>,
    execute: fn(&str, &Value) -> Result<Value, String>,
) -> Result<(), IpcError> {
    let exe = std::env::current_exe().map_err(IpcError::Io)?;
    let digest = file_digest(&exe)?;
    serve_from_env(ToolPluginHandler {
        plugin_id,
        digest,
        specs,
        execute,
    })
    .await
}

/// Host-side wrapper around [`run_tool_plugin`] when a [`BuiltinKind`] is already in hand.
///
/// # Errors
///
/// Returns [`IpcError`] when the socket env is missing or the session fails.
pub async fn run_plugin(kind: BuiltinKind) -> Result<(), IpcError> {
    match kind {
        BuiltinKind::Fs => {
            run_tool_plugin(kind.plugin_id(), builtin::fs::specs, builtin::fs::execute).await
        }
        BuiltinKind::Exec => {
            run_tool_plugin(
                kind.plugin_id(),
                builtin::exec::specs,
                builtin::exec::execute,
            )
            .await
        }
        BuiltinKind::Web => {
            run_tool_plugin(kind.plugin_id(), builtin::web::specs, builtin::web::execute).await
        }
        BuiltinKind::Utility => {
            run_tool_plugin(
                kind.plugin_id(),
                builtin::utility::specs,
                builtin::utility::execute,
            )
            .await
        }
        BuiltinKind::App => {
            run_tool_plugin(kind.plugin_id(), builtin::app::specs, builtin::app::execute).await
        }
    }
}

/// Executes bundled tools in-process (tests) or from a plugin binary.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinExecutor;

impl BuiltinExecutor {
    /// Run one bundled tool by name.
    ///
    /// # Errors
    ///
    /// Returns a tool-defined error string.
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
        "fs.delete"
        | "app.screenshot"
        | "app.window_list"
        | "app.active_window"
        | "app.clipboard_get"
        | "app.list_monitors" => Sensitivity::High,
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

struct ToolPluginHandler {
    plugin_id: &'static str,
    digest: String,
    specs: fn() -> Vec<ToolSpecWire>,
    execute: fn(&str, &Value) -> Result<Value, String>,
}

#[async_trait::async_trait]
impl ToolHandler for ToolPluginHandler {
    fn plugin_id(&self) -> &str {
        self.plugin_id
    }
    fn plugin_name(&self) -> &str {
        self.plugin_id
    }
    fn digest(&self) -> &str {
        self.digest.as_str()
    }
    fn specs(&self) -> Vec<ToolSpecWire> {
        (self.specs)()
    }
    async fn call(&self, name: &str, args: Value) -> Result<Value, IpcError> {
        (self.execute)(name, &args).map_err(IpcError::Call)
    }
}
