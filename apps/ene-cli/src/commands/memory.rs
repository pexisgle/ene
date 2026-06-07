use crate::commands::CliCommand;
use crate::{context::AppContext, style};
use async_trait::async_trait;
use ene_core::Truncate;

pub struct MemoryCommand;

#[async_trait]
impl CliCommand for MemoryCommand {
    fn name(&self) -> &'static str {
        "/memory"
    }

    fn description(&self) -> &'static str {
        "Search and list memories"
    }

    fn usage(&self) -> &'static str {
        "/memory <search|list>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let parts: Vec<&str> = arg.splitn(2, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");

        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        match subcmd {
            "search" => handle_search(parts.get(1).copied().unwrap_or(""), &snapshot).await,
            "list" => handle_list(&snapshot),
            _ => {
                println!("{}", style::warning("Usage: /memory <search|list>"));
                println!("  search <query> - Search memories by similarity");
                println!("  list           - List all stored memories");
            }
        }
        Ok(())
    }
}

async fn handle_search(query: &str, snapshot: &ene_core::EneStateSnapshot) {
    if query.is_empty() {
        println!(
            "{}",
            style::warning("[Memory] Usage: /memory search <query>")
        );
        return;
    }
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    println!(
        "{}",
        style::header(format!("[Memory] Searching query: {query}"))
    );
    match snapshot.memory.embed_query(query).await {
        Ok(embedding) => {
            let card_name = snapshot.card_name.as_str();
            let threshold = 0.0f32;
            match snapshot
                .memory
                .search_summaries(&embedding, card_name, 10, threshold)
            {
                Ok(results) => {
                    if results.is_empty() {
                        println!("{}", style::warning("[Memory] No matching memories found."));
                    } else {
                        println!(
                            "{}",
                            style::success(format!(
                                "[Memory] {} memories recalled:",
                                results.len()
                            ))
                        );
                        for (i, recalled) in results.iter().enumerate() {
                            println!(
                                "\n--- Memory #{} (similarity: {:.4}) ---",
                                i + 1,
                                recalled.similarity
                            );
                            println!("  Session ID: {}", recalled.entry.session_id);
                            println!(
                                "  Date: {}",
                                recalled.entry.ended_at.format("%Y-%m-%d %H:%M")
                            );
                            println!("  Summary: {}", recalled.entry.summary);
                        }
                    }
                }
                Err(e) => println!("{}", style::error(format!("[Memory] Search error: {e}"))),
            }
        }
        Err(e) => println!("{}", style::error(format!("[Memory] Embedding error: {e}"))),
    }
}

fn handle_list(snapshot: &ene_core::EneStateSnapshot) {
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    match snapshot.memory.list_recent_summaries(card_name, 50) {
        Ok(summaries) => {
            if summaries.is_empty() {
                println!("[Memory] No saved conversation summaries found.");
            } else {
                println!("--- Stored Summaries ({}) ---", summaries.len());
                for s in &summaries {
                    println!(
                        "  {} | {} | {}",
                        s.ended_at.format("%Y-%m-%d %H:%M"),
                        s.session_id,
                        Truncate::simple(&s.summary, 80)
                    );
                }
                println!("----------------------------------------");
            }
        }
        Err(e) => println!("[Memory] Error: {e}"),
    }
    if let Ok(facts) = snapshot.memory.get_all_keyfacts(card_name)
        && !facts.is_empty()
    {
        println!("\n--- Key Facts ({}) ---", facts.len());
        for f in &facts {
            println!("  {}: {}", f.key, f.value);
        }
        println!("------------------------");
    }
}
