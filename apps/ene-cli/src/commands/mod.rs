mod card;
mod clear;
mod config;
mod help;
mod history;
mod memory;
mod prompt;
mod session;
mod tool;
mod undo;

use crate::context::AppContext;
use async_trait::async_trait;

/// Trait that represents an individual CLI command.
#[async_trait]
pub trait CliCommand: Send + Sync {
    /// The name of the command, starting with a slash, e.g. "/card"
    fn name(&self) -> &'static str;

    /// Description of the command, shown in help
    fn description(&self) -> &'static str;

    /// Detailed usage information, e.g. "/card `<name>`"
    fn usage(&self) -> &'static str;

    /// Execute the command
    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String>;
}

/// Static registry containing all CLI command implementations except `/quit`
pub static COMMANDS: &[&dyn CliCommand] = &[
    &clear::ClearCommand as &dyn CliCommand,
    &prompt::PromptCommand as &dyn CliCommand,
    &card::CardCommand as &dyn CliCommand,
    &config::ConfigCommand as &dyn CliCommand,
    &history::HistoryCommand as &dyn CliCommand,
    &help::HelpCommand as &dyn CliCommand,
    &undo::UndoCommand as &dyn CliCommand,
    &tool::ToolCommand as &dyn CliCommand,
    &memory::MemoryCommand as &dyn CliCommand,
    &session::SessionCommand as &dyn CliCommand,
];

/// Global command execution entrypoint.
/// Dispatches the input string to the appropriate command handler.
pub async fn execute(input: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    // The user requested a dedicated early exit branch specifically for quit
    if cmd == "/quit" {
        std::process::exit(0);
    }

    if let Some(command) = COMMANDS.iter().find(|c| c.name() == cmd) {
        if let Err(err) = command.execute(arg, ctx).await {
            eprintln!("{}", crate::style::error(err));
        }
    } else {
        eprintln!("{}", crate::style::error(format!("Unknown command: {cmd}")));
    }
}
