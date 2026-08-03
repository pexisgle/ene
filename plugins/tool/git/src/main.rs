//! # ene-plugin-git
//!
//! Plugin binary providing read-only git repository inspection tools:
//! status, diff, log, branch, remote, and blame.
#![warn(missing_docs)]
#![expect(
    clippy::unused_async,
    reason = "tool IPC handlers are async for uniform provider dispatch; git2 calls are synchronous"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "unit tests use unwrap/expect for concise failure paths"
    )
)]

/// Action modules for each git tool.
pub mod action;
/// Shared error type and `ToolError` mapping.
pub mod error;
/// JSON output structs and date formatting.
pub mod output;
/// Tool lifecycle and provider integration.
pub mod provider;
/// Workspace path validation for repository access.
pub mod sandbox;

#[cfg(test)]
pub(crate) mod fixture;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::GitToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-git] Fatal error: {e}");
        std::process::exit(1);
    }
}
