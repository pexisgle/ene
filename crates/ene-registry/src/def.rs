use ene_plane::Sensitivity;
use ene_plugin_ipc::ToolSpecWire;

use crate::builtins::host_spec_for;

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
    pub parameters: serde_json::Value,
    pub output: serde_json::Value,
    pub side_effects: Vec<String>,
    pub source: ToolSource,
    pub timeout_ms: Option<u32>,
    pub sensitivity: Sensitivity,
}

impl ToolDefinition {
    #[must_use]
    pub fn from_wire(spec: ToolSpecWire, source: ToolSource) -> Self {
        if let Some(host) = host_spec_for(&spec.name) {
            return Self {
                name: spec.name.clone(),
                description: host.description,
                parameters: host.parameters,
                output: host.output,
                side_effects: host.side_effects,
                source,
                timeout_ms: None,
                sensitivity: crate::builtins::host_sensitivity(&spec.name),
            };
        }
        let side_effects = spec.side_effects;
        let sensitivity = if side_effects.is_empty() {
            Sensitivity::Medium
        } else {
            Sensitivity::None
        };
        Self {
            name: spec.name,
            description: spec.description,
            parameters: spec.parameters,
            output: spec.output,
            side_effects,
            source,
            timeout_ms: None,
            sensitivity,
        }
    }

    #[must_use]
    pub fn surface_visible(&self) -> bool {
        self.side_effects.is_empty() || self.name.starts_with("delegate.")
    }

    #[must_use]
    pub fn model_schema(&self) -> serde_json::Value {
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
