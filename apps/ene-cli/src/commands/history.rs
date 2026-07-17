use crate::commands::{CliCommand, CliError};
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

    async fn execute(&self, _arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;

        println!("--- Conversation History ---");
        for entry in &snapshot.history {
            println!("[{:?}] {}", entry.role, entry.content);
        }
        println!("----------------------------");

        Ok(())
    }
}
