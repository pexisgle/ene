//! # ene-plugin-openai
//!
//! OpenAI-compatible provider plugin binary for the ene unified plugin system.
//!
//! Exposes the OpenAI Chat Completions and Embeddings APIs as provider
//! traits over the plugin IPC protocol, supporting SSE streaming, tool use,
//! vision inputs, structured output, and batch embeddings. Any
//! OpenAI-compatible endpoint (OpenAI, OpenRouter, local servers) can be
//! targeted via the `base_url` configuration.

mod convert;
mod plugin;

use plugin::OpenAiPlugin;

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
        Some(std::sync::Arc::new(OpenAiPlugin)),
        Some(std::sync::Arc::new(OpenAiPlugin)),
        None,
        None,
    ))
    .await
    {
        tracing::error!(component = "ene-plugin-openai", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
