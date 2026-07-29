use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;
use ene_runtime::UndoReport;

pub struct UndoCommand;

#[async_trait]
impl CliCommand for UndoCommand {
    fn name(&self) -> &'static str {
        "/undo"
    }

    fn description(&self) -> &'static str {
        "Undo the most recent reversible agent operation"
    }

    fn usage(&self) -> &'static str {
        "/undo"
    }

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let report = ctx
            .handle
            .undo()
            .await
            .map_err(|e| CliError::ActorError(e.to_string()))?;

        match report {
            UndoReport::NothingToUndo => {
                println!("Nothing to undo.");
            }
            UndoReport::Irreversible { metadata } => {
                eprintln!(
                    "{}",
                    style::error(format!(
                        "The most recent operation ({}) is irreversible and cannot be undone.",
                        metadata.tool_name
                    ))
                );
            }
            UndoReport::Reverted { metadata, output } => {
                let targets = if metadata.target_resources.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", metadata.target_resources.join(", "))
                };
                println!(
                    "{}",
                    style::success(format!("Undone {}{}.", metadata.tool_name, targets))
                );
                if !output.trim().is_empty() {
                    println!("{output}");
                }
            }
            UndoReport::Failed { metadata, error } => {
                eprintln!(
                    "{}",
                    style::error(format!("Failed to undo {}: {error}", metadata.tool_name))
                );
            }
        }
        Ok(CommandOutcome::Continue)
    }
}
