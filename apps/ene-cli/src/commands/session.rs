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
        "/session <info|split|summaries|list|export <id>|import <path>|search <query>|archive <id>|unarchive <id>>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let parts: Vec<&str> = arg.splitn(2, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");
        let rest = parts.get(1).copied().unwrap_or("").trim();

        // Store-backed subcommands (#176) do not need an actor snapshot.
        match subcmd {
            "list" => return handle_list(ctx).await,
            "export" => return handle_export(ctx, rest).await,
            "import" => return handle_import(ctx, rest).await,
            "search" => return handle_search(ctx, rest).await,
            "archive" => return handle_archive(ctx, rest, true).await,
            "unarchive" => return handle_archive(ctx, rest, false).await,
            _ => {}
        }

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

async fn handle_list(ctx: &AppContext) -> Result<(), CliError> {
    let sessions = ctx
        .handle
        .list_sessions(false, 50)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to list sessions: {e}")))?;

    if sessions.is_empty() {
        println!("No stored sessions.");
    } else {
        println!("{}", style::success("Sessions (newest first):"));
        for s in sessions {
            println!(
                "  - {} | {} | {} turns | updated {}",
                style::header(s.session_id.as_str()),
                if s.title.is_empty() {
                    "(untitled)"
                } else {
                    s.title.as_str()
                },
                s.turn_count,
                s.updated_at.format("%Y-%m-%d %H:%M:%S")
            );
        }
    }
    Ok(())
}

async fn handle_export(ctx: &AppContext, session_id: &str) -> Result<(), CliError> {
    if session_id.is_empty() {
        return Err(CliError::UsageError {
            usage: "Usage: /session export <session_id>".to_string(),
        });
    }
    let json = ctx
        .handle
        .export_session(session_id)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to export session: {e}")))?;

    println!("{json}");
    Ok(())
}

async fn handle_import(ctx: &AppContext, path: &str) -> Result<(), CliError> {
    if path.is_empty() {
        return Err(CliError::UsageError {
            usage: "Usage: /session import <path-to-json>".to_string(),
        });
    }
    let json = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to read {path}: {e}")))?;

    let id = ctx
        .handle
        .import_session(json)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to import session: {e}")))?;

    println!(
        "{}",
        style::success(format!("Imported session (row id {id})."))
    );
    Ok(())
}

async fn handle_search(ctx: &AppContext, query: &str) -> Result<(), CliError> {
    if query.is_empty() {
        return Err(CliError::UsageError {
            usage: "Usage: /session search <query>".to_string(),
        });
    }
    let matches = ctx
        .handle
        .search_sessions(query, 20, 0)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to search sessions: {e}")))?;

    if matches.is_empty() {
        println!("No matching messages.");
    } else {
        println!("{}", style::success("Matching messages:"));
        for (session_id, msg) in matches {
            let preview = ene_config::Truncate::simple(&msg.content, 80);
            println!(
                "  [{}] {}: {}",
                style::header(session_id.as_str()),
                msg.role,
                preview
            );
        }
    }
    Ok(())
}

async fn handle_archive(
    ctx: &AppContext,
    session_id: &str,
    archived: bool,
) -> Result<(), CliError> {
    if session_id.is_empty() {
        return Err(CliError::UsageError {
            usage: format!(
                "Usage: /session {} <session_id>",
                if archived { "archive" } else { "unarchive" }
            ),
        });
    }
    let updated = ctx
        .handle
        .archive_session(session_id, archived)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?
        .map_err(|e| CliError::ExecutionFailed(format!("Failed to update session: {e}")))?;

    if updated {
        let verb = if archived { "Archived" } else { "Unarchived" };
        println!(
            "{}",
            style::success(format!("{verb} session {session_id}."))
        );
    } else {
        println!("No session found with id {session_id}.");
    }
    Ok(())
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
