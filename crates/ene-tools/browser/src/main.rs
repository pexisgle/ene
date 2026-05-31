//! # ene-tools-browser
//!
//! IPC tool binary providing browser automation capabilities:
//! Chrome DevTools Protocol integration for web scraping and interaction.
#![warn(missing_docs)]

mod action;
mod chrome;
mod extract;
mod provider;
mod session;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::BrowserToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-browser] Fatal error: {e}");
        std::process::exit(1);
    }
}
