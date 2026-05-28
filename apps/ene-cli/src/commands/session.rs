use crate::{context::AppContext, style};
use ene_core::{SessionConfig, truncate};

pub async fn execute(arg: &str, ctx: &AppContext) {
    let parts: Vec<&str> = arg.splitn(2, ' ').collect();
    let subcmd = parts.first().copied().unwrap_or("");

    let Ok(snapshot) = ctx.handle.get_snapshot().await else {
        eprintln!("Failed to get actor state");
        return;
    };

    match subcmd {
        "info" => handle_info(&snapshot),
        "split" => {
            handle_split(ctx, &snapshot).await;
        }
        "summaries" => handle_summaries(&snapshot),
        _ => {
            println!("Usage: /session <info|split|summaries>");
        }
    }
}

fn handle_info(snapshot: &ene_core::EneStateSnapshot) {
    println!("--- Session Info ---");
    println!("Session ID: {}", snapshot.session_id);
    println!(
        "Started: {}",
        snapshot.session_started_at.format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("Elapsed: {} min", "? (not tracked locally)");
    println!("Turn count: {}", snapshot.current_turn_count);
    println!("History messages: {}", snapshot.history.len());
    let session_config = snapshot
        .config
        .get_section::<SessionConfig>()
        .unwrap_or_default();
    println!("Auto-split: {}", session_config.auto_session_split);
    println!("Timeout: {} min", session_config.session_timeout_minutes);
    println!("Topic threshold: {}", session_config.topic_change_threshold);
    println!("--------------------");
}

async fn handle_split(_ctx: &AppContext, snapshot: &ene_core::EneStateSnapshot) {
    if snapshot.history.is_empty() {
        println!(
            "{}",
            style::warning("[Session] Cannot split: No conversation history.")
        );
        return;
    }
    if snapshot.memory_store.is_none() {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    }
    if snapshot.embedding_provider.is_none() {
        println!(
            "{}",
            style::warning("[Session] Embedding provider is not available.")
        );
        return;
    }

    println!(
        "{}",
        style::header("[Session] Manually splitting session...")
    );
    match _ctx.handle.manual_split().await {
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
            println!("{}", style::warning("[Session] Session split completed."));
        }
        Err(e) => {
            println!("{}", style::error(format!("[Session] Split error: {}", e)));
        }
    }
}

fn handle_summaries(snapshot: &ene_core::EneStateSnapshot) {
    let Some(store) = &snapshot.memory_store else {
        println!("{}", style::warning("[Session] Memory is not enabled."));
        return;
    };
    let card_name = &snapshot.card_name;
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
