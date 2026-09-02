//! Bundled `tool.app` plugin: screenshot, windows, clipboard, input.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]

mod logic;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) =
        ene_tool_registry::run_tool_plugin("tool.app", logic::specs, logic::execute).await
    {
        tracing::error!(error = %err, plugin = "tool.app", "fatal");
        std::process::exit(1);
    }
}
