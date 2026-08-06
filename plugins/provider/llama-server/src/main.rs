//! # ene-plugin-llama-server
//!
//! Local GGUF inference provider plugin backed by an external `llama-server`
//! sidecar process.
//!
//! Serves chat streaming, non-streaming completion, and GGUF embeddings over
//! the plugin IPC, translating each request to the OpenAI-compatible HTTP API
//! of a managed `llama-server` (router mode). The sidecar is spawned on a
//! loopback random port, health-checked before use, and killed when the
//! plugin exits; the plugin itself compiles no llama.cpp runtime, so the
//! inference engine can be updated by replacing the sidecar binary alone.
//!
//! Capability declarations (`llm/chat@1`, `embed@1`, `gguf-runner@1`) and the
//! config schema (`server_path` / `server_args` / `startup_timeout_secs` /
//! `mmproj_url` / `mmproj_path` / `acceleration`) mirror the in-process
//! `ene-plugin-llama-cpp` plugin so the two can be swapped without touching
//! host routing.
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        reason = "unit tests in the plugin modules use expect for assertions"
    )
)]

mod client;
mod config;
mod convert;
mod gguf;
mod models;
mod plugin;
mod server;

use plugin::LlamaServerPlugin;

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
            Some(std::sync::Arc::new(LlamaServerPlugin)),
            Some(std::sync::Arc::new(LlamaServerPlugin)),
            None,
            None,
        )
        .with_capability_declarations(LlamaServerPlugin::provides(), Vec::new())
        .with_capability_provider(std::sync::Arc::new(LlamaServerPlugin)),
    )
    .await
    {
        tracing::error!(
            component = "ene-plugin-llama-server",
            error = %e,
            "Fatal error"
        );
        std::process::exit(1);
    }
}
