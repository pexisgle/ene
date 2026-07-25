//! # ene-plugin-web
//!
//! Plugin binary providing web fetch and search capabilities.
#![warn(missing_docs)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "search result ranking uses intentional score arithmetic"
)]

mod action;
mod provider;
mod search;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::WebToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin(provider))),
        None,
        None,
    ))
    .await
    {
        eprintln!("[ene-plugin-web] Fatal error: {e}");
        std::process::exit(1);
    }
}
