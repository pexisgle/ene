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
        "Search, list, migrate, and inspect memories"
    }

    fn usage(&self) -> &'static str {
        "/memory <search|list|status|migrate|reset>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let parts: Vec<&str> = arg.splitn(3, ' ').collect();
        let subcmd = parts.first().copied().unwrap_or("");

        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        match subcmd {
            "search" => handle_search(parts.get(1).copied().unwrap_or(""), &snapshot).await,
            "list" => handle_list(&snapshot).await,
            "status" => handle_status(&snapshot).await,
            "migrate" => handle_migrate(parts.get(1).copied().unwrap_or(""), &snapshot).await,
            "reset" => handle_reset(parts.get(1).copied().unwrap_or(""), &snapshot).await,
            _ => {
                println!(
                    "{}",
                    style::warning("Usage: /memory <search|list|status|migrate|reset>")
                );
                println!("  search <query>       - Search memories by similarity");
                println!("  list                 - List stored summaries and key facts");
                println!("  status               - Legacy row counts and migration marker");
                println!("  migrate legacy       - One-shot legacy → typed migration");
                println!("  migrate legacy --dry-run - Preview migration counts");
                println!("  reset legacy --yes   - Truncate legacy + typed memory (destructive)");
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
            let threshold = snapshot
                .config
                .get_section::<ene_memory::MemoryConfig>()
                .map(|c| c.similarity_threshold)
                .unwrap_or(0.5);
            match snapshot
                .memory
                .search_summaries(&embedding, card_name, 10, threshold)
                .await
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

async fn handle_list(snapshot: &ene_core::EneStateSnapshot) {
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    match snapshot.memory.list_recent_summaries(card_name, 50).await {
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
    if let Ok(facts) = snapshot.memory.get_all_keyfacts(card_name).await
        && !facts.is_empty()
    {
        println!("\n--- Key Facts ({}) ---", facts.len());
        for f in &facts {
            println!("  {}: {}", f.key, f.value);
        }
        println!("------------------------");
    }
}

async fn handle_status(snapshot: &ene_core::EneStateSnapshot) {
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    match snapshot.memory.count_legacy_rows(card_name).await {
        Ok(counts) => {
            println!("--- Legacy Memory Status ({card_name}) ---");
            println!("  summaries: {}", counts.summaries);
            println!("  keyfacts:  {}", counts.keyfacts);
            println!("  logs:      {}", counts.logs);
        }
        Err(e) => println!("{}", style::error(format!("[Memory] Status error: {e}"))),
    }
    match snapshot.memory.migration_status(card_name).await {
        Ok(Some(status)) => {
            println!("  migrated: yes ({})", status.migrated_at);
            println!("  strategy: {}", status.strategy);
        }
        Ok(None) => println!("  migrated: no"),
        Err(e) => println!(
            "{}",
            style::error(format!("[Memory] Migration status error: {e}"))
        ),
    }
}

async fn handle_migrate(args: &str, snapshot: &ene_core::EneStateSnapshot) {
    if args != "legacy" && args != "legacy --dry-run" {
        println!(
            "{}",
            style::warning("[Memory] Usage: /memory migrate legacy [--dry-run]")
        );
        return;
    }
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let dry_run = args.contains("--dry-run");
    let card_name = snapshot.card_name.as_str();
    let user_id = snapshot.config.user_name.as_str();
    match snapshot
        .memory
        .migrate_legacy(card_name, user_id, dry_run)
        .await
    {
        Ok(report) => {
            if dry_run {
                println!("[Memory] Dry run — would migrate:");
            } else {
                println!("{}", style::success("[Memory] Migration complete:"));
            }
            println!("  summaries → episodic: {}", report.summaries_migrated);
            println!("  keyfacts → typed:       {}", report.keyfacts_migrated);
            println!("  logs → spans:           {}", report.spans_migrated);
            println!("  skipped (existing):     {}", report.skipped_existing);
        }
        Err(e) => println!(
            "{}",
            style::error(format!("[Memory] Migration failed: {e}"))
        ),
    }
}

async fn handle_reset(args: &str, snapshot: &ene_core::EneStateSnapshot) {
    if args != "legacy --yes" {
        println!(
            "{}",
            style::warning("[Memory] Usage: /memory reset legacy --yes")
        );
        println!("  This permanently deletes legacy tables and typed memory for this card.");
        return;
    }
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    match snapshot.memory.reset_legacy_memory(card_name).await {
        Ok(()) => println!(
            "{}",
            style::success(format!("[Memory] Reset legacy memory for {card_name}"))
        ),
        Err(e) => println!("{}", style::error(format!("[Memory] Reset failed: {e}"))),
    }
}
