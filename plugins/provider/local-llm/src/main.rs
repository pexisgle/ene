//! # ene-plugin-llama-cpp
//!
//! Local GGUF inference provider plugin for the ene unified plugin system.
//!
//! Serves chat streaming, non-streaming completion, and GGUF embeddings over
//! the plugin IPC, backed by the plugin's llama.cpp engines. Capability
//! declarations (`llm/chat@1`, `embed@1`, `gguf-runner@1`) and the config
//! schema (`mmproj_url` / `mmproj_path` / `acceleration`) are established in
//! an earlier slice; the inference core lives in the `embedding` / `gguf` /
//! `llama_cpp` / `local_llm` modules.
#![cfg_attr(
    test,
    expect(
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests in the inference modules use expect/panic for assertions"
    )
)]

mod config;
mod convert;
mod embedding;
mod gguf;
mod llama_cpp;
mod local_llm;
mod models;
mod plugin;

use plugin::LocalLlmPlugin;

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
            Some(std::sync::Arc::new(LocalLlmPlugin)),
            Some(std::sync::Arc::new(LocalLlmPlugin)),
            None,
            None,
        )
        .with_capability_declarations(LocalLlmPlugin::provides(), Vec::new()),
    )
    .await
    {
        tracing::error!(
            component = "ene-plugin-llama-cpp",
            error = %e,
            "Fatal error"
        );
        std::process::exit(1);
    }
}
