//! # ene-plugin-utility
//!
//! Plugin binary providing utility operations:
//! question prompting, todo list management, time, and system info.
#![warn(missing_docs)]
#![expect(
    clippy::unused_async,
    clippy::option_option,
    reason = "tool IPC handlers are async for uniform provider dispatch; nested options match schema"
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "question numbering uses simple counter arithmetic"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "unit tests use unwrap and fixed option indices"
    )
)]

/// Action modules for each tool.
pub mod action;
/// Tool lifecycle and provider integration.
pub mod provider;
/// Shared result formatting helpers.
pub mod result;
/// DB schema declaration for utility tables.
pub mod schema;
/// Background task registry for timers and notifications.
pub mod task_registry;
/// DB-backed todo store.
pub mod todo_store;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::UtilityToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-utility] Fatal error: {e}");
        std::process::exit(1);
    }
}
