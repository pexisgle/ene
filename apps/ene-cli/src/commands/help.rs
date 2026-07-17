use crate::commands::{CliCommand, CliError};
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

    async fn execute(&self, _arg: &str, _ctx: &mut AppContext) -> Result<(), CliError> {
        println!("Commands:");
        println!("  {:<24} - Exit the CLI", "/quit");
        for cmd in crate::commands::COMMANDS {
            println!("  {:<24} - {}", cmd.usage(), cmd.description());
        }
        Ok(())
    }
}
