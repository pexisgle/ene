//! `VOICEVOX`-compatible engine TTS (`provider.voicevox`).
//!
//! Talks HTTP to a user-run engine (`VOICEVOX` :50021, Aivis Speech :10101)
//! or to a sidecar spawned from `server_path`.

#![cfg_attr(test, expect(clippy::expect_used, reason = "tests fail fast"))]

mod client;
mod sidecar;

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
    let sidecar = match sidecar::maybe_start() {
        Ok(guard) => guard,
        Err(err) => {
            tracing::error!(error = %err, plugin = "provider.voicevox", "sidecar");
            std::process::exit(1);
        }
    };
    let provider = Arc::new(Voicevox::new());
    let handlers = ProviderHandlers {
        tts: Some(provider),
        ..ProviderHandlers::default()
    };
    if let Err(err) = serve_provider_from_env(identity(), handlers).await {
        tracing::error!(error = %err, plugin = "provider.voicevox", "fatal");
        drop(sidecar);
        std::process::exit(1);
    }
    drop(sidecar);
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
