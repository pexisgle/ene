use crate::commands::{CliCommand, CliError, CommandOutcome};
use crate::{context::AppContext, style};
use async_trait::async_trait;
use i18n_embed_fl::fl;

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
        "/memory <list|inspect|search|why|pin|archive|forget|dispute|restore|status|pending|retry|approval>"
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
            "list" => handle_list(tail, memory, &snapshot).await,
            "inspect" => handle_inspect(tail, memory).await,
            "why" => handle_why(tail, memory).await,
            "pin" => handle_pin(tail, memory).await,
            "archive" => {
                handle_transition(tail, memory, ene_store::MemoryStatus::Archived, "archive").await
            }
            "forget" => handle_forget(tail, memory).await,
            "dispute" => {
                handle_transition(tail, memory, ene_store::MemoryStatus::Disputed, "dispute").await
            }
            "restore" => handle_restore(tail, memory).await,
            "status" => handle_status(memory, &snapshot).await,
            "pending" => handle_pending(memory, &snapshot).await,
            "retry" => handle_retry(ctx, memory, &snapshot).await,
            "approval" => handle_approval(tail, ctx).await,
            _ => Err(CliError::UsageError {
                usage: self.usage().to_string(),
            }),
        }
    }
}

async fn handle_approval(arg: &str, ctx: &mut AppContext) -> Result<CommandOutcome, CliError> {
    let (sub, tail) = parse_subcommand_and_tail(arg);
    let candidates = ctx.handle.candidates();
    let turn = ctx.handle.active_turn();
    match sub {
        "" | "list" => approval_list(&candidates).await,
        "inspect" => approval_inspect(tail, &candidates).await,
        "approve" => approval_resolve(tail, true, &candidates, turn).await,
        "reject" => approval_resolve(tail, false, &candidates, turn).await,
        "edit" => approval_edit(tail, &candidates, turn).await,
        "history" => approval_history(&candidates).await,
        _ => Err(CliError::UsageError {
            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
        }),
    }
}

async fn approval_list(
    candidates: &ene_runtime::MemoryCandidateHandle,
) -> Result<CommandOutcome, CliError> {
    let rows = candidates.list_pending().await.map_err(|e| {
        CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-error",
            error = e.to_string()
        ))
    })?;
    if rows.is_empty() {
        println!("{}", fl!(crate::i18n::loader(), "memory-approval-empty"));
        return Ok(CommandOutcome::Continue);
    }
    println!(
        "{}",
        fl!(
            crate::i18n::loader(),
            "memory-approval-list-title",
            count = rows.len()
        )
    );
    for row in &rows {
        print_approval_summary(row);
    }
    Ok(CommandOutcome::Continue)
}

async fn approval_inspect(
    id_arg: &str,
    candidates: &ene_runtime::MemoryCandidateHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
        });
    };
    match candidates.inspect_pending(id).await {
        Ok(Some(row)) => {
            print_approval_summary(&row);
            Ok(CommandOutcome::Continue)
        }
        Ok(None) => Err(CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-not-found",
            id = id
        ))),
        Err(e) => Err(CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-error",
            error = e.to_string()
        ))),
    }
}

async fn approval_resolve(
    id_arg: &str,
    approved: bool,
    candidates: &ene_runtime::MemoryCandidateHandle,
    turn: Option<ene_runtime::TurnId>,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
        });
    };
    let result = if approved {
        candidates.approve(id, turn).await
    } else {
        candidates.reject(id, turn).await
    };
    match result {
        Ok(()) => {
            let message = if approved {
                fl!(crate::i18n::loader(), "memory-approval-approve-ok", id = id)
            } else {
                fl!(crate::i18n::loader(), "memory-approval-reject-ok", id = id)
            };
            println!("{}", style::success(message));
            Ok(CommandOutcome::Continue)
        }
        Err(e) => Err(CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-error",
            error = e.to_string()
        ))),
    }
}

async fn approval_edit(
    arg: &str,
    candidates: &ene_runtime::MemoryCandidateHandle,
    turn: Option<ene_runtime::TurnId>,
) -> Result<CommandOutcome, CliError> {
    let Some((id_arg, flags)) = arg.split_once(' ') else {
        return Err(CliError::UsageError {
            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
        });
    };
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
        });
    };
    let parsed = parse_edit_flags(flags)?;
    let edit = ene_store::PendingCandidateEdit {
        title: parsed.title,
        content: parsed.content,
        kind: parsed.kind,
        confidence: parsed.confidence,
    };
    match candidates.edit(id, edit, turn).await {
        Ok(()) => {
            println!(
                "{}",
                style::success(fl!(
                    crate::i18n::loader(),
                    "memory-approval-edit-ok",
                    id = id
                ))
            );
            Ok(CommandOutcome::Continue)
        }
        Err(e) => Err(CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-error",
            error = e.to_string()
        ))),
    }
}

async fn approval_history(
    candidates: &ene_runtime::MemoryCandidateHandle,
) -> Result<CommandOutcome, CliError> {
    let rows = candidates.history(50).await.map_err(|e| {
        CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-error",
            error = e.to_string()
        ))
    })?;
    if rows.is_empty() {
        println!(
            "{}",
            fl!(crate::i18n::loader(), "memory-approval-history-empty")
        );
        return Ok(CommandOutcome::Continue);
    }
    println!(
        "{}",
        fl!(
            crate::i18n::loader(),
            "memory-approval-history-title",
            count = rows.len()
        )
    );
    for row in &rows {
        print_approval_summary(row);
    }
    Ok(CommandOutcome::Continue)
}

struct EditFlags {
    title: String,
    content: String,
    kind: ene_store::MemoryKind,
    confidence: f32,
}

fn parse_edit_flags(arg: &str) -> Result<EditFlags, CliError> {
    let mut title = None;
    let mut content = None;
    let mut kind = None;
    let mut confidence = None;
    let mut tokens = arg.split_whitespace();
    while let Some(token) = tokens.next() {
        let (key, value) = match token.split_once('=') {
            Some((key, value)) if key.starts_with("--") => (key, Some(value.to_string())),
            _ if token.starts_with("--") => {
                let next = tokens.next();
                if next.is_some_and(|v| v.starts_with("--")) {
                    return Err(CliError::UsageError {
                        usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                    });
                }
                (token, next.map(str::to_string))
            }
            _ => {
                return Err(CliError::UsageError {
                    usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                });
            }
        };
        match key {
            "--title" => {
                title = Some(value.ok_or_else(|| CliError::UsageError {
                    usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                })?);
            }
            "--content" => {
                content = Some(value.ok_or_else(|| CliError::UsageError {
                    usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                })?);
            }
            "--kind" => {
                let kind_value = value.ok_or_else(|| CliError::UsageError {
                    usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                })?;
                kind = Some(crate::util::parse_memory_kind(&kind_value).ok_or_else(|| {
                    CliError::ExecutionFailed(fl!(
                        crate::i18n::loader(),
                        "memory-approval-edit-invalid-kind",
                        kind = kind_value
                    ))
                })?);
            }
            "--confidence" => {
                confidence = Some(
                    value
                        .ok_or_else(|| CliError::UsageError {
                            usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                        })?
                        .parse::<f32>()
                        .ok()
                        .filter(|v| v.is_finite() && (0.0..=1.0).contains(v))
                        .ok_or_else(|| {
                            CliError::ExecutionFailed(fl!(
                                crate::i18n::loader(),
                                "memory-approval-edit-invalid-confidence"
                            ))
                        })?,
                );
            }
            _ => {
                return Err(CliError::UsageError {
                    usage: fl!(crate::i18n::loader(), "memory-approval-usage").to_string(),
                });
            }
        }
    }
    let (Some(title), Some(content), Some(kind), Some(confidence)) =
        (title, content, kind, confidence)
    else {
        return Err(CliError::ExecutionFailed(fl!(
            crate::i18n::loader(),
            "memory-approval-edit-missing-flag"
        )));
    };
    Ok(EditFlags {
        title,
        content,
        kind,
        confidence,
    })
}

fn print_approval_summary(row: &ene_runtime::handle::PendingCandidateSummary) {
    println!(
        "\n--- {}: {} [{}] ---",
        fl!(crate::i18n::loader(), "memory-approval-label-id"),
        row.id,
        row.status
    );
    println!(
        "  {}: {}",
        fl!(crate::i18n::loader(), "memory-approval-label-title"),
        row.title
    );
    println!(
        "  {}: {}",
        fl!(crate::i18n::loader(), "memory-approval-label-kind"),
        row.kind
    );
    println!(
        "  {}: {:.2}",
        fl!(crate::i18n::loader(), "memory-approval-label-confidence"),
        row.confidence
    );
    println!(
        "  {}: {}",
        fl!(crate::i18n::loader(), "memory-approval-label-reason"),
        row.reason_detail
    );
    if !row.source_quote.is_empty() {
        println!(
            "  {}: {}",
            fl!(crate::i18n::loader(), "memory-approval-label-source-quote"),
            row.source_quote
        );
    }
    if let Some(turn) = &row.source_turn {
        println!(
            "  {}: {}",
            fl!(crate::i18n::loader(), "memory-approval-label-source-turn"),
            turn
        );
    }
    if let Some(existing) = &row.existing_memory_title {
        println!(
            "  {}: {} (id={})",
            fl!(crate::i18n::loader(), "memory-approval-label-conflict"),
            existing,
            row.existing_memory_id.unwrap_or_default()
        );
    }
    println!(
        "  {}: {}",
        fl!(crate::i18n::loader(), "memory-approval-label-created"),
        row.created_at
    );
    if let Some(resolved) = &row.resolved_at {
        println!(
            "  {}: {}",
            fl!(crate::i18n::loader(), "memory-approval-label-resolved"),
            resolved
        );
    }
    println!(
        "  {}: {}",
        fl!(crate::i18n::loader(), "memory-approval-label-status"),
        match row.status.as_str() {
            "approved" => fl!(crate::i18n::loader(), "memory-approval-status-approved"),
            "rejected" => fl!(crate::i18n::loader(), "memory-approval-status-rejected"),
            _ => fl!(crate::i18n::loader(), "memory-approval-status-pending"),
        }
    );
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
    memory: &ene_runtime::MemoryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    let kind = parse_kind_arg(args)?;
    let memories = memory
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
    memory: &ene_runtime::MemoryHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory inspect <id>".to_string(),
        });
    };
    match memory.inspect_typed_memory(id).await {
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
    memory: &ene_runtime::MemoryHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory why <id>".to_string(),
        });
    };
    match memory.inspect_typed_memory(id).await {
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
    memory: &ene_runtime::MemoryHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory pin <id>".to_string(),
        });
    };
    match memory.pin_typed_memory(id, true).await {
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
    memory: &ene_runtime::MemoryHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory forget <id>".to_string(),
        });
    };
    match memory.user_forget_typed_memory(id).await {
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
    memory: &ene_runtime::MemoryHandle,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: "/memory restore <id>".to_string(),
        });
    };
    match memory.user_restore_typed_memory(id).await {
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
    memory: &ene_runtime::MemoryHandle,
    status: ene_store::MemoryStatus,
    label: &str,
) -> Result<CommandOutcome, CliError> {
    let Some(id) = parse_id(id_arg) else {
        return Err(CliError::UsageError {
            usage: format!("/memory {label} <id>"),
        });
    };
    match memory.set_memory_status(id, status).await {
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
    memory: &ene_runtime::MemoryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let card_name = snapshot.card_name.as_str();
    println!("--- Memory Status ({card_name}) ---");
    println!("  typed memory store: enabled");
    match memory.count_pending_memory_writes(card_name).await {
        Ok((pending, permanent)) => {
            println!("  pending memory writes: {pending}");
            println!("  permanent write failures: {permanent}");
        }
        Err(e) => println!("  pending memory writes: error ({e})"),
    }
    println!("  note: use `/memory pending` to inspect the retry queue");
    Ok(CommandOutcome::Continue)
}

async fn handle_pending(
    memory: &ene_runtime::MemoryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let rows = memory
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
    ctx: &AppContext,
    memory: &ene_runtime::MemoryHandle,
    snapshot: &ene_runtime::EneStateSnapshot,
) -> Result<CommandOutcome, CliError> {
    if !memory.is_enabled() {
        return Err(CliError::ExecutionFailed(
            "Memory is not enabled.".to_string(),
        ));
    }
    let character_id = snapshot.card_name.as_str();
    let scheduled = memory
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
    let llm = ctx
        .handle
        .create_chat_provider()
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Chat provider error: {e}")))?;
    memory
        .drain_pending_memory_writes(&mind, llm.as_ref(), scheduled.max(1))
        .await
        .map_err(|e| CliError::ExecutionFailed(format!("Drain pending error: {e}")))?;
    match memory.count_pending_memory_writes(character_id).await {
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

    use super::{parse_edit_flags, parse_kind_arg, parse_subcommand_and_tail};

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

    #[test]
    fn parse_edit_flags_reads_all_required_flags() {
        let flags =
            parse_edit_flags("--title tea --content matcha --kind preference --confidence 0.9")
                .unwrap();
        assert_eq!(flags.title, "tea");
        assert_eq!(flags.content, "matcha");
        assert_eq!(flags.kind, ene_store::MemoryKind::Preference);
        assert!((flags.confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_edit_flags_accepts_equals_form() {
        let flags =
            parse_edit_flags("--title=tea --content=coffee --kind=preference --confidence=0.5")
                .unwrap();
        assert_eq!(flags.title, "tea");
        assert_eq!(flags.content, "coffee");
        assert!((flags.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_edit_flags_rejects_missing_and_unknown_flags() {
        assert!(parse_edit_flags("--title tea --kind preference --confidence 0.5").is_err());
        assert!(parse_edit_flags("--bogus tea").is_err());
        assert!(
            parse_edit_flags("--title --content x --kind preference --confidence 0.5").is_err(),
            "a missing value must not swallow the next flag"
        );
        assert!(
            parse_edit_flags("--confidence 1.5 --title t --content c --kind preference").is_err()
        );
    }
}
