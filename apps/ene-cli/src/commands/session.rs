use crate::commands::CliCommand;
use crate::{context::AppContext, style};
use async_trait::async_trait;
use ene_config::Truncate;

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
            .diagnostics()
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

fn handle_info(snapshot: &ene_runtime::EneStateSnapshot) {
    println!("--- Session Info ---");
    println!("Session ID: {}", snapshot.session_id);
    println!(
        "Started: {}",
        snapshot.session_started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Elapsed: ? (not tracked locally) min");
    println!("Turn count: {}", snapshot.current_turn_count);
    println!("History messages: {}", snapshot.history.len());
    let mind = snapshot
        .config
        .get_section::<ene_mind::MindConfig>()
        .unwrap_or_default();
    println!("Context compression: {}", mind.context.compression_enabled);
    println!(
        "Scene turn threshold: {}",
        mind.context.scene_turn_threshold
    );
    println!("--------------------");
}

async fn handle_split(ctx: &AppContext, snapshot: &ene_runtime::EneStateSnapshot) {
    if snapshot.history.is_empty() {
        println!(
            "{}",
            style::warning("[Session] Cannot compress: No conversation history.")
        );
        return;
    }
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    }

    println!(
        "{}",
        style::header("[Session] Manually triggering context compression...")
    );
    match ctx.handle.diagnostics().manual_split().await {
        Ok(result) => {
            println!(
                "{}",
                style::warning(format!(
                    "[Session] Summary: {}",
                    Truncate::simple(&result.summary, 120)
                ))
            );
            println!(
                "{}",
                style::warning(format!(
                    "[Session] Session ID unchanged: {}",
                    result.new_session_id
                ))
            );
            println!(
                "{}",
                style::warning("[Session] Context compression completed.")
            );
        }
        Err(e) => {
            println!("{}", style::error(format!("[Session] Compress error: {e}")));
        }
    }
}

async fn handle_summaries(_snapshot: &ene_runtime::EneStateSnapshot) {
    println!(
        "{}",
        style::warning(
            "[Session] Legacy conversation summaries are retired (#125). Use typed memory via /memory list|search, or scene compression via /session split."
        )
    );
}
