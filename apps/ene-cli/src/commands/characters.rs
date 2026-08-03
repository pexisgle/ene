use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::style;
use async_trait::async_trait;

pub struct CharactersCommand;

#[async_trait]
impl CliCommand for CharactersCommand {
    fn name(&self) -> &'static str {
        "/characters"
    }

    fn description(&self) -> &'static str {
        "List discovered characters"
    }

    fn usage(&self) -> &'static str {
        "/characters"
    }

    async fn execute(&self, _arg: &str, _ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let characters = ene_config::discover_characters(ene_config::assets_dir());
        if characters.is_empty() {
            println!("No characters found under assets/characters/.");
            return Ok(CommandOutcome::Continue);
        }
        println!("{}", style::success("Available characters:"));
        for entry in &characters {
            println!(
                "  - {}: {}",
                style::header(entry.name.as_str()),
                entry.card_path
            );
        }
        Ok(CommandOutcome::Continue)
    }
}
