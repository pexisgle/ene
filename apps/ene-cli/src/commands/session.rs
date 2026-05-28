use crate::{context::AppContext, style};
use ene_core::{SessionConfig, SplitReason, execute_split, truncate};

pub async fn execute(arg: &str, ctx: &mut AppContext) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    match subcmd {
        "info" => handle_info(ctx),
        "split" => handle_split(ctx).await,
        "summaries" => handle_summaries(ctx),
        _ => {
            println!("Usage: /session <info|split|summaries>");
        }
    }
}

fn handle_info(ctx: &AppContext) {
    println!("--- Session Info ---");
    println!("Session ID: {}", ctx.session.memory.session_id);
    println!(
        "Started: {}",
        ctx.session
            .memory
            .session_started_at
            .format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Elapsed: {} min", ctx.session.session_elapsed_minutes());
    println!("Turn count: {}", ctx.session.state.current_turn_count);
    println!(
        "History messages: {}",
        ctx.session.history.conversation_history.len()
    );
    let session_config = ctx
        .config
        .get_section::<SessionConfig>()
        .unwrap_or_default();
    println!("Auto-split: {}", session_config.auto_session_split);
    println!("Timeout: {} min", session_config.session_timeout_minutes);
    println!("Topic threshold: {}", session_config.topic_change_threshold);
    println!("--------------------");
}

async fn handle_split(ctx: &mut AppContext) {
    if ctx.session.history.conversation_history.is_empty() {
        println!(
            "{}",
            style::warning("[Session] Cannot split: No conversation history.")
        );
        return;
    }
    let Some(store) = &ctx.session.memory.memory_store else {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    };
    let Some(embedder) = &ctx.session.memory.embedding_provider else {
        println!(
            "{}",
            style::warning("[Session] Embedding provider is not available.")
        );
        return;
    };
    println!(
        "{}",
        style::header("[Session] Manually splitting session...")
    );
    let reason = SplitReason::Manual;
    match execute_split(
        &ctx.session.history.conversation_history,
        &ctx.session.memory.session_id,
        ctx.session.card_name(),
        &ctx.config.user_name,
        store,
        embedder,
        &ctx.config
            .get_section::<ene_memory::MemoryConfig>()
            .unwrap_or_default()
            .resolve_summarization_model(),
        &ctx.config
            .get_section::<ene_memory::MemoryConfig>()
            .unwrap_or_default()
            .resolve_summarization_base_url()
            .unwrap_or_default(),
        &ctx.config
            .get_section::<ene_core::ProviderConfig>()
            .unwrap_or_default()
            .resolve_api_key(),
        reason,
    )
    .await
    {
        Ok(result) => {
            println!(
                "{}",
                style::warning(format!(
                    "[Session] Summary: {}",
                    truncate(&result.summary, 120)
                ))
            );
            if !result.key_facts.is_empty() {
                let facts_str = result
                    .key_facts
                    .iter()
                    .map(|f| format!("{}:{}", f.key, f.value))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!(
                    "  {}",
                    style::warning(format!("[Session] Key Facts: {}", facts_str))
                );
            }
            ctx.session.reset_session();
            ctx.session.memory.session_id = result.new_session_id;
            println!(
                "{}",
                style::warning("[Session] Started a new conversation.")
            );
        }
        Err(e) => {
            println!("{}", style::error(format!("[Session] Split error: {}", e)));
        }
    }
}

fn handle_summaries(ctx: &AppContext) {
    let Some(store) = &ctx.session.memory.memory_store else {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    };
    let card_name = ctx.session.card_name();
    match store.list_recent_summaries(card_name, 10) {
        Ok(summaries) => {
            if summaries.is_empty() {
                println!("[Session] No saved conversation summaries found.");
            } else {
                println!("--- Past Conversation Summaries ({}) ---", summaries.len());
                for s in &summaries {
                    println!(
                        "  {} | {}",
                        s.ended_at.format("%Y-%m-%d %H:%M"),
                        truncate(&s.summary, 80),
                    );
                }
                println!("----------------------------------------");
            }
        }
        Err(e) => println!("[Session] Error: {}", e),
    }
}
