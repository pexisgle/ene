use crate::BuiltinExecutor;
use crate::def::{Layer, ToolDefinition, ToolSource};
use async_trait::async_trait;
use ene_plane::{ApprovalPlane, AuthzRequest};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

/// Executes a registered tool (plugin IPC, MCP, or in-process harness).
#[async_trait]
pub trait ToolInvoke: Send + Sync {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String>;
}

/// In-process executor used by tests and by plugin-side handlers.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltinInvoker;

#[async_trait]
impl ToolInvoke for BuiltinInvoker {
    async fn invoke(&self, name: &str, args: Value) -> Result<Value, String> {
        BuiltinExecutor.execute(name, &args)
    }
}

struct Registered {
    def: ToolDefinition,
    invoke: Arc<dyn ToolInvoke>,
}

/// Registry / pipeline failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PipelineError {
    #[error("unknown tool {0}")]
    Unknown(String),
    #[error("tool {0} is not on the surface schema")]
    NotOnSurface(String),
    #[error("denied {name}: {reason}")]
    Denied { name: String, reason: String },
    #[error("path escapes workspace: {0}")]
    PathEscape(String),
    #[error(transparent)]
    Plane(#[from] ene_plane::PlaneError),
    #[error("execute: {0}")]
    Execute(String),
}

/// In-memory registry. Fiber unload removes rows by plugin id.
pub struct ToolRegistry {
    tools: Mutex<HashMap<String, Registered>>,
    plane: Mutex<Option<Arc<ApprovalPlane>>>,
    workspace: Mutex<Option<PathBuf>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Mutex::new(HashMap::new()),
            plane: Mutex::new(None),
            workspace: Mutex::new(None),
        }
    }

    pub fn set_plane(&self, plane: Arc<ApprovalPlane>) {
        *self.plane.lock() = Some(plane);
    }

    pub fn set_workspace(&self, path: impl Into<PathBuf>) {
        *self.workspace.lock() = Some(path.into());
    }

    pub fn register(&self, def: ToolDefinition) {
        self.register_with(def, Arc::new(BuiltinInvoker));
    }

    pub fn register_with(&self, def: ToolDefinition, invoke: Arc<dyn ToolInvoke>) {
        self.tools
            .lock()
            .insert(def.name.clone(), Registered { def, invoke });
    }

    /// Inverse of register for one plugin source (I-46).
    pub fn unregister_source(&self, source: &ToolSource) {
        self.tools.lock().retain(|_, row| &row.def.source != source);
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<ToolDefinition> {
        self.tools.lock().get(name).map(|row| row.def.clone())
    }

    /// All registered tools, name-sorted.
    #[must_use]
    pub fn list(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.lock();
        let mut defs: Vec<ToolDefinition> = tools.values().map(|row| row.def.clone()).collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Model-visible schemas for a layer. Host-only fields are excluded.
    #[must_use]
    pub fn schemas(&self, layer: Layer) -> Vec<Value> {
        let tools = self.tools.lock();
        let mut names: Vec<&ToolDefinition> = tools.values().map(|row| &row.def).collect();
        names.sort_by(|a, b| a.name.cmp(&b.name));
        names
            .into_iter()
            .filter(|def| match layer {
                Layer::Surface => def.surface_visible(),
                Layer::Job => true,
            })
            .map(ToolDefinition::model_schema)
            .collect()
    }

    /// Validate → deny-by-default for side effects → execute.
    pub async fn execute(
        &self,
        name: &str,
        mut args: Value,
        layer: Layer,
    ) -> Result<Value, PipelineError> {
        let (def, invoke) = {
            let tools = self.tools.lock();
            let row = tools
                .get(name)
                .ok_or_else(|| PipelineError::Unknown(name.to_owned()))?;
            (row.def.clone(), Arc::clone(&row.invoke))
        };
        if layer == Layer::Surface && !def.surface_visible() {
            return Err(PipelineError::NotOnSurface(name.to_owned()));
        }
        let workspace = self.workspace.lock().clone();
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        let in_workspace = path_in_workspace(workspace.as_deref(), path);
        let plane = self.plane.lock().clone();
        if let Some(plane) = plane {
            let req = AuthzRequest {
                tool: name.to_owned(),
                side_effects: def.side_effects.clone(),
                sensitivity: def.sensitivity,
                target: path.to_owned(),
                in_workspace,
            };
            plane.authorize(&req).await?;
        } else if !def.side_effects.is_empty() {
            return Err(PipelineError::Denied {
                name: name.to_owned(),
                reason: "deny-by-default until approval plane".to_owned(),
            });
        }
        if name == "fs.read" || name == "fs.write" {
            let Some(root) = workspace else {
                return Err(PipelineError::Denied {
                    name: name.to_owned(),
                    reason: "workspace is not configured".to_owned(),
                });
            };
            let raw =
                args.get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| PipelineError::Denied {
                        name: name.to_owned(),
                        reason: "missing path".to_owned(),
                    })?;
            let confined = confine_tool_path(&root, Path::new(raw), name == "fs.write")?;
            if let Some(obj) = args.as_object_mut() {
                obj.insert(
                    "path".to_owned(),
                    Value::String(confined.display().to_string()),
                );
            }
        }
        invoke
            .invoke(name, args)
            .await
            .map_err(PipelineError::Execute)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn path_in_workspace(workspace: Option<&Path>, path: &str) -> bool {
    let Some(root) = workspace else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    confine_tool_path(root, Path::new(path), false).is_ok()
}

fn confine_tool_path(
    workspace: &Path,
    path: &Path,
    create_parent: bool,
) -> Result<PathBuf, PipelineError> {
    let base = workspace
        .canonicalize()
        .map_err(|err| PipelineError::PathEscape(err.to_string()))?;
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let file_name = requested
        .file_name()
        .ok_or_else(|| PipelineError::PathEscape(requested.display().to_string()))?;
    if file_name == Component::CurDir.as_os_str()
        || file_name == Component::ParentDir.as_os_str()
        || file_name.to_string_lossy().contains('/')
        || file_name.to_string_lossy().contains('\\')
    {
        return Err(PipelineError::PathEscape(requested.display().to_string()));
    }
    let parent = requested
        .parent()
        .ok_or_else(|| PipelineError::PathEscape(requested.display().to_string()))?;
    let canonical_parent = canonicalize_parent(&base, parent, create_parent)?;
    if !canonical_parent.starts_with(&base) {
        return Err(PipelineError::PathEscape(requested.display().to_string()));
    }
    let resolved = canonical_parent.join(file_name);
    if resolved.exists() {
        let canonical = resolved
            .canonicalize()
            .map_err(|err| PipelineError::PathEscape(err.to_string()))?;
        if !canonical.starts_with(&base) {
            return Err(PipelineError::PathEscape(canonical.display().to_string()));
        }
        return Ok(canonical);
    }
    Ok(resolved)
}

fn canonicalize_parent(
    base: &Path,
    parent: &Path,
    create_parent: bool,
) -> Result<PathBuf, PipelineError> {
    if parent.exists() {
        return parent
            .canonicalize()
            .map_err(|_| PipelineError::PathEscape(parent.display().to_string()));
    }
    if !create_parent {
        return Err(PipelineError::PathEscape(parent.display().to_string()));
    }
    let mut existing = parent.to_path_buf();
    let mut missing: Vec<PathBuf> = Vec::new();
    while !existing.exists() {
        missing.push(existing.clone());
        let Some(next) = existing.parent() else {
            return Err(PipelineError::PathEscape(parent.display().to_string()));
        };
        existing = next.to_path_buf();
    }
    let anchor = existing
        .canonicalize()
        .map_err(|err| PipelineError::PathEscape(err.to_string()))?;
    if !anchor.starts_with(base) {
        return Err(PipelineError::PathEscape(parent.display().to_string()));
    }
    missing.reverse();
    let mut current = anchor;
    for segment in missing {
        let Some(name) = segment.file_name() else {
            return Err(PipelineError::PathEscape(parent.display().to_string()));
        };
        if name == Component::ParentDir.as_os_str() || name == Component::CurDir.as_os_str() {
            return Err(PipelineError::PathEscape(parent.display().to_string()));
        }
        current = current.join(name);
        if !current.starts_with(base) {
            return Err(PipelineError::PathEscape(parent.display().to_string()));
        }
        if !current.exists() {
            std::fs::create_dir(&current)
                .map_err(|err| PipelineError::PathEscape(err.to_string()))?;
        }
    }
    Ok(current)
}
