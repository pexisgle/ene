//! Bundled `tool.exec` plugin, split from `fs` (D-24).

use ene_plugin_ipc::BuiltinKind;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = ene_registry::run_plugin(BuiltinKind::Exec).await {
        tracing::error!(error = %err, plugin = "tool.exec", "fatal");
        std::process::exit(1);
    }
}
