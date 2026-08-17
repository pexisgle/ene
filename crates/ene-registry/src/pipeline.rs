use crate::BuiltinExecutor;
use crate::def::{Layer, ToolDefinition, ToolSource};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
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
    #[error("execute: {0}")]
    Execute(String),
}

/// In-memory registry. Fiber unload removes rows by plugin id.
pub struct ToolRegistry {
    tools: Mutex<HashMap<String, Registered>>,
}

impl ToolRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Mutex::new(HashMap::new()),
        }
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
        args: Value,
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
        if !def.side_effects.is_empty() {
            return Err(PipelineError::Denied {
                name: name.to_owned(),
                reason: "deny-by-default until approval plane".to_owned(),
            });
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
