//! # ene-plugin-counter
//!
//! Sample tool plugin demonstrating the tool SDK end to end:
//! derive-based action schemas, stateful DB IPC, and a
//! permission-gated destructive action.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        reason = "unit tests use unwrap for concise failure paths"
    )
)]

pub mod action;
pub mod approval;
pub mod provider;
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
