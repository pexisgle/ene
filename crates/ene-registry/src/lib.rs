//! Unified tool registry. Surface schemas include only empty `side_effects`
//! (plus `delegate.*`). Side-effect tools are deny-by-default until W2.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]
#![deny(unsafe_code)]

extern crate self as ene_registry;

mod builtin;
mod builtins;
mod def;
mod discovery;
mod host_http;
mod pipeline;

pub use builtin::{arg_str, spec, spec_with_discovery};
pub use builtins::{
    BuiltinExecutor, builtin_digest, builtin_specs, definitions_for, file_digest, host_sensitivity,
    host_spec_for, run_plugin, run_tool_plugin,
};
pub use def::{Layer, ToolDefinition, ToolSource};
pub use discovery::ToolHit;
pub use host_http::{
    try_host_fetch, try_host_post_json, web_credentials::try_web_credentials,
    web_credentials::with_web_credentials, with_http_fetch, with_post_json,
};
pub use pipeline::{BuiltinInvoker, PipelineError, ToolInvoke, ToolRegistry, confine_tool_path};
pub use pipeline::intent_digest;

#[cfg(test)]
mod tests;
