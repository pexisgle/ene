//! # ene-plugin-web
//!
//! Plugin binary providing web fetch and search capabilities.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "search result ranking uses intentional score arithmetic"
)]
#![expect(
    clippy::print_stderr,
    reason = "standalone plugin process reports a fatal startup error to stderr before exit"
)]

mod action;
mod broker;
mod provider;
mod search;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::WebToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        eprintln!("[ene-plugin-web] Fatal error: {e}");
        std::process::exit(1);
    }
}
