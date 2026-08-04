//! # ene-plugin-counter
//!
//! Sample tool plugin demonstrating the tool SDK end to end:
//! derive-based action schemas, stateful DB IPC, and a
//! permission-gated destructive action.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        reason = "unit tests use unwrap for concise failure paths"
    )
)]

/// Action definitions.
pub mod action;
/// Permission approval gate.
pub mod approval;
/// Tool lifecycle and provider integration.
pub mod provider;
/// DB schema declaration for the counter table.
pub mod schema;
/// Counter storage backend (DB-backed plus an in-memory test double).
pub mod store;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::CounterToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-counter] Fatal error: {e}");
        std::process::exit(1);
    }
}
