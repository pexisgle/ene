//! Unified tool registry. Surface schemas include only empty `side_effects`
//! (plus `delegate.*`). Side-effect tools are deny-by-default until W2.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

mod builtins;
mod def;
mod pipeline;

pub use builtins::{
    BuiltinExecutor, BuiltinHandler, builtin_digest, builtin_specs, definitions_for, run_plugin,
};
pub use def::{Layer, ToolDefinition, ToolSource};
pub use pipeline::{BuiltinInvoker, PipelineError, ToolInvoke, ToolRegistry};

#[cfg(test)]
mod tests;
