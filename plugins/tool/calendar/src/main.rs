//! # ene-plugin-calendar
//!
//! Plugin binary providing calendar operations: per-calendar read/write
//! permissions, event listing/creation/update/cancellation, and free-slot
//! search — all with write confirmation and privacy-preserving logging.
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests use expect/panic for concise failure paths"
    )
)]

pub mod action;
pub mod approval;
pub mod provider;
pub mod registry;
pub mod schema;
pub mod state;
pub mod store;
#[cfg(test)]
mod test_db;

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
