//! # ene-plugin-utility
//!
//! Plugin binary providing utility operations:
//! question prompting, todo list management, time, and system info.
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

pub mod action;
pub mod provider;
pub mod result;
pub mod schema;
pub mod task_registry;
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
