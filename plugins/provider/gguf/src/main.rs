//! Local GGUF via host-managed `llama-server` (`provider.gguf`).
//!
//! The host spawns the loopback sidecar and injects `sidecar_base_url`; this
//! plugin maps host-canonical messages onto `/v1` chat completions and embeddings.

#![cfg_attr(
    test,
    expect(clippy::expect_used, clippy::unwrap_used, reason = "tests fail fast")
)]

mod assets;
mod client;
mod sidecar;
mod stream;

use std::sync::Arc;

use assets::GgufAssets;
use client::Gguf;
use ene_plugin_ipc::{PluginIdentity, ProviderHandlers, serve_provider_from_env};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let sidecar = sidecar::maybe_start();
    let provider = Arc::new(Gguf::new());
    let assets = Arc::new(GgufAssets::new());
    let handlers = ProviderHandlers {
        llm: Some(provider.clone()),
        embed: Some(provider.clone()),
        models: Some(provider),
        assets: Some(assets),
        ..ProviderHandlers::default()
    };
    if let Err(err) = serve_provider_from_env(identity(), handlers).await {
        tracing::error!(error = %err, plugin = "provider.gguf", "fatal");
        drop(sidecar);
        std::process::exit(1);
    }
    drop(sidecar);
}

fn identity() -> PluginIdentity {
    PluginIdentity {
        plugin_id: "provider.gguf".to_owned(),
        plugin_name: "gguf".to_owned(),
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
