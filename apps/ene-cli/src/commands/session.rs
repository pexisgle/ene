use crate::commands::{CliCommand, CliError};
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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let parts: Vec<&str> = arg.splitn(2, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");

        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;

        match subcmd {
            "info" => {
                handle_info(&snapshot);
                Ok(())
            }
            "split" => handle_split(ctx, &snapshot).await,
            "summaries" => {
                handle_summaries(&snapshot);
                Ok(())
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
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
    println!("Context compression: enabled");
    println!(
        "Scene turn threshold: {}",
        mind.context.scene_turn_threshold
    );
    println!("--------------------");
}

async fn handle_split(
    ctx: &AppContext,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    if snapshot.history.is_empty() {
        return Err(CliError::ExecutionFailed(
            "Cannot compress: No conversation history.".to_string(),
        ));
    }
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
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
            Ok(())
        }
        Err(e) => Err(CliError::ExecutionFailed(format!("Compress error: {e}"))),
    }
}

fn handle_summaries(_snapshot: &ene_runtime::EneStateSnapshot) {
    println!(
        "{}",
        style::warning(
            "[Session] Legacy conversation summaries are retired (#125). Use typed memory via /memory list|search, or scene compression via /session split."
        )
    );
}
