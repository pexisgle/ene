//! # ene-plugin-random
//!
//! Plugin binary providing random generation tools: random numbers,
//! UUID v4 values, list picks, and hex colors.
#![warn(missing_docs)]
#![expect(
    clippy::unused_async,
    reason = "tool IPC handlers are async for uniform provider dispatch"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        reason = "unit tests use unwrap for concise failure paths"
    )
)]

/// Action modules for each tool.
pub mod action;
/// Tool lifecycle and provider integration.
pub mod provider;

mod error;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::RandomToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-random] Fatal error: {e}");
        std::process::exit(1);
    }
}
