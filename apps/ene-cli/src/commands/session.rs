use crate::commands::CliCommand;
use crate::{context::AppContext, style};
use async_trait::async_trait;
use ene_core::{SessionConfig, Truncate};

pub struct SessionCommand;

#[async_trait]
impl CliCommand for SessionCommand {
    fn name(&self) -> &'static str {
        "/session"
    }

    fn description(&self) -> &'static str {
        "Manage conversation sessions"
    }

    fn usage(&self) -> &'static str {
        "/session <info|split|summaries>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let parts: Vec<&str> = arg.splitn(2, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");

        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        match subcmd {
            "info" => handle_info(&snapshot),
            "split" => {
                handle_split(ctx, &snapshot).await;
            }
            "summaries" => handle_summaries(&snapshot).await,
            _ => {
                println!(
                    "{}",
                    style::warning("Usage: /session <info|split|summaries>")
                );
            }
        }
        Ok(())
    }
}

fn handle_info(snapshot: &ene_core::EneStateSnapshot) {
    println!("--- Session Info ---");
    println!("Session ID: {}", snapshot.session_id);
    println!(
        "Started: {}",
        snapshot.session_started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Elapsed: ? (not tracked locally) min");
    println!("Turn count: {}", snapshot.current_turn_count);
    println!("History messages: {}", snapshot.history.len());
    let session_config = snapshot
        .config
        .get_section::<SessionConfig>()
        .unwrap_or_default();
    println!("Auto-split: {}", session_config.auto_split);
    println!("Timeout: {} min", session_config.timeout_minutes);
    println!(
        "Split threshold: {} (topic wt: {})",
        session_config.split_weights.threshold, session_config.split_weights.topic
    );
    println!("--------------------");
}

async fn handle_split(ctx: &AppContext, snapshot: &ene_core::EneStateSnapshot) {
    if snapshot.history.is_empty() {
        println!(
            "{}",
            style::warning("[Session] Cannot split: No conversation history.")
        );
        return;
    }
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    }

    let cognition_enabled = snapshot
        .config
        .get_section::<ene_core::CognitionConfig>()
        .map(|c| c.enabled && c.context.compression_enabled)
        .unwrap_or(false);

    println!(
        "{}",
        style::header(if cognition_enabled {
            "[Session] Manually triggering context compression..."
        } else {
            "[Session] Manually splitting session..."
        })
    );
    match ctx.handle.manual_split().await {
        Ok(result) => {
            println!(
                "{}",
                style::warning(format!(
                    "[Session] Summary: {}",
                    Truncate::simple(&result.summary, 120)
                ))
            );
            if cognition_enabled {
                println!(
                    "{}",
                    style::warning(format!(
                        "[Session] Session ID unchanged: {}",
                        result.new_session_id
                    ))
                );
            } else if !result.key_facts.is_empty() {
                let facts_str = result
                    .key_facts
                    .iter()
                    .map(|f| format!("{}:{}", f.key, f.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "  {}",
                    style::warning(format!("[Session] Key Facts: {facts_str}"))
                );
            }
            println!(
                "{}",
                style::warning(if cognition_enabled {
                    "[Session] Context compression completed."
                } else {
                    "[Session] Session split completed."
                })
            );
        }
        Err(e) => {
            println!("{}", style::error(format!("[Session] Split error: {e}")));
        }
    }
}

async fn handle_summaries(snapshot: &ene_core::EneStateSnapshot) {
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    match snapshot.memory.list_recent_summaries(card_name, 10).await {
        Ok(summaries) => {
            if summaries.is_empty() {
                println!("[Session] No saved conversation summaries found.");
            } else {
                println!("--- Past Conversation Summaries ({}) ---", summaries.len());
                for s in &summaries {
                    println!(
                        "  {} | {}",
                        s.ended_at.format("%Y-%m-%d %H:%M"),
                        Truncate::simple(&s.summary, 80),
                    );
                }
                println!("----------------------------------------");
            }
        }
        Err(e) => println!("[Session] Error: {e}"),
    }
}
