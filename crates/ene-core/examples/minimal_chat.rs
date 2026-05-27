//! Minimal chat example using ene-core.
//!
//! Initializes the AI runtime, loads a character card, and runs a single-turn
//! streaming conversation with tool support.
//!
//! Requires a valid `assets/settings.json` with LLM provider configuration.

use ene_config::load_full_settings;
use ene_core::{EneRuntime, EneStreamEvent, run_ene_with_tools};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let settings = load_full_settings();
    let mut runtime = EneRuntime::init(settings).await?;

    let user_input = "Hello! What's your name?";
    let _ = runtime.embed_input(user_input).await?;

    let stream = run_ene_with_tools(
        &runtime.settings,
        &runtime.session,
        user_input,
        runtime.registry.clone(),
    )
    .await?;
    tokio::pin!(stream);

    while let Some(event) = stream.next().await {
        match event {
            EneStreamEvent::TextDelta(delta) => print!("{}", delta),
            EneStreamEvent::ToolCallStart { name, arguments } => {
                println!("\n[Tool: {} with {}]", name, arguments);
            }
            EneStreamEvent::ToolCallResult { name, result } => {
                println!("[{} -> {}]", name, &result[..result.len().min(200)]);
            }
            EneStreamEvent::Finished => println!("\n[Done]"),
            EneStreamEvent::Error(err) => eprintln!("\nError: {}", err),
            _ => {}
        }
    }

    Ok(())
}
