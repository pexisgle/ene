use crate::commands::CliCommand;
use crate::context::AppContext;
use async_trait::async_trait;

pub struct UndoCommand;

#[async_trait]
impl CliCommand for UndoCommand {
    fn name(&self) -> &'static str {
        "/undo"
    }

    fn description(&self) -> &'static str {
        "Undo the most recent agent operation"
    }

    fn usage(&self) -> &'static str {
        "/undo"
    }

    async fn execute(&self, _arg: &str, _ctx: &mut AppContext) -> Result<(), String> {
        eprintln!("[Undo] Not yet supported with actor-based runtime. Use /tool call undo");
        Ok(())
    }
}
