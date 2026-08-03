mod affect;
mod card;
mod characters;
mod clear;
mod commitments;
mod config;
mod doctor;
mod greeting;
mod help;
mod history;
mod memory;
mod permissions;
mod prompt;
mod session;
mod store;
mod tool;
mod undo;

use crate::context::AppContext;
use async_trait::async_trait;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Usage: {usage}")]
    UsageError { usage: String },
    #[error("Actor error: {0}")]
    ActorError(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandOutcome {
    /// The REPL should continue to the next prompt.
    Continue,
    /// The REPL should shut down the actor and exit with the given code.
    Exit(i32),
}

#[async_trait]
pub trait CliCommand: Send + Sync {
    /// The name of the command, starting with a slash, e.g. "/card"
    fn name(&self) -> &'static str;

    fn description(&self) -> &'static str;

    fn usage(&self) -> &'static str;

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError>;
}

/// Static registry containing all CLI command implementations except `/quit`
pub static COMMANDS: &[&dyn CliCommand] = &[
    &clear::ClearCommand as &dyn CliCommand,
    &affect::AffectCommand as &dyn CliCommand,
    &prompt::PromptCommand as &dyn CliCommand,
    &card::CardCommand as &dyn CliCommand,
    &characters::CharactersCommand as &dyn CliCommand,
    &config::ConfigCommand as &dyn CliCommand,
    &history::HistoryCommand as &dyn CliCommand,
    &help::HelpCommand as &dyn CliCommand,
    &undo::UndoCommand as &dyn CliCommand,
    &tool::ToolCommand as &dyn CliCommand,
    &memory::MemoryCommand as &dyn CliCommand,
    &commitments::CommitmentsCommand as &dyn CliCommand,
    &session::SessionCommand as &dyn CliCommand,
    &permissions::PermissionsCommand as &dyn CliCommand,
    &doctor::DoctorCommand as &dyn CliCommand,
    &greeting::GreetingCommand as &dyn CliCommand,
    &store::StoreCommand as &dyn CliCommand,
];

/// Maximum time the REPL will wait for the actor to drain on shutdown.
pub const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn execute(input: &str, ctx: &mut AppContext) -> CommandOutcome {
    let parts: Vec<&str> = input.splitn(2, ' ').collect();
    let cmd = parts[0];
    let arg = parts.get(1).copied().unwrap_or("");

    // Signal the REPL to exit; `drain_and_exit` in repl.rs handles the
    // actual actor shutdown so we avoid a redundant double-shutdown here.
    if cmd == "/quit" || cmd == "/exit" {
        return CommandOutcome::Exit(0);
    }

    if let Some(command) = COMMANDS.iter().find(|c| c.name() == cmd) {
        match command.execute(arg, ctx).await {
            Ok(outcome) => return outcome,
            Err(err) => {
                eprintln!("{}", crate::style::error(err.to_string()));
            }
        }
    } else {
        eprintln!(
            "{}",
            crate::style::error(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "unknown-command",
                command = cmd.to_string()
            ))
        );
    }
    CommandOutcome::Continue
}
