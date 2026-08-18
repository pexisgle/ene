//! `VOICEVOX`-compatible engine TTS (`provider.voicevox`).
//!
//! Talks HTTP to a user-run engine (`VOICEVOX` :50021, Aivis Speech :10101).
//! The host does not vendor the engine binary.

mod client;

use std::sync::Arc;

use client::Voicevox;
use ene_plugin_ipc::{PluginIdentity, ProviderHandlers, serve_provider_from_env};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let provider = Arc::new(Voicevox::new());
    let handlers = ProviderHandlers {
        tts: Some(provider),
        ..ProviderHandlers::default()
    };
    if let Err(err) = serve_provider_from_env(identity(), handlers).await {
        tracing::error!(error = %err, plugin = "provider.voicevox", "fatal");
        std::process::exit(1);
    }
}

fn identity() -> PluginIdentity {
    PluginIdentity {
        plugin_id: "provider.voicevox".to_owned(),
        plugin_name: "voicevox".to_owned(),
        digest: exe_digest(),
        spawn_token: None,
    }
}

fn exe_digest() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .map_or_else(
            || "blake3:unknown".to_owned(),
            |bytes| format!("blake3:{}", blake3::hash(&bytes).to_hex()),
        )
}
