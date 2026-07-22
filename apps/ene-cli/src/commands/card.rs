use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;
use ene_config::load_character_card;

pub struct CardCommand;

#[async_trait]
impl CliCommand for CardCommand {
    fn name(&self) -> &'static str {
        "/card"
    }

    fn description(&self) -> &'static str {
        "Load a new character card by name or path"
    }

    fn usage(&self) -> &'static str {
        "/card <name>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        if arg.is_empty() {
            return Err(CliError::UsageError {
                usage: self.usage().to_string(),
            });
        }

        let name = arg.to_string();
        let card = load_character_card(&name)
            .map_err(|e| CliError::ExecutionFailed(format!("Failed to load card: {e}")))?;
        ctx.handle
            .diagnostics()
            .set_character(card)
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to load character card: {e}")))?;
        println!("Character card loaded: {name}");
        Ok(CommandOutcome::Continue)
    }
}
