//! # ene-plugin-homeassistant
//!
//! Plugin binary providing Home Assistant smart home tools: entity state
//! reads, switch/light/plug control, and climate temperature setting.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests use unwrap/expect/panic for concise failure paths"
    )
)]

/// Action modules for each tool.
pub mod action;
/// Permission approval gate for state-changing actions.
pub mod approval;
/// Plugin configuration and schema generation.
pub mod config;
/// Shared error type for parse, format, and validation failures.
pub mod error;
/// Tool lifecycle and provider integration.
pub mod provider;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::HomeAssistantToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-homeassistant] Fatal error: {e}");
        std::process::exit(1);
    }
}
