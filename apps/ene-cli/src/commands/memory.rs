use crate::commands::{CliCommand, CliError};
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
        "/memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<(), CliError> {
        let (subcmd, tail) = parse_subcommand_and_tail(arg);

        let diag = ctx.handle.diagnostics();
        let snapshot = diag
            .get_snapshot()
            .await
            .map_err(|e| CliError::ActorError(format!("Failed to get actor state: {e}")))?;
        let memory = diag.memory();

        match subcmd {
            "search" => handle_search(tail, memory, &snapshot).await,
            "list" => handle_list(tail, &snapshot).await,
            "inspect" => handle_inspect(tail, &snapshot).await,
            "why" => handle_why(tail, &snapshot).await,
            "pin" => handle_pin(tail, &snapshot).await,
            "archive" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_store::MemoryStatus::Archived,
                    "archive",
                )
                .await
            }
            "forget" => handle_forget(tail, &snapshot).await,
            "dispute" => {
                handle_transition(
                    tail,
                    &snapshot,
                    ene_store::MemoryStatus::Disputed,
                    "dispute",
                )
                .await
            }
            "restore" => handle_restore(tail, &snapshot).await,
            "status" => handle_status(&snapshot),
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

async fn handle_search(
    query: &str,
    memory: &ene_runtime::MemoryQueryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    if query.is_empty() {
        return Err(CliError::UsageError {
            usage: "/memory search <query>".to_string(),
        });
    }
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    let results = memory
        .search_typed_memories_hybrid(card_name, Some(&snapshot.config.user_name), query, 10)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Search error: {e}")))?;

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
    Ok(())
}

async fn handle_list(args: &str, snapshot: &ene_runtime::EneStateSnapshot) -> Result<(), CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    let kind = parse_kind_arg(args);
    let memories = snapshot
        .memory
        .list_typed_memories(card_name, kind, 50)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("List error: {e}")))?;

    if memories.is_empty() {
        println!("[Memory] No typed memories found.");
        return Ok(());
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
    Ok(())
}

async fn handle_inspect(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory inspect <id>".to_string(),
        });
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
                    .map_or_else(|| "-".to_string(), |ts| ts.to_rfc3339()),
                m.access_count
            );
            Ok(())
        }
        Ok(None) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Inspect error: {e}"))),
    }
}

async fn handle_why(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory why <id>".to_string(),
        });
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
                    .map_or_else(|| "never".to_string(), |ts| ts.to_rfc3339())
            );
            println!("  note: live recall score breakdown is shown by `/memory search <query>`");
            Ok(())
        }
        Ok(None) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Why error: {e}"))),
    }
}

async fn handle_pin(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory pin <id>".to_string(),
        });
    };
    match snapshot.memory.pin_typed_memory(id, true).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] Pinned id={id}")));
            Ok(())
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Pin error: {e}"))),
    }
}

async fn handle_forget(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory forget <id>".to_string(),
        });
    };
    match snapshot.memory.user_forget_typed_memory(id).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] forgotten id={id}")));
            Ok(())
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Forget error: {e}"))),
    }
}

async fn handle_restore(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory restore <id>".to_string(),
        });
    };
    match snapshot.memory.user_restore_typed_memory(id).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] restored id={id}")));
            Ok(())
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Restore error: {e}"))),
    }
}

async fn handle_transition(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
    status: ene_store::MemoryStatus,
    label: &str,
) -> Result<(), CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: format!("/memory {label} <id>"),
        });
    };
    match snapshot
        .memory
        .transition_typed_memory_status(id, status)
        .await
    {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] {label} id={id}")));
            Ok(())
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Update error: {e}"))),
    }
}

fn parse_id(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

fn parse_kind_arg(args: &str) -> Option<ene_store::MemoryKind> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "--kind" {
        return None;
    }
    match tokens[1] {
        "episodic" => Some(ene_store::MemoryKind::Episodic),
        "semantic" => Some(ene_store::MemoryKind::Semantic),
        "user_profile" => Some(ene_store::MemoryKind::UserProfile),
        "relationship" => Some(ene_store::MemoryKind::Relationship),
        "affective" => Some(ene_store::MemoryKind::Affective),
        "commitment" => Some(ene_store::MemoryKind::Commitment),
        "preference" => Some(ene_store::MemoryKind::Preference),
        "procedure" => Some(ene_store::MemoryKind::Procedure),
        "reflection" => Some(ene_store::MemoryKind::Reflection),
        _ => None,
    }
}

fn handle_status(snapshot: &ene_runtime::EneStateSnapshot) -> Result<(), CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    println!("--- Memory Status ({card_name}) ---");
    println!("  typed memory store: enabled");
    Ok(())
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
    fn parse_subcommand_and_tail_preserves_reset_confirmation() {
        let (subcmd, tail) = parse_subcommand_and_tail("reset legacy --yes");
        assert_eq!(subcmd, "reset");
        assert_eq!(tail, "legacy --yes");
    }

    #[test]
    fn parse_kind_arg_reads_kind_flag_value_pair() {
        assert_eq!(
            parse_kind_arg("--kind preference"),
            Some(ene_store::MemoryKind::Preference)
        );
    }
}
