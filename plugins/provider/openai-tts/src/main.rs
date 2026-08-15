//! # ene-plugin-openai-tts
//!
//! `OpenAI Speech API` (`tts-1` / `tts-1-hd`) TTS provider plugin for the ene
//! unified plugin system. Synthesizes speech via `POST /v1/audio/speech`
//! with `response_format=pcm` (24 kHz 16-bit mono little-endian PCM) and
//! returns the audio as WAV over the plugin IPC.

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

use plugin::OpenAiTtsPlugin;

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
        Some(std::sync::Arc::new(OpenAiTtsPlugin)),
        None,
    ))
    .await
    {
        tracing::error!(component = "ene-plugin-openai-tts", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
