mod affect;
mod card;
mod clear;
mod commitments;
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

/// Custom subcommand error representations.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Command was called with invalid or missing arguments
    #[error("Usage: {usage}")]
    UsageError {
        /// The usage description of the command
        usage: String,
    },
    /// The cognitive actor or runtime state failed
    #[error("Actor error: {0}")]
    ActorError(String),
    /// Subcommand execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    /// Config or storage database error
    #[error("Database error: {0}")]
    DatabaseError(String),
}

/// The return value of a CLI command run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The REPL should continue to the next prompt.
    Continue,
    /// The REPL should shut down the actor and exit with the given code.
    Exit(i32),
}

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
    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError>;
}

/// Static registry containing all CLI command implementations except `/quit`
pub static COMMANDS: &[&dyn CliCommand] = &[
    &clear::ClearCommand as &dyn CliCommand,
    &affect::AffectCommand as &dyn CliCommand,
    &prompt::PromptCommand as &dyn CliCommand,
    &card::CardCommand as &dyn CliCommand,
    &config::ConfigCommand as &dyn CliCommand,
    &history::HistoryCommand as &dyn CliCommand,
    &help::HelpCommand as &dyn CliCommand,
    &undo::UndoCommand as &dyn CliCommand,
    &tool::ToolCommand as &dyn CliCommand,
    &memory::MemoryCommand as &dyn CliCommand,
    &commitments::CommitmentsCommand as &dyn CliCommand,
    &session::SessionCommand as &dyn CliCommand,
];

/// Maximum time the REPL will wait for the actor to drain on shutdown.
pub const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Global command execution entrypoint.
/// Dispatches the input string to the appropriate command handler.
pub async fn execute(input: &str, ctx: &mut AppContext) -> CommandOutcome {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    // The user requested a dedicated early exit branch specifically for quit
    if cmd == "/quit" || cmd == "/exit" {
        // Send a clean shutdown command and await the actor's drain
        // so that pending memory writes, session splits, and tool
        // processes finish (or are killed) before we return to main.
        match ctx.handle.shutdown(SHUTDOWN_TIMEOUT).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!(
                    "{}",
                    crate::style::error(format!(
                        "Actor did not shut down within {SHUTDOWN_TIMEOUT:?}: {e}"
                    ))
                );
            }
        }
        return CommandOutcome::Exit(0);
    }

    if let Some(command) = COMMANDS.iter().find(|c| c.name() == cmd) {
        if let Err(err) = command.execute(arg, ctx).await {
            eprintln!("{}", crate::style::error(err.to_string()));
        }
    } else {
        eprintln!("{}", crate::style::error(format!("Unknown command: {cmd}")));
    }
    CommandOutcome::Continue
}
