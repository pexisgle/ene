//! # ene-plugin-edge-tts
//!
//! Microsoft Edge Neural Voice (Edge-TTS) provider plugin for the ene
//! unified plugin system. Talks to Microsoft's free, keyless Edge Read
//! Aloud WebSocket endpoint and returns WAV audio over the plugin IPC.

mod audio;
mod client;
mod config;
mod error;
mod plugin;
mod protocol;
mod ssml;

#[cfg(test)]
mod tests;

use plugin::EdgeTtsPlugin;

#[tokio::main]
async fn main() {
    // tokio-tungstenite's rustls backend leaves the crypto provider
    // selection to the application; ring is the workspace's provider.
    rustls::crypto::ring::default_provider()
        .install_default()
        .unwrap_or_default();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = ene_plugin::run_plugin_server(ene_plugin::PluginDispatch::new(
        None,
        None,
        None,
        Some(std::sync::Arc::new(EdgeTtsPlugin)),
        None,
    ))
    .await
    {
        tracing::error!(component = "ene-plugin-edge-tts", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
