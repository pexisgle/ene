//! Bundled `tool.utility` plugin: hash, time, calc, random, text.

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
        ene_registry::run_tool_plugin("tool.utility", logic::specs, logic::execute).await
    {
        tracing::error!(error = %err, plugin = "tool.utility", "fatal");
        std::process::exit(1);
    }
}
