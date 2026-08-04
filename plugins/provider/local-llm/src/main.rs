//! # ene-plugin-llama-cpp
//!
//! Local GGUF inference provider plugin for the ene unified plugin system.
//!
//! Serves chat streaming, non-streaming completion, and GGUF embeddings over
//! the plugin IPC, backed by `ene-ai-local`'s llama.cpp providers. Capability
//! declarations (`llm/chat@1`, `embed@1`, `gguf-runner@1`) and the config
//! schema (`mmproj_url` / `mmproj_path` / `acceleration`) are established in
//! an earlier slice; this slice implements the inference actions.

mod config;
mod convert;
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
