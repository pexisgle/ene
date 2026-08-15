use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::{context::AppContext, style};
use async_trait::async_trait;
use ene_util::Truncate;

/// Preserves the actor-dead vs. execution-failure distinction so callers can
/// tell the actor has shut down from an ordinary failure.
pub(crate) fn session_error(e: ene_runtime::PublicApiError) -> CliError {
    match e {
        ene_runtime::PublicApiError::ActorDead => {
            CliError::ActorError("actor is no longer running".to_string())
        }
        other => CliError::ExecutionFailed(other.to_string()),
    }
}

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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let parts: Vec<&str> = arg.splitn(2, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");
        let rest = parts.get(1).copied().unwrap_or("").trim();

        // Store-backed subcommands do not need an actor snapshot.
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
                Ok(CommandOutcome::Continue)
            }
            "split" => handle_split(ctx, &snapshot).await,
            "summaries" => {
                handle_summaries(&snapshot);
                Ok(CommandOutcome::Continue)
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

async fn handle_list(ctx: &AppContext) -> Result<CommandOutcome, CliError> {
    let sessions = ctx
        .handle
        .sessions()
        .list(false, 50)
        .await
        .map_err(session_error)?;

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
    Ok(CommandOutcome::Continue)
}

async fn handle_export(ctx: &AppContext, session_id: &str) -> Result<CommandOutcome, CliError> {
    if session_id.is_empty() {
        return Err(CliError::UsageError {
            usage: "Usage: /session export <session_id>".to_string(),
        });
    }
    let json = ctx
        .handle
        .sessions()
        .export(session_id)
        .await
        .map_err(session_error)?;

    println!("{json}");
    Ok(CommandOutcome::Continue)
}

async fn handle_import(ctx: &AppContext, path: &str) -> Result<CommandOutcome, CliError> {
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
        .sessions()
        .import(&json)
        .await
        .map_err(session_error)?;

    println!(
        "{}",
        style::success(format!("Imported session (row id {id})."))
    );
    Ok(CommandOutcome::Continue)
}

async fn handle_search(ctx: &AppContext, query: &str) -> Result<CommandOutcome, CliError> {
    if query.is_empty() {
        return Err(CliError::UsageError {
            usage: "Usage: /session search <query>".to_string(),
        });
    }
    let matches = ctx
        .handle
        .sessions()
        .search(query, 20, 0)
        .await
        .map_err(session_error)?;

    if matches.is_empty() {
        println!("No matching messages.");
    } else {
        println!("{}", style::success("Matching messages:"));
        for (session_id, msg) in matches {
            let preview = ene_util::Truncate::simple(&msg.content, 80);
            println!(
                "  [{}] {}: {}",
                style::header(session_id.as_str()),
                msg.role,
                preview
            );
        }
    }
    Ok(CommandOutcome::Continue)
}

async fn handle_archive(
    ctx: &AppContext,
    session_id: &str,
    archived: bool,
) -> Result<CommandOutcome, CliError> {
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
        .sessions()
        .set_archived(session_id, archived)
        .await
        .map_err(session_error)?;

    if updated {
        let verb = if archived { "Archived" } else { "Unarchived" };
        println!(
            "{}",
            style::success(format!("{verb} session {session_id}."))
        );
    } else {
        println!("No session found with id {session_id}.");
    }
    Ok(CommandOutcome::Continue)
}

fn handle_info(snapshot: &ene_runtime::EneStateSnapshot) {
    println!("--- Session Info ---");
    println!("Session ID: {}", snapshot.session_id);
    println!(
        "Started: {}",
        snapshot.session_started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
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
) -> Result<CommandOutcome, CliError> {
    if snapshot.history.is_empty() {
        return Err(CliError::ExecutionFailed(
            "Cannot compress: No conversation history.".to_string(),
        ));
    }
    if !ctx.handle.diagnostics().memory().is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }

    println!(
        "{}",
        style::header("[Session] Manually triggering context compression...")
    );
    match ctx.handle.compress_context().await {
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
                    result.session_id
                ))
            );
            println!(
                "{}",
                style::warning("[Session] Context compression completed.")
            );
            Ok(CommandOutcome::Continue)
        }
        Err(e) => Err(CliError::ExecutionFailed(format!("Compress error: {e}"))),
    }
}

fn handle_summaries(_snapshot: &ene_runtime::EneStateSnapshot) {
    println!(
        "{}",
        style::warning(
            "[Session] Legacy conversation summaries are retired. Use typed memory via /memory list|search, or scene compression via /session split."
        )
    );
}
