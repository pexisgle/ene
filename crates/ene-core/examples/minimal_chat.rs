//! Minimal chat example using ene-core.
//!
//! Initializes the AI runtime, loads a character card, and runs a single-turn
//! streaming conversation with tool support.
//!
//! Requires a valid `assets/settings.json` with LLM provider configuration.

use ene_core::{AiRuntime, AiStreamEvent, run_ai_with_tools};
use ene_config::load_full_settings;
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let settings = load_full_settings()?;
    let mut runtime = AiRuntime::init(settings).await?;

    let user_input = "Hello! What's your name?";
    let _ = runtime.embed_input(user_input).await?;

    let mut stream = run_ai_with_tools(
        &runtime.settings,
        &runtime.session,
        user_input,
        runtime.registry.clone(),
    )
    .await?;

    while let Some(event) = stream.next().await {
        match event {
            AiStreamEvent::TextDelta(delta) => print!("{}", delta),
            AiStreamEvent::ToolCallStart { name, arguments } => {
                println!("\n[Tool: {} with {}]", name, arguments);
            }
            AiStreamEvent::ToolCallResult { name, result } => {
                println!("[{} -> {}]", name, &result[..result.len().min(200)]);
            }
            AiStreamEvent::Finished => println!("\n[Done]"),
            AiStreamEvent::Error(err) => eprintln!("\nError: {}", err),
            _ => {}
        }
    }

    Ok(())
}
