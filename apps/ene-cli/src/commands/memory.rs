use crate::commands::{CliCommand, CliError, CommandOutcome};
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
        "/memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status|pending|retry>"
    }

    async fn execute(&self, arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
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
            "status" => handle_status(&snapshot).await,
            "pending" => handle_pending(&snapshot).await,
            "retry" => handle_retry(&snapshot).await,
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

async fn handle_search(
    query: &str,
    memory: &ene_runtime::MemoryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
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
    Ok(CommandOutcome::Continue)
}

async fn handle_list(
    args: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    let kind = parse_kind_arg(args)?;
    let memories = snapshot
        .memory
        .list_typed_memories(card_name, kind, 50)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("List error: {e}")))?;

    if memories.is_empty() {
        println!("[Memory] No typed memories found.");
        return Ok(CommandOutcome::Continue);
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
    Ok(CommandOutcome::Continue)
}

async fn handle_inspect(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
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
            Ok(CommandOutcome::Continue)
        }
        Ok(None) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Inspect error: {e}"))),
    }
}

async fn handle_why(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
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
            Ok(CommandOutcome::Continue)
        }
        Ok(None) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Why error: {e}"))),
    }
}

async fn handle_pin(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory pin <id>".to_string(),
        });
    };
    match snapshot.memory.pin_typed_memory(id, true).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] Pinned id={id}")));
            Ok(CommandOutcome::Continue)
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Pin error: {e}"))),
    }
}

async fn handle_forget(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory forget <id>".to_string(),
        });
    };
    match snapshot.memory.user_forget_typed_memory(id).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] forgotten id={id}")));
            Ok(CommandOutcome::Continue)
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Forget error: {e}"))),
    }
}

async fn handle_restore(
    id_arg: &str,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory restore <id>".to_string(),
        });
    };
    match snapshot.memory.user_restore_typed_memory(id).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] restored id={id}")));
            Ok(CommandOutcome::Continue)
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
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: format!("/memory {label} <id>"),
        });
    };
    match snapshot.memory.set_memory_status(id, status).await {
        Ok(true) => {
            println!("{}", style::success(format!("[Memory] {label} id={id}")));
            Ok(CommandOutcome::Continue)
        }
        Ok(false) => Err(CliError::ExecutionFailed(format!("id={id} not found"))),
        Err(e) => Err(CliError::ExecutionFailed(format!("Update error: {e}"))),
    }
}

fn parse_id(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok()
}

fn parse_kind_arg(args: &str) -> Result<Option<ene_store::MemoryKind>, CliError> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() < 2 || tokens[0] != "--kind" {
        return Ok(None);
    }
    crate::util::parse_memory_kind(tokens[1])
        .ok_or_else(|| CliError::UsageError {
            usage: format!(
                "Unknown memory kind '{}'. Valid: episodic, semantic, user_profile, relationship, affective, commitment, preference, procedure, reflection",
                tokens[1]
            ),
        })
        .map(Some)
}

async fn handle_status(
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    println!("--- Memory Status ({card_name}) ---");
    println!("  typed memory store: enabled");
    match snapshot.memory.count_pending_memory_writes(card_name).await {
        Ok((pending, permanent)) => {
            println!("  pending memory writes: {pending}");
            println!("  permanent write failures: {permanent}");
        }
        Err(e) => println!("  pending memory writes: error ({e})"),
    }
    println!("  note: use `/memory pending` to inspect the retry queue (#240)");
    Ok(CommandOutcome::Continue)
}

async fn handle_pending(
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let rows = snapshot
        .memory
        .list_pending_memory_writes(snapshot.card_name.as_str(), 50)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("List pending error: {e}")))?;
    if rows.is_empty() {
        println!("[Memory] No pending or permanent memory writes.");
        return Ok(CommandOutcome::Continue);
    }
    println!("--- Pending Memory Writes ({}) ---", rows.len());
    for row in rows {
        println!(
            "  id={} status={} attempts={}/{} next_retry={} error={}",
            row.id,
            row.status.as_str(),
            row.attempts,
            row.max_attempts,
            row.next_retry_at.to_rfc3339(),
            row.last_error.unwrap_or_default()
        );
    }
    Ok(CommandOutcome::Continue)
}

async fn handle_retry(
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !snapshot.memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let character_id = snapshot.card_name.as_str();
    let scheduled = snapshot
        .memory
        .schedule_pending_memory_writes_now(character_id)
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Schedule pending error: {e}")))?;
    if scheduled == 0 {
        println!("[Memory] No pending memory writes to retry.");
        return Ok(CommandOutcome::Continue);
    }
    let mind = snapshot
        .config
        .get_section::<ene_mind::MindConfig>()
        .unwrap_or_default();
    let llm = ene_ai::create_task_chat_provider(&snapshot.config, ene_ai::AiTaskKind::Chat)
        .map_err(|e| CliError::ExecutionFailed(format!("Chat provider error: {e}")))?;
    snapshot
        .memory
        .drain_pending_memory_writes(&mind, llm.as_ref(), scheduled.max(1))
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Drain pending error: {e}")))?;
    match snapshot
        .memory
        .count_pending_memory_writes(character_id)
        .await
    {
        Ok((pending, permanent)) => {
            println!(
                "[Memory] Retried {scheduled} write(s); remaining pending={pending} permanent={permanent}"
            );
        }
        Err(e) => println!("[Memory] Retried {scheduled} write(s); count error: {e}"),
    }
    Ok(CommandOutcome::Continue)
}

#[cfg(test)]
mod tests {
    #![expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")]

    use super::{parse_kind_arg, parse_subcommand_and_tail};

    #[test]
    fn parse_subcommand_and_tail_preserves_multi_word_search_query() {
        let (subcmd, tail) = parse_subcommand_and_tail("search what tea do i like");
        assert_eq!(subcmd, "search");
        assert_eq!(tail, "what tea do i like");
    }

    #[test]
    fn parse_kind_arg_reads_kind_flag_value_pair() {
        assert_eq!(
            parse_kind_arg("--kind preference").unwrap(),
            Some(ene_store::MemoryKind::Preference)
        );
    }

    #[test]
    fn parse_kind_arg_returns_error_for_unknown_kind() {
        assert!(parse_kind_arg("--kind unknown_kind").is_err());
    }
}
