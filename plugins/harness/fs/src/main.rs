//! Bundled `tool.fs` plugin. Shell execution lives in `tool.exec` (D-24).

use ene_plugin_ipc::BuiltinKind;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = ene_registry::run_plugin(BuiltinKind::Fs).await {
        tracing::error!(error = %err, plugin = "tool.fs", "fatal");
        std::process::exit(1);
    }
}
