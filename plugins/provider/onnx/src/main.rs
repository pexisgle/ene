//! # ene-plugin-onnx
//!
//! Local ONNX provider plugin: serves the Silero VAD engine over the plugin
//! IPC and declares the `onnx-runner@1` / `g2p/en@1` capabilities for the
//! host's capability registry.

mod config;
mod plugin;

use plugin::OnnxPlugin;

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
        ene_plugin::PluginDispatch::new(None, None, None, None, None)
            .with_vad(std::sync::Arc::new(OnnxPlugin::new()))
            .with_capability_declarations(plugin::provides(), plugin::requires()),
    )
    .await
    {
        tracing::error!(
            component = "ene-plugin-onnx",
            error = %e,
            "Fatal error"
        );
        std::process::exit(1);
    }
}
