use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::{context::AppContext, style};
use async_trait::async_trait;

pub struct AffectCommand;

fn parse_subcommand(arg: &str) -> &str {
    arg.trim()
}

#[async_trait]
impl CliCommand for AffectCommand {
    fn name(&self) -> &'static str {
        "/affect"
    }

    fn description(&self) -> &'static str {
        "Inspect or reset cognitive affect state"
    }

    fn usage(&self) -> &'static str {
        "/affect <show|reset>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let sub = parse_subcommand(arg);
        let diag = ctx.handle.diagnostics();
        let snapshot = diag
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;
        let card_name = snapshot.card_name.as_str();
        // The snapshot no longer carries the memory handle; it lives on
        // the diagnostics facade, which is the documented access path.
        let memory = diag.memory();
        match sub {
            "show" => match memory.show_affect_state(card_name).await {
                Ok(state) => {
                    println!(
                        "[Affect] mood={} valence={:.2} arousal={:.2} trust={:.2} affinity={:.2}",
                        state.mood_label, state.valence, state.arousal, state.trust, state.affinity
                    );
                    Ok(CommandOutcome::Continue)
                }
                Err(e) => Err(CliError::ExecutionFailed(format!("Show error: {e}"))),
            },
            "reset" => match memory.reset_affect_state(card_name).await {
                Ok(()) => {
                    println!("{}", style::success("[Affect] Reset to neutral state"));
                    Ok(CommandOutcome::Continue)
                }
                Err(e) => Err(CliError::ExecutionFailed(format!("Reset error: {e}"))),
            },
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_subcommand;

    #[test]
    fn parse_subcommand_trims_whitespace() {
        assert_eq!(parse_subcommand("  show  "), "show");
        assert_eq!(parse_subcommand("  reset"), "reset");
    }
}
