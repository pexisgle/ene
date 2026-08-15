//! # ene-plugin-elevenlabs
//!
//! `ElevenLabs` TTS provider plugin for the ene unified plugin system.
//! Synthesizes speech over the `/text-to-speech/{voice_id}/stream` REST
//! endpoint (broker-mediated), requesting `pcm_{rate}` and returning WAV
//! over the plugin IPC.

mod broker;
mod client;
mod config;
mod pcm;
mod plugin;
mod wav;

#[cfg(test)]
mod mock_server;
#[cfg(test)]
mod tests;

use plugin::ElevenLabsPlugin;

#[tokio::main]
async fn main() {
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
        Some(std::sync::Arc::new(ElevenLabsPlugin::default())),
        None,
    ))
    .await
    {
        tracing::error!(component = "ene-plugin-elevenlabs", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
