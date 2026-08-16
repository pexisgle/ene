//! # ene-plugin-geo
//!
//! Plugin binary providing geographic information tools:
//! IP-based location, current weather, timezone offset calculation,
//! and sunrise/sunset times.
#![expect(
    clippy::unused_async,
    reason = "tool IPC handlers are async for uniform provider dispatch"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests use unwrap/expect/panic for concise failure paths"
    )
)]

pub mod action;
pub mod approval;
/// Host-mediated network broker session.
pub mod broker;
pub mod error;
pub mod provider;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::GeoToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-geo] Fatal error: {e}");
        std::process::exit(1);
    }
}
