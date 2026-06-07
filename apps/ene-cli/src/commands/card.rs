use crate::commands::CliCommand;
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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        if arg.is_empty() {
            return Err("Usage: /card <name>".to_string());
        }

        let name = arg.to_string();
        let handle = ctx.handle.clone();
        tokio::spawn(async move {
            match handle.load_character(&name).await {
                Ok(()) => println!("Character card loaded: {name}"),
                Err(e) => eprintln!("Failed to load character card: {e}"),
            }
        });

        Ok(())
    }
}
