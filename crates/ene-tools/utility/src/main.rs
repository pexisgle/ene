pub mod provider;
pub mod question;
pub mod todo;

use ene_tool_proto::run_tool_server;

#[tokio::main]
async fn main() {
    let provider = provider::UtilityToolProvider::new();
    if let Err(e) = run_tool_server(Box::new(provider)).await {
        eprintln!("[ene-tools-utility] Fatal error: {e}");
        std::process::exit(1);
    }
}
