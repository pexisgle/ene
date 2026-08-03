//! # ene-plugin-calendar
//!
//! Plugin binary providing calendar operations: per-calendar read/write
//! permissions, event listing/creation/update/cancellation, and free-slot
//! search — all with write confirmation and privacy-preserving logging.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests use expect/panic for concise failure paths"
    )
)]

/// Action modules for each calendar operation.
pub mod action;
/// Approval gate for write operations.
pub mod approval;
/// Calendar tool lifecycle and provider integration.
pub mod provider;
/// Calendar provider abstraction layer.
pub mod registry;
/// DB schema declaration for calendar tables.
pub mod schema;
/// Shared state for the calendar actions.
pub mod state;
/// DB-backed calendar store.
pub mod store;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::CalendarToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-calendar] Fatal error: {e}");
        std::process::exit(1);
    }
}
