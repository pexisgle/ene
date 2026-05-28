//! Minimal chat example using ene-core.
//!
//! Initializes the AI runtime, loads settings, and runs a single-turn
//! streaming conversation with tool support.

use ene_core::{EneRuntime, EneStreamEvent};
use tokio_stream::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut runtime = EneRuntime::init().await?;
    runtime.config().load().await?;
    runtime.character().load()?;

    let user_input = "Hello! What's your name?";
    let stream = runtime.run(user_input).await?;
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
