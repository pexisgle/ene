//! # ene-plugin-kokoro
//!
//! Local Kokoro-TTS (ONNX) provider plugin for the ene unified plugin
//! system. Runs the Kokoro-82M ONNX model in-process via `ene-voice`'s
//! ONNX engine and serves synthesis over the plugin IPC.

mod config;
mod plugin;
mod wav;

#[cfg(test)]
mod tests;

use plugin::KokoroPlugin;

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
            Some(std::sync::Arc::new(KokoroPlugin::new())),
            None,
        )
        .with_capability_declarations(plugin::provides(), Vec::new()),
    )
    .await
    {
        tracing::error!(component = "ene-plugin-kokoro", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
