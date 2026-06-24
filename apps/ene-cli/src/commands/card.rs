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

        // Await the character load before returning
        // so the prompt does not reappear mid-swap and
        // a subsequent message cannot race the reload.
        // The previous form spawned a detached
        // tokio::task, returned Ok(()) immediately, and
        // let the next user input run on the old
        // character.
        let name = arg.to_string();
        match ctx.handle.load_character(&name).await {
            Ok(()) => {
                println!("Character card loaded: {name}");
                Ok(())
            }
            Err(e) => Err(format!("Failed to load character card: {e}")),
        }
    }
}
