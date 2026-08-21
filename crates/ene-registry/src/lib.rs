//! Unified tool registry. Surface schemas include only empty `side_effects`
//! (plus `delegate.*`). Side-effect tools are deny-by-default until W2.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

extern crate self as ene_registry;

mod builtin;
mod builtins;
mod def;
mod pipeline;

pub use builtin::{arg_str, spec};
pub use builtins::{
    BuiltinExecutor, builtin_digest, builtin_specs, definitions_for, file_digest, host_sensitivity,
    host_spec_for, run_plugin, run_tool_plugin,
};
pub use def::{Layer, ToolDefinition, ToolSource};
pub use pipeline::{BuiltinInvoker, PipelineError, ToolInvoke, ToolRegistry, confine_tool_path};

#[cfg(test)]
mod tests;
