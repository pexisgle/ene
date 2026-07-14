//! # ene-tool-browser
//!
//! IPC tool binary providing browser automation capabilities:
//! Chrome `DevTools` Protocol integration for web scraping and interaction.
#![warn(missing_docs)]
#![allow(clippy::unused_async, clippy::unused_async_trait_impl)]

mod action;
mod provider;
mod utils;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::BrowserToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tool-browser] Fatal error: {e}");
        std::process::exit(1);
    }
}
