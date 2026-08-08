use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;

pub struct ImportCommand;

#[async_trait]
impl CliCommand for ImportCommand {
    fn name(&self) -> &'static str {
        "/import"
    }

    fn description(&self) -> &'static str {
        "Import a character card (PNG or CHARX) into the characters directory"
    }

    fn usage(&self) -> &'static str {
        "/import <path>"
    }

    async fn execute(&self, arg: &str, _ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let path = arg.trim();
        if path.is_empty() {
            return Err(CliError::UsageError {
                usage: self.usage().to_string(),
            });
        }
        let imported = ene_card::import_character_file(std::path::Path::new(path))
            .map_err(|e| CliError::ExecutionFailed(format!("Failed to import card: {e}")))?;
        println!(
            "Imported character '{}' to {}",
            imported.name, imported.card_path
        );
        Ok(CommandOutcome::Continue)
    }
}
