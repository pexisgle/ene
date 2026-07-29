use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use async_trait::async_trait;

pub struct HelpCommand;

#[async_trait]
impl CliCommand for HelpCommand {
    fn name(&self) -> &'static str {
        "/help"
    }

    fn description(&self) -> &'static str {
        "Show this help message"
    }

    fn usage(&self) -> &'static str {
        "/help"
    }

    async fn execute(&self, _arg: &str, _ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        println!(
            "{}",
            i18n_embed_fl::fl!(crate::i18n::loader(), "help-commands-title")
        );
        println!(
            "  {:<24} - {}",
            "/quit",
            i18n_embed_fl::fl!(crate::i18n::loader(), "help-quit")
        );
        for cmd in crate::commands::COMMANDS {
            println!("  {:<24} - {}", cmd.usage(), cmd.description());
        }
        Ok(CommandOutcome::Continue)
    }
}
