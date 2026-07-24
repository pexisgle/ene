//! # ene-plugin-fs
//!
//! Plugin binary providing filesystem operations:
//! read, write, edit, search, glob, patch, shell execution, and undo management.
#![warn(missing_docs)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "fs edit/patch/read tools use intentional byte and line-index arithmetic"
)]
#![expect(
    clippy::indexing_slicing,
    reason = "edit/patch matchers index into pre-validated line/byte ranges"
)]
#![expect(
    clippy::string_slice,
    reason = "edit strategies slice at ASCII/byte offsets from prior finds"
)]
#![expect(
    dead_code,
    reason = "tool binary re-exports modules for IPC provider wiring"
)]
#![expect(
    clippy::unused_async,
    clippy::unused_peekable,
    clippy::option_if_let_else,
    reason = "tool IPC handlers and path helpers favor local clarity over nursery lints"
)]
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "unit tests use unwrap/expect for concise failure paths"
    )
)]

mod action;
mod provider;
mod schema;
mod undo;
mod utils;

use ene_plugin::{ToolPluginAdapter, run_plugin_server};

#[tokio::main]
async fn main() {
    let provider = provider::FsToolProvider::new();
    if let Err(e) = run_plugin_server(Box::new(ToolPluginAdapter(provider))).await {
        eprintln!("[ene-plugin-fs] Fatal error: {e}");
        std::process::exit(1);
    }
}
