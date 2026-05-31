//! # ene-tools-utility
//!
//! IPC tool binary providing utility operations:
//! question prompting, todo list management, time, and system info.
#![warn(missing_docs)]

/// Tool lifecycle and provider integration.
pub mod provider;
/// Action modules for each tool.
pub mod action;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::UtilityToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-utility] Fatal error: {e}");
        std::process::exit(1);
    }
}
