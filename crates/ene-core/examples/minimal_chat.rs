//! Minimal chat example using ene-core with the actor-based runtime.
//!
//! Initializes the actor, loads settings, and runs a single-turn
//! streaming conversation with tool support.

use ene_config;
use ene_core::{EneEvent, EneHandle, PermissionDecision};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let handle = EneHandle::new();

    let config = ene_config::load_config();
    handle.reconfigure(config).await?;
    handle
        .load_character("characters/Alicia/character.json")
        .await?;

    let _ = handle.run("Hello! What's your name?");

    let mut rx = handle.subscribe();
    while let Ok(event) = tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv()).await
    {
        match event {
            Ok(EneEvent::TextDelta { delta }) => print!("{}", delta),
            Ok(EneEvent::ToolCallStart { name, arguments }) => {
                println!("\n[Tool: {} with {}]", name, arguments);
            }
            Ok(EneEvent::ToolCallResult { name, result }) => {
                println!("[{} -> {}]", name, &result[..result.len().min(200)]);
            }
            Ok(EneEvent::Done) => {
                println!("\n[Done]");
                break;
            }
            Ok(EneEvent::Failed { message }) => {
                eprintln!("\nError: {}", message);
                break;
            }
            Ok(EneEvent::PermissionRequired {
                request_id,
                action,
                target,
                description,
            }) => {
                println!(
                    "\n[Permission Required] {} on {} ({})",
                    action, target, description
                );
                let _ = handle.decide_permission(request_id, PermissionDecision::AllowOnce);
            }
            _ => {}
        }
    }

    Ok(())
}
