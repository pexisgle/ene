//! # ene-runtime Minimal Chat Example
//!
//! Demonstrates the API v2 host path:
//! 1. Load config + character card via `ene-config`
//! 2. [`EneHandle::open`] — ready before return
//! 3. [`EneHandle::run`] → [`TurnId`], subscribe to events, cancel by id
//!
//! ```bash
//! ENE_PROVIDER__API_KEY=sk-xxx direnv exec . rtk cargo run -p ene-runtime --example minimal_chat
//! ```

use ene_config::{load_character_card, load_config};
use ene_runtime::{
    CueSource, EneEvent, EneHandle, EneStatus, MultiAnswer, PermissionDecision, TerminalReason,
    UserInputResponse,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    println!("=== ene-runtime minimal chat example ===\n");

    let config = load_config()?;
    let card = load_character_card(&config.character)?;
    let ai_cfg = config
        .get_section::<ene_runtime::AiConfig>()
        .unwrap_or_default();
    let model = ai_cfg.tasks.chat.model.as_deref().unwrap_or("unknown");
    println!(
        "[Setup] provider: {}, model: {}",
        ai_cfg.tasks.chat.provider, model,
    );

    let handle = EneHandle::open(config, card).await?;
    println!("[Setup] Runtime ready.\n");

    let mut rx = handle.subscribe();

    let snapshot = handle.diagnostics().get_snapshot().await?;
    if let Some(card) = &snapshot.character_card {
        println!("[Snapshot] Character: {}", card.data.get_character_name());
    }
    println!("[Snapshot] Session ID: {}\n", snapshot.session_id);

    print!("You> ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let turn = handle.run(input)?;
    println!("[Turn] {turn}\n");

    loop {
        match rx.recv().await? {
            EneEvent::TextDelta {
                delta,
                turn: t,
                origin: _,
            } => {
                if t != turn {
                    return Err(format!("unexpected turn id: {t:?}").into());
                }
                print!("{delta}");
                io::stdout().flush()?;
            }
            EneEvent::Performance { cues, source, .. } => {
                for cue in cues {
                    println!(
                        "\n[Performance: {} ({})]",
                        cue.name,
                        match source {
                            CueSource::Affect => "affect",
                            CueSource::LlmAdvisory => "llm_advisory",
                            CueSource::LlmCommand => "llm_command",
                            CueSource::Hysteresis => "hysteresis",
                            CueSource::Fallback => "fallback",
                        }
                    );
                }
            }
            EneEvent::ToolCallStart { name, .. } => {
                println!("\n[Tool start: {name}]");
            }
            EneEvent::ToolCallResult { name, .. } => {
                println!("\n[Tool result: {name}]");
            }
            EneEvent::PermissionRequired {
                request_id,
                description,
                ..
            } => {
                println!("\n[Permission] {description}");
                let _ = handle.decide_permission(request_id, PermissionDecision::Deny);
            }
            EneEvent::UserInputRequired {
                request_id, prompt, ..
            } => {
                println!("\n[User input] {} item(s)", prompt.items.len());
                let _ = handle.submit_user_input(
                    request_id,
                    UserInputResponse::Multi(vec![MultiAnswer::Skip; prompt.items.len()]),
                );
            }
            EneEvent::ContextCompressed { level, .. } => {
                println!("\n[ContextCompressed: {level}]");
            }
            EneEvent::StatusChanged { status } => {
                if status == EneStatus::Error {
                    eprintln!("\n[Status: Error]");
                }
            }
            EneEvent::TurnStarted { .. } => {}
            EneEvent::Terminal {
                turn: t,
                origin: _,
                reason,
            } => {
                if t != turn {
                    return Err(format!("unexpected turn id: {t:?}").into());
                }
                match reason {
                    TerminalReason::Done => println!("\n\n[Done]"),
                    TerminalReason::Cancelled => println!("\n\n[Cancelled]"),
                    TerminalReason::Failed { message } => {
                        eprintln!("\n\n[Failed] {message}");
                    }
                }
                break;
            }
        }
    }

    let _ = handle.shutdown(std::time::Duration::from_secs(2)).await;
    Ok(())
}
