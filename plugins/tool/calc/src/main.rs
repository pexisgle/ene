//! # ene-plugin-calc
//!
//! Plugin binary providing calculation tools:
//! math expression evaluation, unit conversion, currency conversion,
//! and color format conversion.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "evaluation and conversion tools perform intentional f64 arithmetic"
)]
#![expect(
    clippy::unused_async,
    reason = "tool IPC handlers are async for uniform provider dispatch"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        reason = "unit tests use unwrap for concise failure paths"
    )
)]

pub mod action;
/// Host-mediated network broker session.
pub mod broker;
pub mod provider;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::CalcToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-calc] Fatal error: {e}");
        std::process::exit(1);
    }
}
