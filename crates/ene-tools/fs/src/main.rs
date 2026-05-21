use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = ene_tools_fs::provider::FsToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-fs] Fatal error: {e}");
        std::process::exit(1);
    }
}