//! # ene-tool-utility
//!
//! IPC tool binary providing utility operations:
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
