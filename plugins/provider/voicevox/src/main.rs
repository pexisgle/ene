//! # ene-plugin-voicevox
//!
//! VOICEVOX / Aivis Speech-compatible TTS provider plugin for the ene
//! unified plugin system. Talks to a local VOICEVOX-compatible engine over
//! its HTTP API (2-step `audio_query` → `synthesis` flow) and optionally
//! spawns and supervises the engine binary itself (managed mode).

mod client;
mod config;
mod engine;
mod plugin;

#[cfg(test)]
mod mock_engine;
#[cfg(test)]
mod tests;

use plugin::VoicevoxPlugin;

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
        Some(std::sync::Arc::new(VoicevoxPlugin::default())),
        None,
    ))
    .await
    {
        tracing::error!(component = "ene-plugin-voicevox", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
