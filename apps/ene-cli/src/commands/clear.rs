use crate::commands::{CliCommand, CliError};
use crate::context::AppContext;
use async_trait::async_trait;

pub struct ClearCommand;

#[async_trait]
impl CliCommand for ClearCommand {
    fn name(&self) -> &'static str {
        "/clear"
    }

    fn description(&self) -> &'static str {
        "Clear conversation history"
    }

    fn usage(&self) -> &'static str {
        "/clear"
    }

    async fn execute(&self, _arg: &str, _ctx: &mut AppContext) -> Result<(), CliError> {
        println!("Conversation history will be refreshed on next run.");
        Ok(())
    }
}
