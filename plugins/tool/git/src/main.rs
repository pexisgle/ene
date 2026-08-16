//! # ene-plugin-git
//!
//! Plugin binary providing read-only git repository inspection tools:
//! status, diff, log, branch, remote, and blame.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::await_holding_lock,
        reason = "unit tests use unwrap/expect for concise failure paths"
    )
)]

pub mod action;
/// Host-mediated process broker session.
pub mod broker;
pub mod error;
pub mod output;
pub mod provider;
pub mod sandbox;

#[cfg(test)]
pub(crate) mod fixture;

use ene_plugin::{PluginDispatch, ToolProviderPlugin, run_plugin_server};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let provider = provider::GitToolProvider::new();
    if let Err(e) = run_plugin_server(PluginDispatch::new(
        Some(Arc::new(ToolProviderPlugin::new(provider))),
        None,
        None,
        None,
        None,
    ))
    .await
    {
        tracing::error!("[ene-plugin-git] Fatal error: {e}");
        std::process::exit(1);
    }
}
