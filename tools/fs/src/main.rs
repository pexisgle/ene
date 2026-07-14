//! # ene-tool-fs
//!
//! IPC tool binary providing filesystem operations:
//! read, write, edit, search, glob, patch, shell execution, and undo management.
#![expect(dead_code)]
#![warn(missing_docs)]
#![allow(
    clippy::unused_async,
    clippy::unused_async_trait_impl,
    clippy::unused_peekable,
    clippy::option_if_let_else
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod action;
mod provider;
mod schema;
mod undo;
mod utils;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::FsToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tool-fs] Fatal error: {e}");
        std::process::exit(1);
    }
}
