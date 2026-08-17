use ene_plugin_ipc::ToolSpecWire;
use serde_json::Value;

/// Who supplies the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    Plugin { plugin_id: String },
    Harness { name: String },
    Mcp { server: String },
}

/// One registered tool. Host-only fields never appear in `schemas()`.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub output: Value,
    pub side_effects: Vec<String>,
    pub source: ToolSource,
    pub timeout_ms: Option<u32>,
}

impl ToolDefinition {
    #[must_use]
    pub fn from_wire(spec: ToolSpecWire, source: ToolSource) -> Self {
        Self {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
            output: spec.output,
            side_effects: spec.side_effects,
            source,
            timeout_ms: None,
        }
    }

    #[must_use]
    pub fn surface_visible(&self) -> bool {
        self.side_effects.is_empty() || self.name.starts_with("delegate.")
    }

    #[must_use]
    pub fn model_schema(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "parameters": self.parameters,
        })
    }
}

/// Dialogue vs job layer (D-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Surface,
    Job,
}
