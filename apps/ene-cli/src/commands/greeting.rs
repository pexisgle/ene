use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::context::AppContext;
use crate::terminal_ui::TerminalUi;
use async_trait::async_trait;

pub struct GreetingCommand;

#[async_trait]
impl CliCommand for GreetingCommand {
    fn name(&self) -> &'static str {
        "/greeting"
    }

    fn description(&self) -> &'static str {
        "Choose the character's opening greeting for a new session"
    }

    fn usage(&self) -> &'static str {
        "/greeting [<index>|none]"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let Some(card) = ctx.handle.character_card() else {
            return Err(CliError::ExecutionFailed(
                "No character card loaded.".to_string(),
            ));
        };
        let greetings = greeting_options(&card);
        if greetings.is_empty() {
            println!("This character has no greetings.");
            return Ok(CommandOutcome::Continue);
        }
        if !ctx.handle.history().await.map_err(cli_error)?.is_empty() {
            println!(
                "Greetings can only be chosen before the first message. \
                 Restart the REPL to open a new session."
            );
            return Ok(CommandOutcome::Continue);
        }

        let selection = if arg.is_empty() {
            select_interactively(&greetings)
        } else if arg == "none" {
            None
        } else {
            match arg.parse::<u32>() {
                Ok(index) => Some(index),
                Err(_) => {
                    return Err(CliError::UsageError {
                        usage: self.usage().to_string(),
                    });
                }
            }
        };

        match selection {
            None => {
                println!("No greeting selected.");
            }
            Some(index) => match ctx.handle.set_greeting(index).await {
                Ok(text) => {
                    println!("{}", crate::style::success("Greeting selected:"));
                    println!("{text}");
                }
                Err(e) => {
                    println!(
                        "{}",
                        crate::style::error(format!("Failed to set greeting: {e}"))
                    );
                }
            },
        }
        Ok(CommandOutcome::Continue)
    }
}

fn greeting_options(card: &ene_config::CharacterCardV3) -> Vec<(u32, String)> {
    let mut options = Vec::new();
    if !card.data.first_mes.trim().is_empty() {
        options.push((0, card.data.first_mes.trim().to_string()));
    }
    options.extend(
        card.data
            .alternate_greetings
            .iter()
            .enumerate()
            .filter(|(_, text)| !text.trim().is_empty())
            .map(|(i, text)| (i as u32 + 1, text.trim().to_string())),
    );
    options
}

fn select_interactively(greetings: &[(u32, String)]) -> Option<u32> {
    let ui = TerminalUi::global();
    for (index, text) in greetings {
        println!("[{index}] {text}");
    }
    let labels: Vec<String> = greetings
        .iter()
        .map(|(index, text)| format!("[{index}] {}", first_line(text)))
        .collect();
    let mut items = vec!["(none)".to_string()];
    items.extend(labels);
    ui.pause_for_external_prompt();
    let choice = dialoguer::Select::new()
        .with_prompt("Choose a greeting (Enter to confirm)")
        .items(&items)
        .default(0)
        .interact()
        .unwrap_or(0);
    ui.resume_after_external_prompt();
    (choice > 0).then(|| greetings[choice - 1].0)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

fn cli_error(e: ene_runtime::PublicApiError) -> CliError {
    match e {
        ene_runtime::PublicApiError::ActorDead => {
            CliError::ActorError("actor is no longer running".to_string())
        }
        other => CliError::ExecutionFailed(other.to_string()),
    }
}
