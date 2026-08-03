use crate::commands::session::session_error;
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
            return Err(CliError::ExecutionFailed(i18n_embed_fl::fl!(
                crate::i18n::loader(),
                "greeting-no-card"
            )));
        };
        let greetings = card.data.greeting_options();
        if greetings.is_empty() {
            println!(
                "{}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "greeting-no-greetings")
            );
            return Ok(CommandOutcome::Continue);
        }
        if !ctx
            .handle
            .history()
            .await
            .map_err(session_error)?
            .is_empty()
        {
            println!(
                "{}",
                i18n_embed_fl::fl!(crate::i18n::loader(), "greeting-history-not-empty")
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
                println!(
                    "{}",
                    i18n_embed_fl::fl!(crate::i18n::loader(), "greeting-none-selected")
                );
            }
            Some(index) => match ctx.handle.set_greeting(index).await {
                Ok(text) => {
                    println!(
                        "{}",
                        crate::style::success(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "greeting-selected"
                        ))
                    );
                    println!("{text}");
                }
                Err(e) => {
                    println!(
                        "{}",
                        crate::style::error(i18n_embed_fl::fl!(
                            crate::i18n::loader(),
                            "greeting-failed",
                            error = e.to_string()
                        ))
                    );
                }
            },
        }
        Ok(CommandOutcome::Continue)
    }
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
    let mut items = vec![i18n_embed_fl::fl!(crate::i18n::loader(), "greeting-none")];
    items.extend(labels);
    ui.pause_for_external_prompt();
    let choice = dialoguer::Select::new()
        .with_prompt(i18n_embed_fl::fl!(crate::i18n::loader(), "greeting-choose"))
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
