use crate::commands::CliCommand;
use crate::{context::AppContext, style};
use async_trait::async_trait;

pub struct MemoryCommand;

fn parse_subcommand_and_tail(arg: &str) -> (&str, &str) {
    match arg.trim().split_once(' ') {
        Some((sub, rest)) => (sub, rest.trim()),
        None => (arg.trim(), ""),
    }
}

#[async_trait]
impl CliCommand for MemoryCommand {
    fn name(&self) -> &'static str {
        "/memory"
    }

    fn description(&self) -> &'static str {
        "Inspect and manage cognitive memories"
    }

    fn usage(&self) -> &'static str {
        "/memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status|migrate|reset>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), String> {
        let (subcmd, tail) = parse_subcommand_and_tail(arg);

        let snapshot = ctx
            .handle
            .get_snapshot()
            .await
            .map_err(|e| format!("Failed to get actor state: {e}"))?;

        match subcmd {
            "search" => handle_search(tail, &snapshot).await,
            "list" => handle_list(tail, &snapshot).await,
            "inspect" => handle_inspect(tail, &snapshot).await,
            "why" => handle_why(tail, &snapshot).await,
            "pin" => handle_pin(tail, &snapshot).await,
            "archive" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_memory::MemoryStatus::Archived,
                    "archived",
                )
                .await
            }
            "forget" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_memory::MemoryStatus::UserDeleted,
                    "forgotten",
                )
                .await
            }
            "dispute" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_memory::MemoryStatus::Disputed,
                    "disputed",
                )
                .await
            }
            "restore" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_memory::MemoryStatus::Active,
                    "restored",
                )
                .await
            }
            "status" => handle_status(&snapshot).await,
            "migrate" => handle_migrate(tail, &snapshot).await,
            "reset" => handle_reset(tail, &snapshot).await,
            _ => {
                println!(
                    "{}",
                    style::warning(
                        "Usage: /memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status|migrate|reset>",
                    )
                );
                println!("  list [--kind <kind>] - List typed memories");
                println!("  inspect <id>         - Show full memory details");
                println!("  search <query>       - Search typed memories (hybrid score)");
                println!("  why <id>             - Explain memory lifecycle/debug context");
                println!("  pin <id>             - Pin memory against natural decay");
                println!("  archive <id>         - Mark memory archived");
                println!("  forget <id>          - Mark memory as user_deleted");
                println!("  dispute <id>         - Mark memory as disputed");
                println!("  restore <id>         - Restore memory to active");
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
    let card_name = snapshot.card_name.as_str();
    match snapshot
        .memory
        .search_typed_memories_hybrid(card_name, Some(&snapshot.config.user_name), query, 10)
        .await
    {
        Ok(results) => {
            if results.is_empty() {
                println!(
                    "{}",
                    style::warning("[Memory] No matching typed memories found.")
                );
            } else {
                println!(
                    "{}",
                    style::success(format!("[Memory] {} matches:", results.len()))
                );
                for (i, scored) in results.iter().enumerate() {
                    println!(
                        "\n--- #{} id={} kind={} total={:.3} ---",
                        i + 1,
                        scored.item.id.unwrap_or_default(),
                        scored.item.kind.as_str(),
                        scored.breakdown.total
                    );
                    println!("  title: {}", scored.item.title);
                    println!(
                        "  why: vector={:.3} lexical={:.3} recency={:.3} salience={:.3} confidence={:.3}",
                        scored.breakdown.vector_similarity,
                        scored.breakdown.lexical_score,
                        scored.breakdown.recency_score,
                        scored.breakdown.salience,
                        scored.breakdown.confidence
                    );
                }
            }
        }
        Err(e) => println!("{}", style::error(format!("[Memory] Search error: {e}"))),
    }
}

async fn handle_list(args: &str, snapshot: &ene_core::EneStateSnapshot) {
    if !snapshot.memory.is_enabled() {
        println!("{}", style::warning("[Memory] Memory is not enabled."));
        return;
    }
    let card_name = snapshot.card_name.as_str();
    let kind = parse_kind_arg(args);
    match snapshot
        .memory
        .list_typed_memories(card_name, kind, 50)
        .await
    {
        Ok(memories) => {
            if memories.is_empty() {
                println!("[Memory] No typed memories found.");
                return;
            }
            println!("--- Typed Memories ({}) ---", memories.len());
            for memory in memories {
                println!(
                    "  id={} [{}|{}] {}",
                    memory.id.unwrap_or_default(),
                    memory.kind.as_str(),
                    memory.status.as_str(),
                    memory.title
                );
            }
            println!("----------------------------");
        }
        Err(e) => println!("[Memory] Error: {e}"),
    }
}

async fn handle_inspect(id_arg: &str, snapshot: &ene_core::EneStateSnapshot) {
    let Some(id) = parse_id(id_arg) else {
        println!("{}", style::warning("[Memory] Usage: /memory inspect <id>"));
        return;
    };
    match snapshot.memory.inspect_typed_memory(id).await {
        Ok(Some(m)) => {
            println!("id={}", m.id.unwrap_or_default());
            println!("kind={}", m.kind.as_str());
            println!("status={}", m.status.as_str());
            println!("title={}", m.title);
            println!("content={}", m.content);
            println!(
                "confidence={:.2} salience={:.2}",
                m.confidence.get(),
                m.salience.get()
            );
            println!(
                "source={} source_ref={}",
                m.source.as_str(),
                m.source_ref.unwrap_or_default()
            );
            println!(
                "last_accessed={} access_count={}",
                m.last_accessed_at
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "-".to_string()),
                m.access_count
            );
        }
        Ok(None) => println!("{}", style::warning(format!("[Memory] id={id} not found"))),
        Err(e) => println!("{}", style::error(format!("[Memory] Inspect error: {e}"))),
    }
}

async fn handle_why(id_arg: &str, snapshot: &ene_core::EneStateSnapshot) {
    let Some(id) = parse_id(id_arg) else {
        println!("{}", style::warning("[Memory] Usage: /memory why <id>"));
        return;
    };
    match snapshot.memory.inspect_typed_memory(id).await {
        Ok(Some(m)) => {
            println!(
                "[Memory] why id={}: status={}, confidence={:.2}, salience={:.2}, source={}, last_accessed={}",
                id,
                m.status.as_str(),
                m.confidence.get(),
                m.salience.get(),
                m.source.as_str(),
                m.last_accessed_at
                    .map(|ts| ts.to_rfc3339())
                    .unwrap_or_else(|| "never".to_string())
            );
            println!("  note: live recall score breakdown is shown by `/memory search <query>`");
        }
        Ok(None) => println!("{}", style::warning(format!("[Memory] id={id} not found"))),
        Err(e) => println!("{}", style::error(format!("[Memory] Why error: {e}"))),
    }
}

async fn handle_pin(id_arg: &str, snapshot: &ene_core::EneStateSnapshot) {
    let Some(id) = parse_id(id_arg) else {
        println!("{}", style::warning("[Memory] Usage: /memory pin <id>"));
        return;
    };
    match snapshot.memory.pin_typed_memory(id, true).await {
        Ok(true) => println!("{}", style::success(format!("[Memory] Pinned id={id}"))),
        Ok(false) => println!("{}", style::warning(format!("[Memory] id={id} not found"))),
        Err(e) => println!("{}", style::error(format!("[Memory] Pin error: {e}"))),
    }
}

async fn handle_transition(
    id_arg: &str,
    snapshot: &ene_core::EneStateSnapshot,
    status: ene_memory::MemoryStatus,
    label: &str,
) {
    let Some(id) = parse_id(id_arg) else {
        println!(
            "{}",
            style::warning(format!("[Memory] Usage: /memory {label} <id>"))
        );
        return;
    };
    match snapshot
        .memory
        .transition_typed_memory_status(id, status)
        .await
    {
        Ok(true) => println!("{}", style::success(format!("[Memory] {label} id={id}"))),
        Ok(false) => println!("{}", style::warning(format!("[Memory] id={id} not found"))),
        Err(e) => println!("{}", style::error(format!("[Memory] Update error: {e}"))),
    }
}

fn parse_id(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

fn parse_kind_arg(args: &str) -> Option<ene_memory::MemoryKind> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "--kind" {
        return None;
    }
    match tokens[1] {
        "episodic" => Some(ene_memory::MemoryKind::Episodic),
        "semantic" => Some(ene_memory::MemoryKind::Semantic),
        "user_profile" => Some(ene_memory::MemoryKind::UserProfile),
        "relationship" => Some(ene_memory::MemoryKind::Relationship),
        "affective" => Some(ene_memory::MemoryKind::Affective),
        "commitment" => Some(ene_memory::MemoryKind::Commitment),
        "preference" => Some(ene_memory::MemoryKind::Preference),
        "procedure" => Some(ene_memory::MemoryKind::Procedure),
        "reflection" => Some(ene_memory::MemoryKind::Reflection),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::{parse_kind_arg, parse_subcommand_and_tail};

    #[test]
    fn parse_subcommand_and_tail_preserves_multi_word_search_query() {
        let (subcmd, tail) = parse_subcommand_and_tail("search what tea do i like");
        assert_eq!(subcmd, "search");
        assert_eq!(tail, "what tea do i like");
    }

    #[test]
    fn parse_subcommand_and_tail_preserves_migrate_flags() {
        let (subcmd, tail) = parse_subcommand_and_tail("migrate legacy --dry-run");
        assert_eq!(subcmd, "migrate");
        assert_eq!(tail, "legacy --dry-run");
    }

    #[test]
    fn parse_subcommand_and_tail_preserves_reset_confirmation() {
        let (subcmd, tail) = parse_subcommand_and_tail("reset legacy --yes");
        assert_eq!(subcmd, "reset");
        assert_eq!(tail, "legacy --yes");
    }

    #[test]
    fn parse_kind_arg_reads_kind_flag_value_pair() {
        assert_eq!(
            parse_kind_arg("--kind preference"),
            Some(ene_memory::MemoryKind::Preference)
        );
    }
}
