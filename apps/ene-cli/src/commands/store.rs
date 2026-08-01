//! Store backup / integrity diagnostic commands.

use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_store::{list_backups, restore_database};

pub struct StoreCommand;

#[async_trait]
impl CliCommand for StoreCommand {
    fn name(&self) -> &'static str {
        "/store"
    }

    fn description(&self) -> &'static str {
        "Backup, restore, and integrity-check the memory database"
    }

    fn usage(&self) -> &'static str {
        "/store <backup|list-backups|restore <path>|integrity>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let (sub, rest) = match arg.trim().split_once(' ') {
            Some((sub, rest)) => (sub, rest.trim()),
            None => (arg.trim(), ""),
        };
        match sub {
            "backup" => backup(ctx).await,
            "list-backups" | "list" => list(ctx),
            "restore" => restore(ctx, rest).await,
            "integrity" => integrity(ctx).await,
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

fn require_store(ctx: &AppContext) -> Result<&ene_runtime::MemoryHandle, CliError> {
    let memory = ctx.handle.diagnostics().memory();
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory store is not enabled.".to_string(),
        ));
    }
    Ok(memory)
}

async fn backup(ctx: &AppContext) -> Result<CommandOutcome, CliError> {
    let memory = require_store(ctx)?;
    let path = memory
        .backup()
        .await
        .map_err(|e| CliError::DatabaseError(e.to_string()))?;
    println!(
        "{}",
        style::success(format!("[Store] Backup created: {}", path.display()))
    );
    Ok(CommandOutcome::Continue)
}

fn list(ctx: &AppContext) -> Result<CommandOutcome, CliError> {
    let memory = require_store(ctx)?;
    let Some(db_path) = memory.path() else {
        return Err(CliError::ExecutionFailed(
            "In-memory store has no file backups.".to_string(),
        ));
    };
    let backups = list_backups(db_path).map_err(|e| CliError::DatabaseError(e.to_string()))?;
    if backups.is_empty() {
        println!("[Store] No backups found for {}.", db_path.display());
        return Ok(CommandOutcome::Continue);
    }
    println!("--- Backups for {} ---", db_path.display());
    for b in backups {
        println!("  {}", b.display());
    }
    Ok(CommandOutcome::Continue)
}

async fn restore(ctx: &mut AppContext, backup_path: &str) -> Result<CommandOutcome, CliError> {
    if backup_path.is_empty() {
        return Err(CliError::UsageError {
            usage: "/store restore <backup-path>".to_string(),
        });
    }
    let memory = require_store(ctx)?;
    let Some(db_path) = memory.path().map(std::path::Path::to_path_buf) else {
        return Err(CliError::ExecutionFailed(
            "In-memory store cannot be restored from a file.".to_string(),
        ));
    };
    // Shut down the actor so the DB file is released before overwrite.
    ctx.handle
        .shutdown(crate::commands::SHUTDOWN_TIMEOUT)
        .await
        .map_err(|e| CliError::ActorError(e.to_string()))?;
    restore_database(std::path::Path::new(backup_path), &db_path)
        .map_err(|e| CliError::DatabaseError(e.to_string()))?;
    println!(
        "{}",
        style::success(format!(
            "[Store] Restored {} from {}. Exiting so the store can be reopened cleanly.",
            db_path.display(),
            backup_path
        ))
    );
    Ok(CommandOutcome::Exit(0))
}

async fn integrity(ctx: &AppContext) -> Result<CommandOutcome, CliError> {
    let memory = require_store(ctx)?;
    memory
        .check_integrity()
        .await
        .map_err(|e| CliError::DatabaseError(e.to_string()))?;
    println!("{}", style::success("[Store] integrity_check: ok"));
    Ok(CommandOutcome::Continue)
}
