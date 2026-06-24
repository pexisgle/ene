//! # ene-core Minimal Chat Example
//!
//! This example demonstrates the minimal usage of the `ene-core` library.
//!
//! ## Execution Flow
//!
//! 1. Spawn the actor using [`EneHandle::new`].
//! 2. Load and apply the configuration using [`EneHandle::load_config`].
//! 3. Load the character card using [`EneHandle::load_character`].
//! 4. Subscribe to the event stream using [`EneHandle::subscribe`].
//! 5. Send user input using [`EneHandle::run`].
//! 6. Loop and receive events using [`EneEventReceiver::recv`](ene_core::EneEventReceiver::recv).
//!
//! ## How to Run
//!
//! ```bash
//! # Set the API key via environment variables or write it in assets/settings.json
//! ENE_PROVIDER__API_KEY=sk-xxx direnv exec . cargo run -p ene-core --example minimal_chat
//! ```
//!
//! ## Required Settings
//!
//! Configure the following in `assets/settings.json` (or via `ENE_` prefixed env vars):
//!
//! ```json
//! {
//!   "provider": {
//!     "provider_name": "openai",
//!     "model": "gpt-4o-mini",
//!     "base_url": "https://api.openai.com/v1",
//!     "api_key": "sk-..."
//!   }
//! }
//! ```

use ene_core::{
    EneEvent, EneHandle, EneStatus, MultiAnswer, PermissionDecision, UserInputResponse,
};
use std::io::{self, Write};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Initialize Tracing ──────────────────────────────────────────────────
    // Tracing subscriber reads from RUST_LOG environment variable if set.
    tracing_subscriber::fmt::init();

    println!("=== ene-core minimal chat example ===\n");

    // ── 1. Spawn Actor ────────────────────────────────────────────────────
    // EneHandle::new() spawns the actor as a background task.
    // The shutdown command is automatically sent when the last handle is dropped.
    let handle = EneHandle::new();

    // ── 2. Load and Apply Configuration ───────────────────────────────────
    // load_config() merges assets/settings.json and ENE_* environment variables,
    // and initializes the provider, memory store, and tool registry.
    let config = handle.load_config().await?;
    let provider_cfg = config
        .get_section::<ene_core::ProviderConfig>()
        .unwrap_or_default();
    println!(
        "[Setup] provider: {}, model: {}",
        provider_cfg.name, provider_cfg.model,
    );
    println!("[Setup] Reconfigured.\n");

    // ── 3. Load Character Card ─────────────────────────────────────────────
    // load_character() loads the character card by name (e.g. "Alicia").
    // Bare names are automatically resolved to their full path.
    handle.load_character("Alicia").await?;
    println!("[Setup] Character loaded.\n");

    // ── 4. Subscribe to the Event Stream ──────────────────────────────────
    // subscribe() returns the broadcast channel receiver.
    // The handle can have multiple subscribers concurrently (e.g., CLI + Bevy UI).
    let mut rx = handle.subscribe();

    // ── 5. Inspect State Snapshot (Optional) ──────────────────────────────
    // get_snapshot() retrieves character card data, session ID, history, etc.
    let snapshot = handle.get_snapshot().await?;
    if let Some(card) = &snapshot.character_card {
        println!("[Setup] Character: {}", card.data.name);
    }
    println!("[Setup] Session ID: {}\n", snapshot.session_id.as_str());

    // ── 6. Chat Loop ───────────────────────────────────────────────────────
    println!("Type your message and press Enter. (Ctrl+C or /quit to exit)\n");

    loop {
        // Prompt for user input
        print!("You: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let input = input.trim().to_string();

        if input.is_empty() {
            continue;
        }
        if input == "/quit" || input == "/exit" {
            println!("Bye!");
            break;
        }
        if input == "/cancel" {
            handle.cancel()?;
            println!("[Cancelled]\n");
            continue;
        }
        if input == "/split" {
            // Manual session split: generates a summary, stores it in memory,
            // and starts a clean session. Requires memory to be enabled.
            match handle.manual_split().await {
                Ok(result) => {
                    println!("[Session Split] {}", result.summary);
                    println!("[New Session] {}\n", result.new_session_id.as_str());
                }
                Err(e) => eprintln!("[Split Error] {e}\n"),
            }
            continue;
        }

        // ── Send user input to the actor using run() ──────────────────────
        // This method is fire-and-forget; it returns send errors only.
        // The actual response arrives asynchronously as EneEvents.
        handle.run(&input)?;

        // ── Event Loop: receive events until Done or Failed ────────────────
        print!("Ene: ");
        io::stdout().flush()?;

        'stream: loop {
            // Wait for the next event with a timeout to prevent hanging
            let event =
                match tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv()).await {
                    Ok(Ok(ev)) => ev,
                    Ok(Err(_)) => {
                        // Broadcast lagged (events were dropped because of slow receiver)
                        eprintln!("\n[Warning] Event receiver lagged, some events were missed.");
                        continue;
                    }
                    Err(_) => {
                        eprintln!("\n[Timeout] No response within 60 seconds.");
                        break 'stream;
                    }
                };

            match event {
                // TextDelta: A chunk of text generated by the LLM
                EneEvent::TextDelta { delta } => {
                    print!("{delta}");
                    io::stdout().flush()?;
                }

                // SpecialToken: e.g., emotion tokens like <|emo:happy|>
                EneEvent::SpecialToken { token } => {
                    if let Some(emotion) = ene_core::extract_emotion_from_token(&token) {
                        print!(" [{emotion}]");
                    }
                }

                // Tool call requested by the LLM
                EneEvent::ToolCallStart { name, arguments } => {
                    println!(
                        "\n  [Tool↑] {} {}",
                        name,
                        &arguments[..arguments.len().min(80)]
                    );
                    print!("Ene: ");
                    io::stdout().flush()?;
                }

                // Tool execution result
                EneEvent::ToolCallResult { name, result } => {
                    let preview = &result[..result.len().min(120)];
                    println!("\n  [Tool↓] {name} → {preview}");
                    print!("Ene: ");
                    io::stdout().flush()?;
                }

                // Permission request: requires user approval for destructive/unsafe tools
                EneEvent::PermissionRequired {
                    request_id,
                    action,
                    target,
                    description,
                } => {
                    println!("\n  [Permission] {action} on `{target}`: {description}");
                    print!("  Allow? [y/N]: ");
                    io::stdout().flush()?;

                    let mut ans = String::new();
                    io::stdin().read_line(&mut ans)?;
                    let decision = if ans.trim().eq_ignore_ascii_case("y") {
                        PermissionDecision::AllowOnce
                    } else {
                        PermissionDecision::Deny
                    };
                    let _ = handle.decide_permission(request_id, decision);
                }

                // Interactive user input request (e.g. the `question` tool)
                EneEvent::UserInputRequired { request_id, prompt } => {
                    let mut answers: Vec<MultiAnswer> = Vec::with_capacity(prompt.items.len());
                    let mut cancelled = false;
                    for (i, item) in prompt.items.iter().enumerate() {
                        println!(
                            "\n  [Question {}/{}] {}",
                            i + 1,
                            prompt.items.len(),
                            item.question
                        );
                        if !item.options.is_empty() {
                            let mut opts = item.options.clone();
                            opts.push("(skip)".to_string());
                            opts.push("(cancel all)".to_string());
                            println!("  Options: {}", opts.join(" | "));
                        } else if !item.allow_free_text {
                            println!(
                                "  (no input; press Enter to skip, type 'cancel' to abort all)"
                            );
                        }
                        print!("  Answer: ");
                        io::stdout().flush()?;

                        let mut ans = String::new();
                        io::stdin().read_line(&mut ans)?;
                        let trimmed = ans.trim();
                        if trimmed.eq_ignore_ascii_case("cancel") {
                            cancelled = true;
                            break;
                        }
                        if let Some(opt) =
                            item.options.iter().find(|o| o.as_str() == trimmed).cloned()
                        {
                            answers.push(MultiAnswer::Selected { option: opt });
                        } else if !trimmed.is_empty() && item.allow_free_text {
                            answers.push(MultiAnswer::Answer {
                                text: trimmed.to_string(),
                            });
                        } else {
                            answers.push(MultiAnswer::Skip);
                        }
                    }

                    let response = if cancelled {
                        UserInputResponse::Cancel
                    } else {
                        UserInputResponse::Multi(answers)
                    };
                    let _ = handle.submit_user_input(request_id, response);
                }

                // Task progress for long running tasks
                EneEvent::TaskProgress {
                    step,
                    total_steps,
                    description,
                    ..
                } => {
                    let total = total_steps.map(|t| format!("/{t}")).unwrap_or_default();
                    println!("\n  [Progress {step}{total}] {description}");
                }

                // Session split occurred (auto or manual)
                EneEvent::SessionSplit { summary, reason } => {
                    println!(
                        "\n  [Session Split: {:?}] {}",
                        reason,
                        &summary[..summary.len().min(80)]
                    );
                }

                // Actor status update
                EneEvent::StatusChanged { status } => {
                    if status == EneStatus::Error {
                        eprintln!("\n  [Status] Error");
                    }
                }

                // Generation successfully completed
                EneEvent::Terminal(ene_core::TerminalReason::Done) => {
                    println!("\n");
                    break 'stream;
                }

                // Generation cancelled by the user
                EneEvent::Terminal(ene_core::TerminalReason::Cancelled) => {
                    println!("\n  [Cancelled]\n");
                    break 'stream;
                }

                // Error occurred during generation
                EneEvent::Terminal(ene_core::TerminalReason::Failed { message }) => {
                    eprintln!("\n  [Error] {message}\n");
                    break 'stream;
                }
            }
        }
    }

    // ── Cleanup ────────────────────────────────────────────────────────────
    // Dropping the handle sends the Shutdown command and exits the actor.
    drop(handle);

    Ok(())
}
