//! # ene-tools-web
//!
//! IPC tool binary providing web fetch and search capabilities.
#![warn(missing_docs)]

mod config;
mod provider;
mod webfetch;
mod websearch;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::WebToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-web] Fatal error: {e}");
        std::process::exit(1);
    }
}
