//! Bundled `tool.utility` plugin (core + tool subprotocols).

use ene_plugin_ipc::BuiltinKind;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = ene_registry::run_plugin(BuiltinKind::Utility).await {
        tracing::error!(error = %err, plugin = "tool.utility", "fatal");
        std::process::exit(1);
    }
}
