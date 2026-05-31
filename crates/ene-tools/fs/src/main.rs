//! # ene-tools-fs
//!
//! IPC tool binary providing filesystem operations:
//! read, write, edit, search, glob, patch, shell execution, and undo management.
#![allow(dead_code)]
#![warn(missing_docs)]

mod action;
mod filesystem;
mod permission;
mod provider;
mod sandbox;
mod schema;
mod undo_manager;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::FsToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-fs] Fatal error: {e}");
        std::process::exit(1);
    }
}
