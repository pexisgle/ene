//! Bundled `tool.exec` plugin: process execution, separate from `tool.fs`.

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
        ene_tool_registry::run_tool_plugin("tool.exec", logic::specs, logic::execute).await
    {
        tracing::error!(error = %err, plugin = "tool.exec", "fatal");
        std::process::exit(1);
    }
}
