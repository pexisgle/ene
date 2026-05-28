//! # ene-tools-app
//!
//! IPC tool binary providing desktop application control:
//! window management, input simulation, and portal overlay.
#![warn(missing_docs)]

mod actions;
mod portal;
mod provider;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::AppToolProvider;
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-app] Fatal error: {e}");
        std::process::exit(1);
    }
}
