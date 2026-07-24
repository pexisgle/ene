//! # ene-plugin-browser
//!
//! Plugin binary providing browser automation capabilities:
//! Chrome `DevTools` Protocol integration for web scraping and interaction.
#![warn(missing_docs)]

mod action;
mod provider;
mod utils;

use ene_plugin::{ToolPluginAdapter, run_plugin_server};

#[tokio::main]
async fn main() {
    let provider = provider::BrowserToolProvider::new();
    let store = provider.store.clone();
    let result = run_plugin_server(Box::new(ToolPluginAdapter(provider))).await;
    store.shutdown().await;
    if let Err(e) = result {
        tracing::error!("[ene-plugin-browser] Fatal error: {e}");
        std::process::exit(1);
    }
}
