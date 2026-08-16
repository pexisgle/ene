//! # ene-plugin-app
//!
//! Plugin binary providing desktop application control:
//! window management, input simulation, and portal overlay.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "app portal/input tools use intentional coordinate and timing arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "key combo and capture helpers index into validated buffers"
)]
#![expect(
    clippy::string_slice,
    reason = "portal compositor parsers slice at ASCII delimiters"
)]

mod action;
mod config;
mod provider;
mod utils;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::AppToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-app] Fatal error: {e}");
        std::process::exit(1);
    }
}
