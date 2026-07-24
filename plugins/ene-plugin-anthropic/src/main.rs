//! # ene-plugin-anthropic
//!
//! Anthropic Claude plugin binary for the ene unified plugin system.
//!
//! Exposes the Anthropic Messages API as an LLM provider over the plugin IPC
//! protocol (v3), supporting both SSE streaming and non-streaming chat
//! completions with tool use and vision inputs.

mod convert;
mod plugin;

use plugin::AnthropicPlugin;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = ene_plugin::run_plugin_server(Box::new(AnthropicPlugin)).await {
        tracing::error!(component = "ene-plugin-anthropic", error = %e, "Fatal error");
        std::process::exit(1);
    }
}
