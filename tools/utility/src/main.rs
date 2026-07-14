//! # ene-tool-utility
//!
//! IPC tool binary providing utility operations:
//! question prompting, todo list management, time, and system info.
#![warn(missing_docs)]
#![allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::option_option
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Action modules for each tool.
pub mod action;
/// Tool lifecycle and provider integration.
pub mod provider;
/// DB schema declaration for utility tables.
pub mod schema;
/// DB-backed todo store.
pub mod todo_store;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::UtilityToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tool-utility] Fatal error: {e}");
        std::process::exit(1);
    }
}
