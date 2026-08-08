use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;

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
        let card =
            ene_card::load_character_card_localized(&name, &crate::i18n::active_language_code())
                .map_err(|e| CliError::ExecutionFailed(format!("Failed to load card: {e}")))?;
        ctx.handle
            .set_character(card)
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to load character card: {e}")))?;
        println!(
            "{}",
            i18n_embed_fl::fl!(crate::i18n::loader(), "card-loaded", name = name)
        );
        if let Some(card) = ctx.handle.character_card() {
            println!("{}", crate::style::header(card.data.get_character_name()));
            if !card.data.creator_notes.trim().is_empty() {
                println!("{}", card.data.creator_notes.trim());
            }
        }
        Ok(CommandOutcome::Continue)
    }
}
