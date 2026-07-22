use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::{context::AppContext, style};
use async_trait::async_trait;

pub struct CommitmentsCommand;

#[async_trait]
impl CliCommand for CommitmentsCommand {
    fn name(&self) -> &'static str {
        "/commitments"
    }

    fn description(&self) -> &'static str {
        "List and complete active commitments"
    }

    fn usage(&self) -> &'static str {
        "/commitments <list|done <id>>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
        let parts: Vec<&str> = arg.split_whitespace().collect();
        let sub = parts.first().copied().unwrap_or("");
        let snapshot = ctx
            .handle
            .diagnostics()
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;
        let card_name = snapshot.card_name.as_str();
        let user_id = snapshot.config.user_name.as_str();
        match sub {
            "list" => match snapshot
                .memory
                .list_active_commitments(card_name, Some(user_id), 50)
                .await
            {
                Ok(rows) => {
                    if rows.is_empty() {
                        println!("[Commitments] No active commitments.");
                    } else {
                        println!("--- Active Commitments ({}) ---", rows.len());
                        for row in rows {
                            println!(
                                "  id={} [{}] {}",
                                row.id.unwrap_or_default(),
                                row.status.as_str(),
                                row.title
                            );
                        }
                    }
                    Ok(CommandOutcome::Continue)
                }
                Err(e) => Err(CliError::ExecutionFailed(format!("List error: {e}"))),
            },
            "done" => {
                let id = parts.get(1).and_then(|raw| raw.parse::<i64>().ok());
                let Some(id) = id else {
                    return Err(CliError::UsageError {
                        usage: "Usage: /commitments done <id>".to_string(),
                    });
                };
                match snapshot.memory.complete_commitment(id).await {
                    Ok(true) => {
                        println!("{}", style::success(format!("[Commitments] done id={id}")));
                        Ok(CommandOutcome::Continue)
                    }
                    Ok(false) => Err(CliError::ExecutionFailed(format!(
                        "id={id} not found or not active"
                    ))),
                    Err(e) => Err(CliError::ExecutionFailed(format!("Done error: {e}"))),
                }
            }
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn parse_parts_for_list_and_done_subcommands() {
        assert_eq!("list".split_whitespace().collect::<Vec<_>>(), vec!["list"]);
        assert_eq!(
            "done 42".split_whitespace().collect::<Vec<_>>(),
            vec!["done", "42"]
        );
    }
}
