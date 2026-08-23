use ene_plane::Sensitivity;
use ene_plugin_ipc::ToolSpecWire;
use std::fmt;

use crate::builtins::host_spec_for;

/// Who supplies the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    Plugin { plugin_id: String },
    Harness { name: String },
    Mcp { server: String },
}

/// One registered tool. Host-only fields never appear in `schemas()`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub output: serde_json::Value,
    pub side_effects: Vec<String>,
    pub source: ToolSource,
    pub timeout_ms: Option<u32>,
    pub sensitivity: Sensitivity,
    pub category: String,
    pub keywords: Vec<String>,
    pub examples: Vec<String>,
    pub background: bool,
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
                category: host.category,
                keywords: host.keywords,
                examples: host.examples,
                background: spec.background,
            };
        }
        let background = spec.background;
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
            category: spec.category,
            keywords: spec.keywords,
            examples: spec.examples,
            background,
        }
    }

    #[must_use]
    pub fn surface_visible(&self) -> bool {
        self.side_effects.is_empty() || self.name.starts_with("delegate.")
    }

    #[must_use]
    pub fn available_on(&self, layer: Layer) -> bool {
        match layer {
            Layer::Surface => self.surface_visible(),
            Layer::Job => true,
        }
    }

    #[must_use]
    pub fn primary_layer(&self) -> Layer {
        if self.surface_visible() {
            Layer::Surface
        } else {
            Layer::Job
        }
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

impl Layer {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Job => "job",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
