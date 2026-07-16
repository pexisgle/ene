//! # ene-tool-web
//!
//! IPC tool binary providing web fetch and search capabilities.
#![warn(missing_docs)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "websearch result ranking uses intentional score arithmetic"
)]

mod action;
mod provider;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::WebToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tool-web] Fatal error: {e}");
        std::process::exit(1);
    }
}
