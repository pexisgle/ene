use crate::commands::CliCommand;
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

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let parts = parse_parts(arg);
        let sub = parts.first().copied().unwrap_or("");
        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;
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
                }
                Err(e) => println!("{}", style::error(format!("[Commitments] List error: {e}"))),
            },
            "done" => {
                let id = parts.get(1).and_then(|raw| raw.parse::<i64>().ok());
                let Some(id) = id else {
                    println!("{}", style::warning("Usage: /commitments done <id>"));
                    return Ok(());
                };
                match snapshot.memory.complete_commitment(id).await {
                    Ok(true) => {
                        println!("{}", style::success(format!("[Commitments] done id={id}")))
                    }
                    Ok(false) => println!(
                        "{}",
                        style::warning(format!("[Commitments] id={id} not found or not active"))
                    ),
                    Err(e) => {
                        println!("{}", style::error(format!("[Commitments] Done error: {e}")))
                    }
                }
            }
            _ => println!("{}", style::warning("Usage: /commitments <list|done <id>>")),
        }
        Ok(())
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
