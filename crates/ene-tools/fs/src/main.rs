mod delete;
mod edit;
mod filesystem;
mod patch;
mod patch_parser;
mod permission;
mod provider;
mod read;
mod read_binary;
mod sandbox;
mod search;
mod search_glob;
mod search_grep;
mod shell;
mod shell_platform;
mod undo_manager;
mod undo_tool;
mod write;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::FsToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-fs] Fatal error: {e}");
        std::process::exit(1);
    }
}
