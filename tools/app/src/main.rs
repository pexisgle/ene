//! # ene-tool-app
//!
//! IPC tool binary providing desktop application control:
//! window management, input simulation, and portal overlay.
#![warn(missing_docs)]
#![allow(clippy::unused_async)]

mod action;
mod config;
mod provider;
mod utils;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::AppToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tool-app] Fatal error: {e}");
        std::process::exit(1);
    }
}
