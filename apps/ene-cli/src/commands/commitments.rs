use crate::commands::{CliCommand, CliError};
use crate::{context::AppContext, style};
use async_trait::async_trait;

pub struct CommitmentsCommand;

fn parse_parts(arg: &str) -> Vec<&str> {
    arg.split_whitespace().collect()
}

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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let parts = parse_parts(arg);
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
                    Ok(())
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
                        Ok(())
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
    use super::parse_parts;

    #[test]
    fn parse_parts_for_list_and_done_subcommands() {
        assert_eq!(parse_parts("list"), vec!["list"]);
        assert_eq!(parse_parts("done 42"), vec!["done", "42"]);
    }
}
