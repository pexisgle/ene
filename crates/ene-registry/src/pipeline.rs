use crate::BuiltinExecutor;
use crate::def::{Layer, ToolDefinition, ToolSource};
use crate::discovery::{IndexedTool, ToolHit, lexical_score};
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

    async fn start_background(
        &self,
        execution_id: &str,
        name: &str,
        args: Value,
        deadline_ms: Option<u64>,
    ) -> Result<(), String> {
        let _ = (execution_id, args, deadline_ms);
        Err(format!("{name} is not background-capable"))
    }

    async fn cancel_background(&self, execution_id: &str) -> Result<String, String> {
        let _ = execution_id;
        Ok("unknown".to_owned())
    }

    async fn status_background(
        &self,
        execution_id: &str,
    ) -> Result<(String, Option<String>), String> {
        let _ = execution_id;
        Ok(("unknown".to_owned(), Some("unknown_execution".to_owned())))
    }

    async fn take_completion(
        &self,
        execution_id: &str,
    ) -> Option<ene_plugin_ipc::ToolExecutionComplete> {
        let _ = execution_id;
        None
    }

    async fn take_completions(&self) -> Vec<ene_plugin_ipc::ToolExecutionComplete> {
        Vec::new()
    }
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
    owner: String,
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
    #[error("tool {0} is not background-capable")]
    NotBackground(String),
    #[error("path escapes workspace: {0}")]
    PathEscape(String),
    #[error(transparent)]
    Plane(#[from] ene_plane::PlaneError),
    #[error("execute: {0}")]
    Execute(String),
}

/// In-memory registry. Fiber unload removes rows by plugin id.
pub struct ToolRegistry {
    tools: Mutex<HashMap<String, Vec<Registered>>>,
    index: Mutex<HashMap<String, IndexedTool>>,
    plane: Mutex<Option<Arc<ApprovalPlane>>>,
    workspace: Mutex<Option<PathBuf>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Mutex::new(HashMap::new()),
            index: Mutex::new(HashMap::new()),
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

    #[must_use]
    pub fn workspace(&self) -> Option<PathBuf> {
        self.workspace.lock().clone()
    }

    pub fn register(&self, def: ToolDefinition) {
        self.register_with(def, Arc::new(BuiltinInvoker));
    }

    pub fn register_with(&self, def: ToolDefinition, invoke: Arc<dyn ToolInvoke>) {
        self.register_owned(String::new(), def, invoke);
    }

    /// Push a named tool owned by `owner`. The last push is the active binding.
    pub fn register_owned(
        &self,
        owner: impl Into<String>,
        def: ToolDefinition,
        invoke: Arc<dyn ToolInvoke>,
    ) {
        let owner = owner.into();
        let name = def.name.clone();
        let indexed = IndexedTool::from_definition(&def);
        self.tools
            .lock()
            .entry(name.clone())
            .or_default()
            .push(Registered { owner, def, invoke });
        self.index.lock().insert(name, indexed);
    }

    /// Inverse of a single `register` / `register_with`. Drops every owner.
    pub fn unregister(&self, name: &str) {
        self.tools.lock().remove(name);
        self.index.lock().remove(name);
    }

    /// Drop this owner's entries for `name`. A buried owner can leave without
    /// clobbering a later binding; unloading the top restores the previous owner.
    pub fn unregister_owned(&self, name: &str, owner: &str) {
        let mut tools = self.tools.lock();
        let mut index = self.index.lock();
        {
            let Some(stack) = tools.get_mut(name) else {
                return;
            };
            stack.retain(|row| row.owner != owner);
            if let Some(last) = stack.last() {
                index.insert(name.to_owned(), IndexedTool::from_definition(&last.def));
                return;
            }
        }
        tools.remove(name);
        index.remove(name);
    }

    /// Inverse of register for one plugin source.
    pub fn unregister_source(&self, source: &ToolSource) {
        let mut tools = self.tools.lock();
        let mut index = self.index.lock();
        tools.retain(|name, stack| {
            stack.retain(|row| &row.def.source != source);
            if let Some(last) = stack.last() {
                index.insert(name.clone(), IndexedTool::from_definition(&last.def));
                true
            } else {
                index.remove(name);
                false
            }
        });
    }

    /// Drop every tool registered by one plugin id.
    pub fn unregister_plugin(&self, plugin_id: &str) {
        self.unregister_source(&ToolSource::Plugin {
            plugin_id: plugin_id.to_owned(),
        });
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<ToolDefinition> {
        self.tools
            .lock()
            .get(name)
            .and_then(|stack| stack.last())
            .map(|row| row.def.clone())
    }

    /// All registered tools, name-sorted.
    #[must_use]
    pub fn list(&self) -> Vec<ToolDefinition> {
        let tools = self.tools.lock();
        let mut defs: Vec<ToolDefinition> = tools
            .values()
            .filter_map(|stack| stack.last().map(|row| row.def.clone()))
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Lexical tool discovery. Empty query returns a name-sorted prefix.
    #[must_use]
    pub fn search_tools(&self, query: &str, limit: usize) -> Vec<ToolHit> {
        let tools = self.tools.lock();
        let index = self.index.lock();
        let mut hits: Vec<ToolHit> = tools
            .values()
            .filter_map(|stack| stack.last())
            .map(|row| {
                let indexed = index
                    .get(&row.def.name)
                    .map_or_else(|| IndexedTool::from_definition(&row.def), Clone::clone);
                let score = lexical_score(query, &indexed);
                ToolHit {
                    tool: row.def.clone(),
                    score,
                }
            })
            .collect();
        if query.trim().is_empty() {
            hits.sort_by(|left, right| left.tool.name.cmp(&right.tool.name));
        } else {
            hits.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| left.tool.name.cmp(&right.tool.name))
            });
        }
        hits.truncate(limit);
        hits
    }

    /// Model-visible schemas for a layer. Host-only fields are excluded.
    #[must_use]
    pub fn schemas(&self, layer: Layer) -> Vec<Value> {
        let tools = self.tools.lock();
        let mut names: Vec<&ToolDefinition> = tools
            .values()
            .filter_map(|stack| stack.last().map(|row| &row.def))
            .collect();
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
        args: Value,
        layer: Layer,
    ) -> Result<Value, PipelineError> {
        self.execute_inner(name, args, layer, false, None).await
    }

    /// Same as [`Self::execute`], confining filesystem tools to `workspace`.
    pub async fn execute_in_workspace(
        &self,
        name: &str,
        args: Value,
        layer: Layer,
        workspace: &Path,
    ) -> Result<Value, PipelineError> {
        self.execute_inner(name, args, layer, false, Some(workspace.to_path_buf()))
            .await
    }

    /// Host-initiated call (continuous observation). Enabling the setting is consent.
    pub async fn execute_host(&self, name: &str, args: Value) -> Result<Value, PipelineError> {
        self.execute_inner(name, args, Layer::Job, true, None).await
    }

    async fn execute_inner(
        &self,
        name: &str,
        args: Value,
        layer: Layer,
        host: bool,
        workspace_override: Option<PathBuf>,
    ) -> Result<Value, PipelineError> {
        let (_, invoke, prepared) = self
            .prepare(name, args, layer, host, workspace_override)
            .await?;
        invoke
            .invoke(name, prepared)
            .await
            .map_err(PipelineError::Execute)
    }

    pub async fn start_background(
        &self,
        name: &str,
        args: Value,
        execution_id: &str,
        layer: Layer,
        deadline_ms: Option<u64>,
    ) -> Result<(), PipelineError> {
        self.start_background_inner(name, args, execution_id, layer, deadline_ms, None)
            .await
    }

    pub async fn start_background_in_workspace(
        &self,
        name: &str,
        args: Value,
        execution_id: &str,
        layer: Layer,
        deadline_ms: Option<u64>,
        workspace: &Path,
    ) -> Result<(), PipelineError> {
        self.start_background_inner(
            name,
            args,
            execution_id,
            layer,
            deadline_ms,
            Some(workspace.to_path_buf()),
        )
        .await
    }

    async fn start_background_inner(
        &self,
        name: &str,
        args: Value,
        execution_id: &str,
        layer: Layer,
        deadline_ms: Option<u64>,
        workspace_override: Option<PathBuf>,
    ) -> Result<(), PipelineError> {
        let (def, invoke, prepared) = self
            .prepare(name, args, layer, false, workspace_override)
            .await?;
        if !def.background {
            return Err(PipelineError::NotBackground(name.to_owned()));
        }
        invoke
            .start_background(execution_id, name, prepared, deadline_ms)
            .await
            .map_err(PipelineError::Execute)
    }

    pub async fn cancel_background(
        &self,
        name: &str,
        execution_id: &str,
    ) -> Result<String, PipelineError> {
        let invoke = self.invoke_for(name)?;
        invoke
            .cancel_background(execution_id)
            .await
            .map_err(PipelineError::Execute)
    }

    pub async fn status_background(
        &self,
        name: &str,
        execution_id: &str,
    ) -> Result<(String, Option<String>), PipelineError> {
        let invoke = self.invoke_for(name)?;
        invoke
            .status_background(execution_id)
            .await
            .map_err(PipelineError::Execute)
    }

    pub async fn take_completion(
        &self,
        name: &str,
        execution_id: &str,
    ) -> Result<Option<ene_plugin_ipc::ToolExecutionComplete>, PipelineError> {
        let invoke = self.invoke_for(name)?;
        Ok(invoke.take_completion(execution_id).await)
    }

    pub async fn take_completions(
        &self,
        name: &str,
    ) -> Result<Vec<ene_plugin_ipc::ToolExecutionComplete>, PipelineError> {
        let invoke = self.invoke_for(name)?;
        Ok(invoke.take_completions().await)
    }

    fn invoke_for(&self, name: &str) -> Result<Arc<dyn ToolInvoke>, PipelineError> {
        let tools = self.tools.lock();
        let row = tools
            .get(name)
            .and_then(|stack| stack.last())
            .ok_or_else(|| PipelineError::Unknown(name.to_owned()))?;
        Ok(Arc::clone(&row.invoke))
    }

    async fn prepare(
        &self,
        name: &str,
        mut args: Value,
        layer: Layer,
        host: bool,
        workspace_override: Option<PathBuf>,
    ) -> Result<(ToolDefinition, Arc<dyn ToolInvoke>, Value), PipelineError> {
        let (def, invoke) = {
            let tools = self.tools.lock();
            let row = tools
                .get(name)
                .and_then(|stack| stack.last())
                .ok_or_else(|| PipelineError::Unknown(name.to_owned()))?;
            (row.def.clone(), Arc::clone(&row.invoke))
        };
        if !host && layer == Layer::Surface && !def.surface_visible() {
            return Err(PipelineError::NotOnSurface(name.to_owned()));
        }
        let workspace = workspace_override.or_else(|| self.workspace.lock().clone());
        let path = args.get("path").and_then(Value::as_str).unwrap_or("");
        let in_workspace = path_in_workspace(workspace.as_deref(), path);
        if !host {
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
        }
        confine_fs_args(name, &mut args, workspace.as_deref())?;
        Ok((def, invoke, args))
    }
}

fn confine_fs_args(
    name: &str,
    args: &mut Value,
    workspace: Option<&Path>,
) -> Result<(), PipelineError> {
    if !matches!(
        name,
        "fs.read" | "fs.write" | "fs.edit" | "fs.list" | "fs.search" | "fs.patch"
    ) {
        return Ok(());
    }
    let Some(root) = workspace else {
        return Err(PipelineError::Denied {
            name: name.to_owned(),
            reason: "workspace is not configured".to_owned(),
        });
    };
    let raw = args.get("path").and_then(Value::as_str).unwrap_or("");
    if raw.is_empty() && (name == "fs.list" || name == "fs.search") {
        if let Some(obj) = args.as_object_mut() {
            obj.insert("path".to_owned(), Value::String(root.display().to_string()));
        }
        return Ok(());
    }
    let create_parent = name == "fs.write" || name == "fs.edit" || name == "fs.patch";
    let confined = confine_tool_path(root, Path::new(raw), create_parent)?;
    if let Some(obj) = args.as_object_mut() {
        obj.insert(
            "path".to_owned(),
            Value::String(confined.display().to_string()),
        );
    }
    Ok(())
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

/// Resolve `path` under `workspace`, rejecting parent-directory escapes.
pub fn confine_tool_path(
    workspace: &Path,
    path: &Path,
    create_parent: bool,
) -> Result<PathBuf, PipelineError> {
    let base = workspace
        .canonicalize()
        .map_err(|err| PipelineError::PathEscape(err.to_string()))?;
    if path.as_os_str().is_empty() || path == Path::new(".") {
        return Ok(base);
    }
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    if let Ok(canonical) = requested.canonicalize()
        && canonical == base
    {
        return Ok(base);
    }
    let file_name = requested
        .file_name()
        .ok_or_else(|| PipelineError::PathEscape(requested.display().to_string()))?;
    if file_name == Component::ParentDir.as_os_str()
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
