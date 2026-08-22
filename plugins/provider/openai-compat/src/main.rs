//! OpenAI-compatible HTTP provider (`provider.openai_compat`).
//!
//! Speaks the provider subprotocol and maps host-canonical messages onto
//! remote `/v1` chat, embeddings, TTS, and STT. Local GGUF is `provider.gguf`.

#![cfg_attr(test, expect(clippy::unwrap_used, reason = "tests fail fast"))]

mod client;
mod stream;

use std::sync::Arc;

use client::OpenAiCompat;
use ene_plugin_ipc::{PluginIdentity, ProviderHandlers, serve_provider_from_env};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let provider = Arc::new(OpenAiCompat::new());
    let handlers = ProviderHandlers {
        llm: Some(provider.clone()),
        embed: Some(provider.clone()),
        tts: Some(provider.clone()),
        stt: Some(provider.clone()),
        models: Some(provider),
        ..ProviderHandlers::default()
    };
    if let Err(err) = serve_provider_from_env(identity(), handlers).await {
        tracing::error!(error = %err, plugin = "provider.openai_compat", "fatal");
        std::process::exit(1);
    }
}

fn identity() -> PluginIdentity {
    PluginIdentity {
        plugin_id: "provider.openai_compat".to_owned(),
        plugin_name: "openai_compat".to_owned(),
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
