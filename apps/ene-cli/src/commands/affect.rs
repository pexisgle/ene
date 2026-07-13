use crate::commands::CliCommand;
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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let sub = parse_subcommand(arg);
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;
        let card_name = snapshot.card_name.as_str();
        match sub {
            "show" => match snapshot.memory.show_affect_state(card_name).await {
                Ok(state) => {
                    println!(
                        "[Affect] mood={} valence={:.2} arousal={:.2} trust={:.2} affinity={:.2}",
                        state.mood_label, state.valence, state.arousal, state.trust, state.affinity
                    );
                }
                Err(e) => println!("{}", style::error(format!("[Affect] Show error: {e}"))),
            },
            "reset" => match snapshot.memory.reset_affect_state(card_name).await {
                Ok(()) => println!("{}", style::success("[Affect] Reset to neutral state")),
                Err(e) => println!("{}", style::error(format!("[Affect] Reset error: {e}"))),
            },
            _ => {
                println!("{}", style::warning("Usage: /affect <show|reset>"));
            }
        }
        Ok(())
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
