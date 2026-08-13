//! # ene-plugin-whisper
//!
//! Local whisper.cpp STT provider plugin: serves transcription over the
//! plugin IPC and declares the `whisper-runner@1` capability for the host's
//! capability registry.

mod config;
mod plugin;
mod server;
mod wav;

use plugin::WhisperPlugin;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = ene_plugin::run_plugin_server(
        ene_plugin::PluginDispatch::new(
            None,
            None,
            None,
            None,
            Some(std::sync::Arc::new(WhisperPlugin::new())),
        )
        .with_capability_declarations(plugin::provides(), Vec::new()),
    )
    .await
    {
        tracing::error!(
            component = "ene-plugin-whisper",
            error = %e,
            "Fatal error"
        );
        std::process::exit(1);
    }
}
