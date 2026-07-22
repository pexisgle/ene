use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;

pub struct HistoryCommand;

#[async_trait]
impl CliCommand for HistoryCommand {
    fn name(&self) -> &'static str {
        "/history"
    }

    fn description(&self) -> &'static str {
        "View conversation history"
    }

    fn usage(&self) -> &'static str {
        "/history"
    }

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;

        println!("--- Conversation History ---");
        for entry in &snapshot.history {
            let role_label = match entry.role {
                ene_ai::Role::System => "System",
                ene_ai::Role::User => "User",
                ene_ai::Role::Assistant => "Assistant",
            };
            println!("[{role_label}] {}", entry.content);
        }
        println!("----------------------------");

        Ok(CommandOutcome::Continue)
    }
}
