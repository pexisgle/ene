//! # ene-runtime Minimal Chat Example
//!
//! Demonstrates the API v1 host path:
//! 1. Load config + character card via `ene-config`
//! 2. [`EneHandle::open`] — ready before return
//! 3. [`EneHandle::run`] → [`TurnId`], subscribe to events, cancel by id
//!
//! ```bash
//! ENE_PROVIDER__API_KEY=sk-xxx direnv exec . rtk cargo run -p ene-runtime --example minimal_chat
//! ```

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "example binary prints turn/session output to the terminal by design"
)]

use ene_config::{load_character_card, load_config};
use ene_runtime::{
    CueSource, EneEvent, EneHandle, LifecycleEvent, MultiAnswer, PermissionDecision,
    TerminalReason, UserInputResponse,
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

    // Lifecycle notifications (status changes, pending memory candidates,
    // background tool completions) ride a separate bus from chat events —
    // drain them on their own task.
    let mut lifecycle_rx = handle.subscribe_lifecycle();
    tokio::spawn(async move {
        while let Ok(event) = lifecycle_rx.recv().await {
            // Status only ever reports `Idle` / `Running`: failures surface
            // on the chat bus as `EneEvent::Terminal { reason: Failed }`,
            // handled below.
            if let LifecycleEvent::StatusChanged { status } = event {
                eprintln!("\n[Status: {status:?}]");
            }
        }
    });

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
                            CueSource::Llm => "llm",
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
                drop(handle.decide_permission(request_id, PermissionDecision::Deny));
            }
            EneEvent::UserInputRequired {
                request_id, prompt, ..
            } => {
                println!("\n[User input] {} item(s)", prompt.items.len());
                drop(handle.submit_user_input(
                    request_id,
                    UserInputResponse::Multi(vec![MultiAnswer::Skip; prompt.items.len()]),
                ));
            }
            EneEvent::ContextCompressed { level, .. } => {
                println!("\n[ContextCompressed: {level}]");
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
                    TerminalReason::Declined => println!("\n\n[Declined]"),
                    TerminalReason::Failed { message } => {
                        eprintln!("\n\n[Failed] {message}");
                    }
                }
                break;
            }
        }
    }

    drop(handle.shutdown(std::time::Duration::from_secs(2)).await);
    Ok(())
}
